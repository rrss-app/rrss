//! Character counts, never UTF-8 byte lengths. Conservative defaults for short news too.
pub(super) const MIN_CHARS: usize = 200;
pub(super) const MIN_BLOCKS: usize = 3;
pub(super) const MIN_ARTICLE_RATIO: f64 = 0.5;
pub(super) const MAX_LINK_DENSITY: f64 = 0.65;
pub(super) const MIN_LINKED_ITEMS: usize = 12;
pub(super) const MIN_LINKED_ITEM_RATIO: f64 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspicion {
    ShortText,
    FewBlocks,
    LowArticleRatio,
    MissingTitle,
}

pub(super) fn judge(
    chars: usize,
    blocks: usize,
    article_chars: usize,
    title: &str,
) -> Vec<Suspicion> {
    let mut reasons = Vec::new();
    if chars < MIN_CHARS {
        reasons.push(Suspicion::ShortText);
    }
    if blocks < MIN_BLOCKS {
        reasons.push(Suspicion::FewBlocks);
    }
    if article_chars > 0 && (chars as f64 / article_chars as f64) < MIN_ARTICLE_RATIO {
        reasons.push(Suspicion::LowArticleRatio);
    }
    if title.trim().is_empty() {
        reasons.push(Suspicion::MissingTitle);
    }
    reasons
}

pub(super) fn not_article(
    suspicious: bool,
    body_chars: usize,
    link_chars: usize,
    items: usize,
    linked_items: usize,
) -> bool {
    suspicious
        && ((body_chars > 0 && link_chars as f64 / body_chars as f64 > MAX_LINK_DENSITY)
            || (linked_items >= MIN_LINKED_ITEMS
                && linked_items as f64 / items as f64 >= MIN_LINKED_ITEM_RATIO))
}
