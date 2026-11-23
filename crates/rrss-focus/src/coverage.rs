//! 阶段 23「B1 基础层 · 多源报道」：已有合辑的不同源数，纯函数。

use std::collections::BTreeSet;

pub fn score(feeds: &[i64]) -> (usize, f64) {
    let count = feeds.iter().collect::<BTreeSet<_>>().len();
    (count, (count.max(1) as f64).ln())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_sources_not_articles() {
        assert_eq!(score(&[]), (0, 0.0));
        assert_eq!(score(&[1, 1]), (1, 0.0));
        assert_eq!(score(&[1, 2, 3, 3]), (3, 3_f64.ln()));
    }
}
