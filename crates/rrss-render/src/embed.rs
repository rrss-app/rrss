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
    if !["<iframe", "<video", "<audio", "<embed", "<object"]
        .iter()
        .any(|tag| lower.contains(tag))
    {
        return (html.to_owned(), Vec::new());
    }
    let dom = dom_query::Document::fragment(html);
    let mut blocks = Vec::new();
    for node in dom.select("iframe, video, audio, object, embed").nodes() {
        if node
            .ancestors_it(None)
            .any(|ancestor| ancestor.is("noscript, template, iframe, video, audio"))
        {
            continue;
        }
        let hint = if node.has_name("video") {
            EmbedHint::Video
        } else if node.has_name("audio") {
            EmbedHint::Audio
        } else {
            EmbedHint::Iframe
        };
        let sources = std::iter::once(*node).chain(
            node.children_it(false)
                .filter(|child| child.has_name("source")),
        );
        let mut external = None;
        let mut playable = None;
        'sources: for source in sources {
            let primary = if source.has_name("object") {
                "data"
            } else {
                "src"
            };
            let mime = source.attr("type");
            for attr in ["data-src", "data-lazy-src", primary] {
                let Some(url) = source.attr(attr).and_then(|raw| normalize_url(&raw, base)) else {
                    continue;
                };
                let class = classify_with_hint(&url, hint, mime.as_deref());
                match class {
                    EmbedClass::External { .. } => {
                        external.get_or_insert(class);
                    }
                    EmbedClass::Direct { .. }
                        if type_media(mime.as_deref()).is_none()
                            && Url::parse(&url)
                                .ok()
                                .as_ref()
                                .and_then(direct_extension)
                                .is_none() =>
                    {
                        playable.get_or_insert(class);
                    }
                    _ => {
                        playable = Some(class);
                        break 'sources;
                    }
                }
            }
        }
        let Some(class) = playable.or(external) else {
            continue;
        };
        let title = node.attr("title").or_else(|| node.attr("aria-label"));
        let presented = present_class(&class, title.as_deref());
        node.replace_with_html(format!("<p>RRSS_EMBED_SLOT_{}</p>", blocks.len()));
        blocks.push(Block::Embed {
            kind: presented.kind,
            url: presented.url,
            title: presented.title,
        });
    }
    if blocks.is_empty() {
        return (html.to_owned(), blocks);
    }
    (dom.html_root().inner_html().to_string(), blocks)
}

