//! 只读文件头取图片的像素宽高，供阅读器在图片解码前按真实比例占位。
//! 不解码像素；JPEG 按段长跳读，遇到大段 ICC / EXIF 也只读几个段头。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// 宽高按显示方向给出：JPEG 的 EXIF 方向为 5–8 时交换，和浏览器默认的 `image-orientation: from-image` 一致。
pub fn image_size(path: &Path) -> Option<(u32, u32)> {
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut head = [0u8; 32];
    let n = read_up_to(&mut file, &mut head).ok()?;
    let head = &head[..n];
    let size = if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) && n >= 24 {
        Some((be32(&head[16..20]), be32(&head[20..24])))
    } else if (head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a")) && n >= 10 {
        Some((le16(&head[6..8]) as u32, le16(&head[8..10]) as u32))
    } else if head.starts_with(b"BM") && n >= 26 {
        Some((
            le32(&head[18..22]),
            (le32(&head[22..26]) as i32).unsigned_abs(),
        ))
    } else if head.starts_with(b"RIFF") && n >= 30 && &head[8..12] == b"WEBP" {
        webp(head)
    } else if head.starts_with(&[0xFF, 0xD8]) {
        file.seek(SeekFrom::Start(2)).ok()?;
        jpeg(&mut file)
    } else if head.len() >= 8 && &head[4..8] == b"ftyp" {
        avif_dimensions(&mut file)
    } else {
        None
    }?;
    (size.0 > 0 && size.1 > 0).then_some(size)
}

