//! Text written into cached documents by older versions. Only matched, never produced.

/// Older versions stored a "open in browser" placeholder as the embed title.
pub(crate) fn placeholder_embed_title(title: &str) -> bool {
    title.contains("在浏览器打开")
}