pub fn attachment_embed(
    raw: &str,
    base: Option<&Url>,
    mime: Option<&str>,
) -> Option<PresentedEmbed> {
    let url = normalize_url(raw, base)?;
    let class = classify_with_hint(&url, EmbedHint::Unknown, mime);
    (!matches!(class, EmbedClass::External { .. })).then(|| present_class(&class, None))
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
    let hint = match stored {
        EmbedKind::Video => EmbedHint::Video,
        EmbedKind::Audio => EmbedHint::Audio,
        _ => EmbedHint::Unknown,
    };
    classify_with_hint(&normalized, hint, None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbedHint {
    Iframe,
    Video,
    Audio,
    Unknown,
}

fn classify_with_hint(url: &str, hint: EmbedHint, type_attr: Option<&str>) -> EmbedClass {
    let Some(parsed) = Url::parse(url).ok().filter(is_http) else {
        return external_for(url);
    };
    if let Some(class) = official(&parsed) {
        return class;
    }
    let host = parsed.host_str().unwrap_or_default();
    if [
        "youtube.com",
        "youtu.be",
        "youtube-nocookie.com",
        "bilibili.com",
        "b23.tv",
    ]
    .iter()
    .any(|domain| host_is(host, domain))
        || rejected_container(&parsed)
        || type_is_playlist(type_attr)
    {
        return external_for(parsed.as_str());
    }
    let tag_media = match hint {
        EmbedHint::Video => Some(DirectMedia::Video),
        EmbedHint::Audio => Some(DirectMedia::Audio),
        _ => None,
    };
    let declared = type_media(type_attr);
    let media = match path_ext(&parsed).as_deref() {
        Some("webm" | "mp4" | "ogg") => {
            declared.or(tag_media).or_else(|| direct_extension(&parsed))
        }
        _ => direct_extension(&parsed).or(declared).or(tag_media),
    };
    match media {
        Some(media) => EmbedClass::Direct {
            media,
            url: parsed.to_string(),
        },
        None => external_for(parsed.as_str()),
    }
}

fn present_class(class: &EmbedClass, attr_title: Option<&str>) -> PresentedEmbed {
    let (kind, url, fallback) = match class {
        // Without a usable page title the title stays empty; the UI names
        // the site or media kind in its own language.
        EmbedClass::Youtube { open_url, .. } => (EmbedKind::Youtube, open_url.clone(), ""),
        EmbedClass::Bilibili { open_url, .. } => (EmbedKind::Bilibili, open_url.clone(), ""),
        EmbedClass::Direct { media, url } => {
            let kind = match media {
                DirectMedia::Audio => EmbedKind::Audio,
                DirectMedia::Video => EmbedKind::Video,
            };
            (kind, url.clone(), "")
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
        "embed" | "shorts" | "live" | "v" => valid_yt_id(segs.next()?),
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
    if !raw.is_ascii() || raw.len() < 3 || !raw[..2].eq_ignore_ascii_case("bv") {
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
    matches!(
        path_ext(url).as_deref(),
        Some("mkv" | "avi" | "m3u" | "m3u8" | "mpd" | "pls")
    )
}

fn direct_extension(url: &Url) -> Option<DirectMedia> {
    type_media(direct_media_mime(url))
}

pub fn direct_media_mime(url: &Url) -> Option<&'static str> {
    Some(match path_ext(url)?.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "m4a" | "m4b" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        _ => return None,
    })
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
    t.contains("mpegurl") || matches!(t.as_str(), "application/dash+xml" | "audio/x-scpls")
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
        | "audio/opus" | "audio/webm" | "audio/flac" | "audio/x-flac" | "audio/x-wav" => {
            Some(DirectMedia::Audio)
        }
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
        .unwrap_or_default()
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
        || crate::legacy::placeholder_embed_title(t)
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
    fn mixed_containers_keep_the_media_kind_and_official_players_stay_external() {
        for (html, expected_kind) in [
            (
                r#"<audio src="https://cdn.example/episode.webm"></audio>"#,
                EmbedKind::Audio,
            ),
            (
                r#"<video src="https://cdn.example/movie.ogg"></video>"#,
                EmbedKind::Video,
            ),
            (
                r#"<video src="https://www.youtube.com/playlist?list=sample"></video>"#,
                EmbedKind::Iframe,
            ),
            (
                r#"<audio src="https://cdn.example/stream.mpd"></audio>"#,
                EmbedKind::Iframe,
            ),
        ] {
            let Block::Embed { kind, url, title } = embed(html) else {
                unreachable!()
            };
            assert_eq!(kind, expected_kind);
            assert_eq!(present_embed(kind, &url, &title).kind, expected_kind);
        }
    }

    #[test]
    fn wordpress_and_lazy_players_keep_order_and_skip_unusable_sources() {
        let html = r#"<figure class="wp-block-video"><video height="576" style="aspect-ratio: 720 / 576;" width="720" controls src="/clip.mp4"></video></figure>
        <audio><source src="show.m3u8"><source src="javascript:void(0)"><source data-lazy-src = "episode.flac?a=1&amp;b=2"></audio>
        <iframe src="about:blank" data-src="https://player.bilibili.com/player.html?page=1&amp;bvid=BVabc"></iframe>
        <object data="https://www.youtube.com/v/abc123"></object>
        <script>"<video src='script.mp4'></video>"</script><!-- <audio src='comment.mp3'></audio> -->"#;
        let doc = html_to_document(
            html,
            &RenderOpts {
                base_url: Some(Url::parse("https://reader.example/post/").unwrap()),
            },
        );
        let media: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Embed { kind, url, .. } => Some((*kind, url.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            media,
            [
                (EmbedKind::Video, "https://reader.example/clip.mp4"),
                (
                    EmbedKind::Audio,
                    "https://reader.example/post/episode.flac?a=1&b=2"
                ),
                (EmbedKind::Bilibili, "https://www.bilibili.com/video/BVabc/"),
                (EmbedKind::Youtube, "https://www.youtube.com/watch?v=abc123"),
            ]
        );
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