fn webp(head: &[u8]) -> Option<(u32, u32)> {
    match &head[12..16] {
        // 有损：帧头起始码后是 14 位宽高。
        b"VP8 " if head[23..26] == [0x9D, 0x01, 0x2A] => Some((
            (le16(&head[26..28]) & 0x3FFF) as u32,
            (le16(&head[28..30]) & 0x3FFF) as u32,
        )),
        // 无损：签名 0x2F 后 14 位宽减一、14 位高减一。
        b"VP8L" if head[20] == 0x2F => {
            let bits = le32(&head[21..25]);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        // 扩展：画布宽高各 24 位，减一存放。
        b"VP8X" => Some((le24(&head[24..27]) + 1, le24(&head[27..30]) + 1)),
        _ => None,
    }
}

/// 只走到 `ispe`。越界或畸形返回 `None`，不继续解析后面的盒。
fn avif_dimensions(file: &mut (impl Read + Seek)) -> Option<(u32, u32)> {
    let len = file.seek(SeekFrom::End(0)).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut prefix = [0u8; 256];
    let n = read_up_to(file, &mut prefix).ok()?;
    if crate::media::sniff_image_ext(&prefix[..n]) != Some("avif") {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    find_ispe(file, len).ok().flatten()
}

fn find_ispe(file: &mut (impl Read + Seek), end: u64) -> Result<Option<(u32, u32)>, ()> {
    loop {
        let pos = file.stream_position().map_err(|_| ())?;
        if pos >= end {
            return Ok(None);
        }
        if pos.checked_add(8).ok_or(())? > end {
            return Err(());
        }
        let mut header = [0u8; 8];
        file.read_exact(&mut header).map_err(|_| ())?;
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = [header[4], header[5], header[6], header[7]];
        let (header_len, size) = match size32 {
            0 => (8u64, end.checked_sub(pos).ok_or(())?),
            1 => {
                if pos.checked_add(16).ok_or(())? > end {
                    return Err(());
                }
                let mut large = [0u8; 8];
                file.read_exact(&mut large).map_err(|_| ())?;
                let size = u64::from_be_bytes(large);
                if size < 16 {
                    return Err(());
                }
                (16, size)
            }
            size if size < 8 => return Err(()),
            size => (8, u64::from(size)),
        };
        let box_end = pos.checked_add(size).ok_or(())?;
        if box_end > end {
            return Err(());
        }
        if &kind == b"ispe" {
            return read_ispe(file, box_end).map(Some);
        }
        if &kind == b"meta" || &kind == b"iprp" || &kind == b"ipco" {
            let extra = if &kind == b"meta" { 4 } else { 0 };
            let child = pos
                .checked_add(header_len)
                .ok_or(())?
                .checked_add(extra)
                .ok_or(())?;
            if child > box_end {
                return Err(());
            }
            file.seek(SeekFrom::Start(child)).map_err(|_| ())?;
            if let Some(size) = find_ispe(file, box_end)? {
                return Ok(Some(size));
            }
        }
        file.seek(SeekFrom::Start(box_end)).map_err(|_| ())?;
    }
}

fn read_ispe(file: &mut (impl Read + Seek), box_end: u64) -> Result<(u32, u32), ()> {
    let start = file.stream_position().map_err(|_| ())?;
    if start.checked_add(12).ok_or(())? > box_end {
        return Err(());
    }
    let mut payload = [0u8; 12];
    file.read_exact(&mut payload).map_err(|_| ())?;
    let width = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let height = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
    if width == 0 || height == 0 {
        return Err(());
    }
    Ok((width, height))
}

fn jpeg(file: &mut (impl Read + Seek)) -> Option<(u32, u32)> {
    let mut orientation = 1;
    loop {
        let mut marker = [0u8; 2];
        file.read_exact(&mut marker).ok()?;
        if marker[0] != 0xFF {
            return None;
        }
        // 段之间允许填充的 0xFF。
        let mut code = marker[1];
        while code == 0xFF {
            let mut b = [0u8; 1];
            file.read_exact(&mut b).ok()?;
            code = b[0];
        }
        if matches!(code, 0x01 | 0xD0..=0xD7) {
            continue;
        }
        if matches!(code, 0xD9 | 0xDA) {
            return None;
        }
        let mut len = [0u8; 2];
        file.read_exact(&mut len).ok()?;
        let len = u16::from_be_bytes(len) as i64;
        if len < 2 {
            return None;
        }
        let is_sof = matches!(code, 0xC0..=0xCF) && !matches!(code, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let mut sof = [0u8; 5];
            file.read_exact(&mut sof).ok()?;
            let (h, w) = (
                u16::from_be_bytes([sof[1], sof[2]]) as u32,
                u16::from_be_bytes([sof[3], sof[4]]) as u32,
            );
            return Some(if (5..=8).contains(&orientation) {
                (h, w)
            } else {
                (w, h)
            });
        }
        if code == 0xE1 && len >= 16 {
            let mut exif = vec![0u8; (len as usize - 2).min(64 * 1024)];
            file.read_exact(&mut exif).ok()?;
            orientation = exif_orientation(&exif).unwrap_or(orientation);
            file.seek(SeekFrom::Current(len - 2 - exif.len() as i64))
                .ok()?;
            continue;
        }
        file.seek(SeekFrom::Current(len - 2)).ok()?;
    }
}

/// APP1 段里的 EXIF：`Exif\0\0` + TIFF 头，第 0 个 IFD 里找 0x0112（方向）。
fn exif_orientation(data: &[u8]) -> Option<u16> {
    let tiff = data.strip_prefix(b"Exif\0\0")?;
    let big = match tiff.get(0..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        tiff.get(at..at + 2).map(|b| {
            if big {
                u16::from_be_bytes([b[0], b[1]])
            } else {
                u16::from_le_bytes([b[0], b[1]])
            }
        })
    };
    let u32_at = |at: usize| {
        tiff.get(at..at + 4).map(|b| {
            let b = [b[0], b[1], b[2], b[3]];
            if big {
                u32::from_be_bytes(b)
            } else {
                u32::from_le_bytes(b)
            }
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    (0..count)
        .map(|i| ifd + 2 + i * 12)
        .find(|&at| u16_at(at) == Some(0x0112))
        .and_then(|at| u16_at(at + 8))
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn le24(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], 0])
}
fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size_of(bytes: &[u8]) -> Option<(u32, u32)> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img");
        std::fs::write(&path, bytes).unwrap();
        image_size(&path)
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    fn jpeg(w: u16, h: u16, orientation: Option<u16>, padding: usize) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8];
        // 大 APP2（类似 ICC）：验证按段长跳过，而不是读进内存。
        b.extend_from_slice(&[0xFF, 0xE2]);
        b.extend_from_slice(&((padding + 2) as u16).to_be_bytes());
        b.extend(vec![0u8; padding]);
        if let Some(o) = orientation {
            let mut exif = b"Exif\0\0MM\0\x2A\0\0\0\x08".to_vec();
            exif.extend_from_slice(&1u16.to_be_bytes());
            exif.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]);
            exif.extend_from_slice(&o.to_be_bytes());
            exif.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
            b.extend_from_slice(&[0xFF, 0xE1]);
            b.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
            b.extend(exif);
        }
        b.extend_from_slice(&[0xFF, 0xC2, 0, 17, 8]);
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&[3; 10]);
        b
    }

    #[test]
    fn reads_common_formats() {
        assert_eq!(size_of(&png(1200, 630)), Some((1200, 630)));
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0x40, 0x01, 0xF0, 0x00, 0, 0]);
        assert_eq!(size_of(&gif), Some((320, 240)));
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 16]);
        bmp.extend_from_slice(&64u32.to_le_bytes());
        bmp.extend_from_slice(&(-32i32).to_le_bytes());
        assert_eq!(size_of(&bmp), Some((64, 32)));
        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X\x0A\0\0\0\0\0\0\0".to_vec();
        vp8x.extend_from_slice(&[0x7F, 0x07, 0x00, 0x37, 0x04, 0x00]);
        assert_eq!(size_of(&vp8x), Some((1920, 1080)));
        let mut vp8l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2F".to_vec();
        vp8l.extend_from_slice(&((99u32) | (49u32 << 14)).to_le_bytes());
        vp8l.extend_from_slice(&[0; 5]);
        assert_eq!(size_of(&vp8l), Some((100, 50)));
    }

    #[test]
    fn jpeg_skips_segments_and_honours_orientation() {
        assert_eq!(size_of(&jpeg(800, 600, None, 40_000)), Some((800, 600)));
        assert_eq!(size_of(&jpeg(800, 600, Some(1), 0)), Some((800, 600)));
        assert_eq!(size_of(&jpeg(800, 600, Some(6), 10)), Some((600, 800)));
    }

    fn bmff(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn avif(width: u32, height: u32) -> Vec<u8> {
        let mut ispe = vec![0, 0, 0, 0];
        ispe.extend_from_slice(&width.to_be_bytes());
        ispe.extend_from_slice(&height.to_be_bytes());
        let ipco = bmff(b"ipco", &bmff(b"ispe", &ispe));
        let mut meta = vec![0, 0, 0, 0];
        meta.extend_from_slice(&bmff(b"iprp", &ipco));
        let mut file = bmff(b"ftyp", b"avif\0\0\0\0");
        file.extend_from_slice(&bmff(b"meta", &meta));
        file
    }

    #[test]
    fn avif_ispe_size_stops_at_the_box_and_rejects_bad_input() {
        let file = avif(640, 480);
        assert_eq!(size_of(&file[..file.len() / 2]), None);
        let mut with_tail = file.clone();
        with_tail.extend_from_slice(&u32::MAX.to_be_bytes());
        with_tail.extend_from_slice(b"junk");
        assert_eq!(size_of(&with_tail), Some((640, 480)));
        assert_eq!(size_of(&avif(0, 480)), None);
        let mut broken = bmff(b"ftyp", b"avif\0\0\0\0");
        broken.extend_from_slice(&100u32.to_be_bytes());
        broken.extend_from_slice(b"meta");
        assert_eq!(size_of(&broken), None);
        assert_eq!(
            size_of(&[
                0, 0, 0, 16, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0, 0, 0, 0
            ]),
            None
        );
    }

    #[test]
    fn unknown_or_truncated_is_none() {
        assert_eq!(size_of(b"<svg/>"), None);
        assert_eq!(size_of(&png(1200, 630)[..20]), None);
        assert_eq!(size_of(&jpeg(800, 600, None, 10)[..20]), None);
        assert_eq!(size_of(&png(0, 10)), None);
    }
}
