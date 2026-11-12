//! 在消毒前把 iframe / video / audio 收成 Embed，避免 ammonia 剥掉。
//!
//! 分类只看主机名和路径（以及直链扩展名），禁止用 `contains("youtube.com")`。
//! 本 crate 不发网络请求。

use url::Url;

use crate::ir::{Block, EmbedKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectMedia {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedClass {
    Youtube { id: String, open_url: String },
    Bilibili { id: String, open_url: String },
    Direct { media: DirectMedia, url: String },
    External { open_url: String, label: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlVerdict {
    Official,
    Direct,
    Rejected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedEmbed {
    pub kind: EmbedKind,
    pub url: String,
    pub title: String,
}

/// 把 iframe / video / audio 从 HTML 里挖走，换成占位段落，避免播放器被追加到文末，
/// 也避免 `<audio>` 里的 “does not support” 提示留在正文。
pub fn excise_embeds(html: &str, base: Option<&Url>) -> (String, Vec<Block>) {
    let lower = html.to_ascii_lowercase();
    let mut spans: Vec<(usize, usize, Block)> = Vec::new();
    for tag in ["iframe", "video", "audio"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}");
        let mut from = 0;
        while let Some(idx) = lower[from..].find(&open) {
            let start = from + idx;
            let after = start + open.len();
            if after < lower.len() {
                let b = lower.as_bytes()[after];
                if b.is_ascii_alphanumeric() {
                    from = after;
                    continue;
                }
            }
            let Some(gt) = lower[start..].find('>') else {
                break;
            };
            let open_end = start + gt;
            let open_tag = &html[start..=open_end];
            let mut src = attr_value(open_tag, "src");
            let mut type_attr = attr_value(open_tag, "type");
            let element_end = match lower[open_end + 1..].find(&close) {
                Some(rel) => {
                    let close_at = open_end + 1 + rel;
                    let inner = &html[open_end + 1..close_at];
                    if src.is_none() && tag != "iframe" {
                        if let Some((source_src, source_type)) = first_source(inner) {
                            src = Some(source_src);
                            if type_attr.is_none() {
                                type_attr = source_type;
                            }
                        }
                    }
                    lower[close_at..]
                        .find('>')
                        .map(|gt| close_at + gt + 1)
                        .unwrap_or(close_at)
                }
                None => open_end + 1,
            };
            if let Some(raw) = src {
                if let Some(normalized) = normalize_url(&raw, base) {
                    let hint = match tag {
                        "audio" => EmbedHint::Audio,
                        "video" => EmbedHint::Video,
                        _ => EmbedHint::Iframe,
                    };
                    let class = classify_with_hint(&normalized, hint, type_attr.as_deref());
                    let attr_title = attr_value(open_tag, "title")
                        .or_else(|| attr_value(open_tag, "aria-label"));
                    let presented = present_class(&class, attr_title.as_deref());
                    tracing::debug!(kind = ?presented.kind, "embed collapsed");
                    spans.push((
                        start,
                        element_end,
                        Block::Embed {
                            kind: presented.kind,
                            url: presented.url,
                            title: presented.title,
                        },
                    ));
                }
            }
            from = element_end;
        }
    }
    spans.sort_by_key(|(start, _, _)| *start);
    let mut kept: Vec<(usize, usize, Block)> = Vec::new();
    let mut cursor = 0;
    for span in spans {
        if span.0 < cursor {
            continue;
        }
        cursor = span.1;
        kept.push(span);
    }
    let mut html_out = String::with_capacity(html.len());
    let mut blocks = Vec::with_capacity(kept.len());
    let mut at = 0;
    for (start, end, block) in kept {
        html_out.push_str(&html[at..start]);
        html_out.push_str("<p>RRSS_EMBED_SLOT_");
        html_out.push_str(&blocks.len().to_string());
        html_out.push_str("</p>");
        blocks.push(block);
        at = end;
    }
    html_out.push_str(&html[at..]);
    (html_out, blocks)
}

/// 阅读时按 URL 重分类。官方地址和直链扩展名压过存储的 kind；
/// 外壳 query 里的 youtube.com 不会因为旧 kind 被当成官方视频。
pub fn present_embed(stored: EmbedKind, url: &str, title: &str) -> PresentedEmbed {
    let class = classify_for_read(url, stored);
    let mut presented = present_class(&class, None);
    if usable_title(title) {
        presented.title = title.trim().to_string();
    }
    presented
}

pub fn classify_for_read(url: &str, stored: EmbedKind) -> EmbedClass {
    let normalized = normalize_url(url, None).unwrap_or_else(|| url.to_string());
    let (verdict, class) = classify_parts(&normalized, EmbedHint::Unknown, None);
    match verdict {
        UrlVerdict::Official | UrlVerdict::Direct | UrlVerdict::Rejected => class,
        UrlVerdict::Unknown => {
            let hint = match stored {
                EmbedKind::Video => EmbedHint::Video,
                EmbedKind::Audio => EmbedHint::Audio,
                _ => EmbedHint::Unknown,
            };
            if hint == EmbedHint::Unknown {
                class
            } else {
                classify_with_hint(&normalized, hint, None)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbedHint {
    Iframe,
    Video,
    Audio,
    Unknown,
}

fn classify_with_hint(url: &str, hint: EmbedHint, type_attr: Option<&str>) -> EmbedClass {
    classify_parts(url, hint, type_attr).1
}

fn classify_parts(url: &str, hint: EmbedHint, type_attr: Option<&str>) -> (UrlVerdict, EmbedClass) {
    let Some(parsed) = Url::parse(url).ok().filter(is_http) else {
        return (UrlVerdict::Unknown, external_for(url));
    };
    if let Some(class) = official(&parsed) {
        return (UrlVerdict::Official, class);
    }
    if rejected_container(&parsed) || type_is_playlist(type_attr) {
        return (UrlVerdict::Rejected, external_for(parsed.as_str()));
    }
    if let Some(media) = direct_extension(&parsed).or_else(|| type_media(type_attr)) {
        return (
            UrlVerdict::Direct,
            EmbedClass::Direct {
                media,
                url: parsed.to_string(),
            },
        );
    }
    match hint {
        EmbedHint::Video => (
            UrlVerdict::Direct,
            EmbedClass::Direct {
                media: DirectMedia::Video,
                url: parsed.to_string(),
            },
        ),
        EmbedHint::Audio => (
            UrlVerdict::Direct,
            EmbedClass::Direct {
                media: DirectMedia::Audio,
                url: parsed.to_string(),
            },
        ),
        EmbedHint::Iframe | EmbedHint::Unknown => {
            (UrlVerdict::Unknown, external_for(parsed.as_str()))
        }
    }
}

fn present_class(class: &EmbedClass, attr_title: Option<&str>) -> PresentedEmbed {
    let (kind, url, fallback) = match class {
        EmbedClass::Youtube { open_url, .. } => (EmbedKind::Youtube, open_url.clone(), "YouTube"),
        EmbedClass::Bilibili { open_url, .. } => {
            (EmbedKind::Bilibili, open_url.clone(), "哔哩哔哩")
        }
        EmbedClass::Direct { media, url } => {
            let kind = match media {
                DirectMedia::Audio => EmbedKind::Audio,
                DirectMedia::Video => EmbedKind::Video,
            };
            let fallback = match media {
                DirectMedia::Audio => "音频",
                DirectMedia::Video => "视频",
            };
            (kind, url.clone(), fallback)
        }
        EmbedClass::External { open_url, label } => {
            (EmbedKind::Iframe, open_url.clone(), label.as_str())
        }
    };
    let title = match attr_title
        .map(str::trim)
        .filter(|t| !t.is_empty() && !title_is_url(t))
    {
        Some(t) => t.to_string(),
        None => fallback.to_string(),
    };
    PresentedEmbed { kind, url, title }
}

fn official(url: &Url) -> Option<EmbedClass> {
    let host = url.host_str()?.to_ascii_lowercase();
    if host_is(&host, "youtube.com") {
        let id = youtube_id_on_youtube(url)?;
        return Some(EmbedClass::Youtube {
            open_url: youtube_open(&id),
            id,
        });
    }
    if host_is(&host, "youtu.be") {
        let id = first_segment(url).and_then(valid_yt_id)?;
        return Some(EmbedClass::Youtube {
            open_url: youtube_open(&id),
            id,
        });
    }
    if host_is(&host, "youtube-nocookie.com") {
        let mut segs = url.path_segments()?;
        if segs.next()? != "embed" {
            return None;
        }
        let id = valid_yt_id(segs.next()?)?;
        return Some(EmbedClass::Youtube {
            open_url: youtube_open(&id),
            id,
        });
    }
    if host == "player.bilibili.com" {
        if url.path() != "/player.html" {
            return None;
        }
        let bvid = url
            .query_pairs()
            .find(|(k, _)| k == "bvid")
            .and_then(|(_, v)| normalize_bvid(&v))?;
        return Some(EmbedClass::Bilibili {
            open_url: bilibili_open(&bvid),
            id: bvid,
        });
    }
    if host_is(&host, "bilibili.com") {
        let mut segs = url.path_segments()?;
        if segs.next()? != "video" {
            return None;
        }
        let bvid = normalize_bvid(segs.next()?)?;
        return Some(EmbedClass::Bilibili {
            open_url: bilibili_open(&bvid),
            id: bvid,
        });
    }
    None
}

fn youtube_id_on_youtube(url: &Url) -> Option<String> {
    let mut segs = url.path_segments()?;
    match segs.next()? {
        "watch" => {
            let id = url.query_pairs().find(|(k, _)| k == "v")?.1;
            valid_yt_id(&id)
        }
        "embed" | "shorts" => valid_yt_id(segs.next()?),
        _ => None,
    }
}

fn youtube_open(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

fn bilibili_open(bvid: &str) -> String {
    format!("https://www.bilibili.com/video/{bvid}/")
}

fn valid_yt_id(raw: &str) -> Option<String> {
    let id = raw.trim();
    if id.is_empty() || id.len() > 64 {
        return None;
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some(id.to_string())
}

fn normalize_bvid(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.len() < 3 || !raw[..2].eq_ignore_ascii_case("bv") {
        return None;
    }
    if !raw[2..].bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(format!("BV{}", &raw[2..]))
}

fn first_segment(url: &Url) -> Option<&str> {
    url.path_segments()?.next().filter(|s| !s.is_empty())
}

fn host_is(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn rejected_container(url: &Url) -> bool {
    matches!(path_ext(url).as_deref(), Some("mkv" | "avi" | "m3u8"))
}

fn direct_extension(url: &Url) -> Option<DirectMedia> {
    match path_ext(url)?.as_str() {
        "mp4" | "webm" | "ogv" => Some(DirectMedia::Video),
        "ogg" | "mp3" | "m4a" | "aac" | "wav" | "opus" => Some(DirectMedia::Audio),
        _ => None,
    }
}

fn path_ext(url: &Url) -> Option<String> {
    let seg = url.path_segments()?.next_back()?;
    let (_, ext) = seg.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 8 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

fn type_is_playlist(type_attr: Option<&str>) -> bool {
    let Some(t) = type_attr else { return false };
    let t = t.split(';').next().unwrap_or(t).trim().to_ascii_lowercase();
    t.contains("mpegurl") || t == "application/vnd.apple.mpegurl"
}

fn type_media(type_attr: Option<&str>) -> Option<DirectMedia> {
    let t = type_attr?
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match t.as_str() {
        "video/mp4" | "video/webm" | "video/ogg" => Some(DirectMedia::Video),
        "audio/mpeg" | "audio/mp4" | "audio/aac" | "audio/wav" | "audio/wave" | "audio/ogg"
        | "audio/opus" | "audio/webm" => Some(DirectMedia::Audio),
        _ => None,
    }
}

fn external_for(url: &str) -> EmbedClass {
    let open_url = normalize_url(url, None).unwrap_or_else(|| url.to_string());
    let label = site_label(&open_url);
    EmbedClass::External { open_url, label }
}

fn site_label(url: &str) -> String {
    Url::parse(url)
        .ok()
        .filter(is_http)
        .and_then(|u| u.host_str().map(str::to_string))
        .map(|host| host.trim_start_matches("www.").to_string())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "在浏览器打开".into())
}

fn title_is_url(title: &str) -> bool {
    let t = title.trim();
    t.contains("://") || t.starts_with("//") || t.contains('?')
}

fn usable_title(title: &str) -> bool {
    let t = title.trim();
    if t.is_empty() || title_is_url(t) {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    if lower.contains("youtube.com")
        || lower.contains("youtu.be")
        || lower.contains("bilibili.com")
        || t.contains("在浏览器打开")
    {
        return false;
    }
    true
}

fn is_http(url: &Url) -> bool {
    url.scheme() == "http" || url.scheme() == "https"
}

fn normalize_url(raw: &str, base: Option<&Url>) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let with_scheme = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        raw.to_string()
    };
    if let Ok(url) = Url::parse(&with_scheme) {
        if is_http(&url) {
            return Some(url.to_string());
        }
    }
    let base = base?;
    let url = base.join(raw).ok()?;
    is_http(&url).then(|| url.to_string())
}

fn first_source(inner: &str) -> Option<(String, Option<String>)> {
    let lower = inner.to_ascii_lowercase();
    let mut from = 0;
    while let Some(idx) = lower[from..].find("<source") {
        let start = from + idx;
        let after = start + "<source".len();
        if after < lower.len() && lower.as_bytes()[after].is_ascii_alphanumeric() {
            from = after;
            continue;
        }
        let gt = lower[start..].find('>')?;
        let tag = &inner[start..start + gt + 1];
        if let Some(src) = attr_value(tag, "src") {
            return Some((src, attr_value(tag, "type")));
        }
        from = start + gt + 1;
    }
    None
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let lower = tag.to_ascii_lowercase();
    let i = lower.find(&needle)?;
    let rest = &tag[i + needle.len()..];
    let rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)?;
        Some(rest[1..=end].to_string())
    } else {
        Some(
            rest.split(|c: char| c.is_whitespace() || c == '>')
                .next()?
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, EmbedKind, RenderOpts, html_to_document};

    fn embed(html: &str) -> Block {
        let doc = html_to_document(html, &RenderOpts::default());
        doc.blocks
            .into_iter()
            .find(|b| matches!(b, Block::Embed { .. }))
            .unwrap_or_else(|| panic!("no embed in {html}"))
    }

    #[test]
    fn official_urls_become_canonical_cards() {
        let yt = embed(r#"<iframe src="https://www.youtube.com/embed/abc123def45"></iframe>"#);
        match yt {
            Block::Embed { kind, url, .. } => {
                assert_eq!(kind, EmbedKind::Youtube);
                assert_eq!(url, "https://www.youtube.com/watch?v=abc123def45");
            }
            other => panic!("{other:?}"),
        }
        let short = embed(r#"<iframe src="https://youtu.be/abc123def45"></iframe>"#);
        assert!(matches!(
            short,
            Block::Embed { kind: EmbedKind::Youtube, ref url, .. } if url == "https://www.youtube.com/watch?v=abc123def45"
        ));
        let shorts = embed(r#"<iframe src="https://www.youtube.com/shorts/abc123def45"></iframe>"#);
        assert!(matches!(
            shorts,
            Block::Embed { kind: EmbedKind::Youtube, ref url, .. } if url == "https://www.youtube.com/watch?v=abc123def45"
        ));
        let nocookie =
            embed(r#"<iframe src="https://www.youtube-nocookie.com/embed/abc123def45"></iframe>"#);
        assert!(matches!(
            nocookie,
            Block::Embed { kind: EmbedKind::Youtube, ref url, .. } if url == "https://www.youtube.com/watch?v=abc123def45"
        ));
        let watch =
            embed(r#"<iframe src="https://www.youtube.com/watch?v=abc123def45&t=3s"></iframe>"#);
        assert!(matches!(
            watch,
            Block::Embed { kind: EmbedKind::Youtube, ref url, .. } if url == "https://www.youtube.com/watch?v=abc123def45"
        ));
        let bili = embed(
            r#"<iframe src="https://player.bilibili.com/player.html?bvid=BV1xx411c7mD&page=1"></iframe>"#,
        );
        match bili {
            Block::Embed { kind, url, .. } => {
                assert_eq!(kind, EmbedKind::Bilibili);
                assert_eq!(url, "https://www.bilibili.com/video/BV1xx411c7mD/");
            }
            other => panic!("{other:?}"),
        }
        let page = embed(r#"<iframe src="https://www.bilibili.com/video/BV1xx411c7mD/"></iframe>"#);
        assert!(matches!(
            page,
            Block::Embed { kind: EmbedKind::Bilibili, ref url, .. } if url == "https://www.bilibili.com/video/BV1xx411c7mD/"
        ));
    }

    #[test]
    fn iframe_inside_shell_div_is_still_official() {
        let block = embed(
            r#"<div class="player"><iframe src="https://www.youtube.com/embed/abc123def45" title="讲座"></iframe></div>"#,
        );
        assert!(matches!(
            block,
            Block::Embed { kind: EmbedKind::Youtube, ref url, ref title }
                if url == "https://www.youtube.com/watch?v=abc123def45" && title == "讲座"
        ));
    }

    #[test]
    fn protocol_relative_youtube_is_https() {
        let block = embed(r#"<iframe src="//www.youtube.com/embed/abc123def45"></iframe>"#);
        match block {
            Block::Embed { kind, url, .. } => {
                assert_eq!(kind, EmbedKind::Youtube);
                assert_eq!(url, "https://www.youtube.com/watch?v=abc123def45");
                assert!(url.starts_with("https://"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn embedly_query_containing_youtube_is_not_youtube() {
        let block = embed(
            r#"<div class="embedly"><iframe src="https://cdn.embedly.com/widgets/media.html?src=https%3A%2F%2Fwww.youtube.com%2Fembed%2Fabc123def45&url=https%3A%2F%2Fyoutube.com%2Fwatch%3Fv%3Dabc123def45"></iframe></div>"#,
        );
        match block {
            Block::Embed { kind, url, title } => {
                assert_eq!(kind, EmbedKind::Iframe);
                assert!(url.contains("embedly.com"), "{url}");
                assert!(!title.contains("youtube.com"), "{title}");
                assert!(!title.contains('?'), "{title}");
                assert!(!title.contains("watch"), "{title}");
                assert_eq!(title, "cdn.embedly.com");
            }
            other => panic!("{other:?}"),
        }
        let short = embed(r#"<iframe src="https://b23.tv/BV1xx411c7mD"></iframe>"#);
        assert!(matches!(
            short,
            Block::Embed {
                kind: EmbedKind::Iframe,
                ..
            }
        ));
    }

    #[test]
    fn source_child_is_direct_and_playlist_containers_are_not() {
        let mp4 = embed(
            r#"<video controls><source src="https://cdn.example/a.mp4" type="video/mp4"></video>"#,
        );
        assert!(matches!(
            mp4,
            Block::Embed { kind: EmbedKind::Video, ref url, .. } if url == "https://cdn.example/a.mp4"
        ));
        let audio = embed(r#"<audio src="https://cdn.example/talk.mp3"></audio>"#);
        assert!(matches!(
            audio,
            Block::Embed {
                kind: EmbedKind::Audio,
                ..
            }
        ));
        let m3u8 = embed(r#"<video src="https://cdn.example/live.m3u8"></video>"#);
        match m3u8 {
            Block::Embed { kind, title, url } => {
                assert_eq!(kind, EmbedKind::Iframe, "m3u8 must not be a local player");
                assert!(!url.contains("m3u8") || kind != EmbedKind::Video);
                assert!(
                    !title.contains('?') && !title.contains("live.m3u8"),
                    "{title}"
                );
            }
            other => panic!("{other:?}"),
        }
        let mkv = embed(r#"<video src="https://cdn.example/film.mkv"></video>"#);
        assert!(matches!(
            mkv,
            Block::Embed {
                kind: EmbedKind::Iframe,
                ..
            }
        ));
        let avi =
            embed(r#"<video><source src="https://cdn.example/film.avi" type="video/avi"></video>"#);
        assert!(matches!(
            avi,
            Block::Embed {
                kind: EmbedKind::Iframe,
                ..
            }
        ));
    }

    #[test]
    fn stored_youtube_kind_loses_to_embedly_url() {
        let presented = present_embed(
            EmbedKind::Youtube,
            "https://cdn.embedly.com/widgets/media.html?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3Dabc",
            "YouTube（在浏览器打开）",
        );
        assert_eq!(presented.kind, EmbedKind::Iframe);
        assert_eq!(presented.title, "cdn.embedly.com");
        assert!(!presented.title.contains("youtube"));
        assert!(!presented.url.contains("watch?v="));
    }

    #[test]
    fn audio_fallback_is_removed_and_player_stays_in_place() {
        let doc = html_to_document(
            r#"<p>Before</p><p><audio controls style="width: 100%"><source src="https://static.example/clip.wav" type="audio/wav">Your browser does not support the audio element.</audio></p><p>After</p>"#,
            &RenderOpts::default(),
        );
        let labels: Vec<String> = doc
            .blocks
            .iter()
            .map(|block| match block {
                Block::Paragraph { inlines } => crate::inlines_to_plain(inlines),
                Block::Embed { kind, .. } => format!("{kind:?}"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                "Before".to_string(),
                "Audio".to_string(),
                "After".to_string()
            ]
        );
        let plain = crate::document_to_plain(&doc);
        assert!(!plain.contains("does not support"));
    }
}
