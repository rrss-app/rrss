use super::*;
use crate::translate::{TranslationPatch, TranslationPlan};
use crate::{Block, Inline};

fn texts(segments: &[TaggedSegment]) -> Vec<&str> {
    segments.iter().map(|s| s.text.as_str()).collect()
}

/// 模拟模型：保留占位，给可见文字加前缀。
fn fake_translate(segment: &str) -> String {
    format!("T:{segment}")
}

fn roundtrip(segments: &[TaggedSegment], lang: &str) -> String {
    let restored: Vec<String> = segments
        .iter()
        .map(|s| restore_segment_tags(&fake_translate(&s.text), &s.tag_map))
        .collect();
    let spaces: Vec<bool> = segments.iter().map(|s| s.trailing_space).collect();
    join_translated_segments(&restored, &spaces, lang)
}

#[test]
fn short_units_and_zero_limit_stay_whole() {
    let text = "<x0>Short</x0> text.";
    for max in [0, 100] {
        let segments = split_tagged_unit(text, max);
        assert_eq!(texts(&segments), [text]);
        assert_eq!(segments[0].tag_map, [0]);
        assert!(!segments[0].trailing_space);
    }
}

#[test]
fn never_cuts_inside_a_placeholder_pair() {
    let text = "<x0>Sentence one. Sentence two.</x0> Tail sentence here. More text follows.";
    for max in 10..text.len() {
        let segments = split_tagged_unit(text, max);
        for segment in &segments {
            assert!(tokenize(&segment.text).is_some(), "{max}: {}", segment.text);
            assert!(!segment.text.starts_with("Sentence two"), "{max}");
        }
        assert!(
            segments[0]
                .text
                .starts_with("<x0>Sentence one. Sentence two.</x0>")
        );
    }
    let segments = split_tagged_unit(text, 60);
    assert_eq!(
        texts(&segments),
        [
            "<x0>Sentence one. Sentence two.</x0> Tail sentence here.",
            "More text follows."
        ]
    );
    assert!(segments[0].trailing_space && !segments[1].trailing_space);
}

#[test]
fn decimals_versions_domains_and_abbreviations_are_not_sentence_ends() {
    let first =
        "Rust 1.90 shipped and e.g. Dr. Smith of example.com and the U.S. office tried v1.2 today.";
    let text = format!("{first} The second sentence follows here.");
    let segments = split_tagged_unit(&text, first.chars().count() + 5);
    assert_eq!(
        texts(&segments),
        [first, "The second sentence follows here."]
    );
    // 上限内没有句末时退到分句，再退到空白，都不会切在 `1.` / `e.g.` / `example.`。
    let segments = split_tagged_unit(first, 40);
    assert!(segments.len() >= 2);
    for segment in &segments {
        let text = segment.text.as_str();
        assert!(
            !text.ends_with("1.") && !text.ends_with("e.g.") && !text.ends_with("example."),
            "{text}"
        );
        assert!(
            !text.starts_with(".90") && !text.starts_with("com"),
            "{text}"
        );
    }
}

#[test]
fn sentence_punctuation_wins_over_clauses_and_spaces() {
    let text = "Alpha beta, gamma delta. Epsilon zeta, eta theta iota kappa lambda.";
    let segments = split_tagged_unit(text, 50);
    assert_eq!(
        texts(&segments),
        [
            "Alpha beta, gamma delta.",
            "Epsilon zeta, eta theta iota kappa lambda."
        ]
    );
    // 没有句末就切在分句标点后。
    let text = "Alpha beta gamma delta, epsilon zeta eta theta; iota kappa lambda mu";
    let segments = split_tagged_unit(text, 50);
    assert_eq!(
        texts(&segments),
        [
            "Alpha beta gamma delta, epsilon zeta eta theta;",
            "iota kappa lambda mu"
        ]
    );
}

#[test]
fn cjk_and_english_sentences_split_and_join_by_target_language() {
    let zh = "第一句话说得比较长。“第二句话也很长！”第三句话问什么？最后一句。";
    let segments = split_tagged_unit(zh, 12);
    assert_eq!(
        texts(&segments),
        [
            "第一句话说得比较长。",
            "“第二句话也很长！”",
            "第三句话问什么？",
            "最后一句。"
        ]
    );
    assert!(segments.iter().all(|s| !s.trailing_space));
    assert_eq!(roundtrip(&segments, "zh-CN").matches(' ').count(), 0);
    let english: Vec<String> = ["First.", "\"Second!\"", "Third?", "Last."]
        .map(String::from)
        .to_vec();
    assert_eq!(
        join_translated_segments(&english, &[false; 4], "en"),
        "First. \"Second!\" Third? Last."
    );

    let en = "One sentence here. Two sentence here! Three sentence here? Four.";
    let segments = split_tagged_unit(en, 20);
    assert_eq!(
        texts(&segments),
        [
            "One sentence here.",
            "Two sentence here!",
            "Three sentence here?",
            "Four."
        ]
    );
    assert!(segments[..3].iter().all(|s| s.trailing_space));
    assert_eq!(
        roundtrip(&segments, "en"),
        "T:One sentence here. T:Two sentence here! T:Three sentence here? T:Four."
    );
    let chinese: Vec<String> = ["第一句。", "第二句！", "第三句？", "第四句。"]
        .map(String::from)
        .to_vec();
    let spaces: Vec<bool> = segments.iter().map(|s| s.trailing_space).collect();
    assert_eq!(
        join_translated_segments(&chinese, &spaces, "zh"),
        "第一句。第二句！第三句？第四句。"
    );
    // 中文译文里夹英文词时，原文有空白的切点仍留空格。
    let mixed: Vec<String> = ["使用", "Rust 编写"].map(String::from).to_vec();
    assert_eq!(
        join_translated_segments(&mixed, &[true, false], "ja"),
        "使用 Rust 编写"
    );
}

#[test]
fn many_links_split_restore_original_ids_and_patch_back() {
    let mut md = String::new();
    for i in 0..13 {
        if i == 4 {
            md.push_str(&format!(
                "**Bold [link {i}](https://a.example/{i}) with `code{i}`.** "
            ));
        } else {
            md.push_str(&format!(
                "Sentence {i} points to [link {i}](https://a.example/{i}) and keeps going. "
            ));
        }
    }
    let doc = crate::markdown_to_document(&md);
    let plan = TranslationPlan::new("", &doc);
    let unit = &plan.units[0].t;
    assert!(unit.contains("<x12>") || unit.contains("<x14>"), "{unit}");
    let segments = split_tagged_unit(unit, 160);
    assert!(segments.len() >= 4, "{}", segments.len());
    assert!(segments.len() <= MAX_UNIT_SEGMENTS);
    for segment in &segments {
        assert!(segment.text.chars().count() <= 160, "{}", segment.text);
        // 每片从 x0 起编，局部编号连续。
        assert!(segment.text.contains("<x0>"), "{}", segment.text);
        assert!(
            !segment
                .text
                .contains(&format!("<x{}", segment.tag_map.len()))
        );
        assert!(segment.accepts(&fake_translate(&segment.text)));
    }
    let joined = roundtrip(&segments, "en");
    let Ok(TranslationPatch::Block { index: 0, block }) = plan.patch(&doc, 0, &joined) else {
        panic!("joined segments must patch back: {joined}");
    };
    let Block::Paragraph { inlines } = block else {
        panic!("paragraph");
    };
    let mut hrefs = Vec::new();
    let mut codes = 0;
    fn walk(inlines: &[Inline], hrefs: &mut Vec<String>, codes: &mut usize) {
        for inline in inlines {
            match inline {
                Inline::Link { href, children } => {
                    hrefs.push(href.clone());
                    walk(children, hrefs, codes);
                }
                Inline::Strong { children } | Inline::Emphasis { children } => {
                    walk(children, hrefs, codes)
                }
                Inline::Code { .. } => *codes += 1,
                _ => {}
            }
        }
    }
    walk(&inlines, &mut hrefs, &mut codes);
    let expected: Vec<String> = (0..13).map(|i| format!("https://a.example/{i}")).collect();
    assert_eq!(hrefs, expected);
    assert_eq!(codes, 1);
}

#[test]
fn restore_replaces_ids_in_one_pass_and_keeps_unknown_ones() {
    assert_eq!(
        restore_segment_tags("<x0>a</x0><x1>b</x1><x2/>", &[1, 2, 3]),
        "<x1>a</x1><x2>b</x2><x3/>"
    );
    assert_eq!(
        restore_segment_tags("< x1 >b</x1> <x0/> <x7>?</x7> a < b", &[10, 11]),
        "<x11>b</x11> <x10/> <x7>?</x7> a < b"
    );
}

#[test]
fn atomic_spans_and_unbreakable_text_never_loop() {
    let wrapped = format!("<x0>{}</x0>", "Long words inside one link. ".repeat(20));
    assert_eq!(split_tagged_unit(&wrapped, 50).len(), 1);
    let solid = "a".repeat(500);
    assert_eq!(texts(&split_tagged_unit(&solid, 50)), [solid.as_str()]);
    let malformed = format!("{} a < b {}", "word ".repeat(40), "word ".repeat(40));
    assert_eq!(
        texts(&split_tagged_unit(&malformed, 50)),
        [malformed.as_str()]
    );
    let unclosed = format!("<x0>{}", "word ".repeat(40));
    assert_eq!(split_tagged_unit(&unclosed, 50).len(), 1);
    // 中间的原子块超出上限时推进到它之后的第一个切点。
    let text = format!(
        "Intro sentence. <x0>{}</x0> after link. Tail sentence.",
        "inner words ".repeat(10).trim_end()
    );
    let segments = split_tagged_unit(&text, 30);
    assert_eq!(segments[0].text, "Intro sentence.");
    assert!(segments[1].text.starts_with("<x0>inner") && segments[1].text.contains("</x0>"));
    assert_eq!(segments.last().unwrap().text, "after link. Tail sentence.");
}

#[test]
fn segment_count_is_bounded() {
    let text = "word ".repeat(5000);
    let segments = split_tagged_unit(&text, 10);
    assert!(segments.len() > 1 && segments.len() <= MAX_UNIT_SEGMENTS);
    let joined: String = texts(&segments).join(" ");
    assert_eq!(joined.trim_end(), text.trim_end());
}

#[test]
fn accepts_requires_every_placeholder_once_with_the_same_kind() {
    let segment = &split_tagged_unit("<x3>Bold</x3> and <x4/> code. Second sentence here.", 32)[0];
    assert_eq!(segment.text, "<x0>Bold</x0> and <x1/> code.");
    assert_eq!(segment.tag_map, [3, 4]);
    assert!(segment.accepts("<x1/> 代码和<x0>粗体</x0>。"));
    assert!(!segment.accepts("<x0>粗体</x0>和代码。"));
    assert!(!segment.accepts("<x0>粗体</x0><x0>重复</x0><x1/>"));
    assert!(!segment.accepts("<x0/><x1>错型</x1>"));
    assert!(!segment.accepts("<x0>交错<x1/>"));
    assert!(!segment.accepts("<x0></x0><x1/> "));
}

#[test]
fn joinable_rejects_unclosed_tags_and_tag_only_outputs() {
    let segment = &split_tagged_unit("<x0>Link</x0> and <x1/> code. Second sentence here.", 32)[0];
    // 缺了一个占位但标签闭合、有可见文字：下限不可再拆时允许安全拼回给整单元宽松兜底。
    assert!(segment.joinable("<x0>链接</x0>和代码。"));
    assert!(segment.joinable("< x0 >链接</x0> a < b"));
    // 未闭合或错配的成对标签会跨片吞并后续段落，必须拒绝。
    assert!(!segment.joinable("<x0>链接没闭合"));
    assert!(!segment.joinable("多余闭合</x0>"));
    assert!(!segment.joinable("<x0><x1>交错</x0></x1>"));
    // 没有可见文字的纯标签输出必须拒绝。
    assert!(!segment.joinable("<x0></x0><x1/> "));
}

#[test]
fn entity_semicolons_and_non_ascii_initials_are_not_false_breaks() {
    // `&lt;` 和 `&amp;` 末尾的分号不能抢走真正逗号的分句优先级。
    let text = "Compare alpha, when x &lt; y and R &amp; D continues onward";
    let segments = split_tagged_unit(text, 45);
    assert_eq!(
        texts(&segments),
        ["Compare alpha,", "when x &lt; y and R &amp; D continues onward"]
    );
    // 非 ASCII 单字母首字母缩写（如 `É. Smith`）不当句末切点。
    let french = "Author É. Smith wrote a long note about parsers. Next sentence follows.";
    let segments = split_tagged_unit(french, 52);
    assert_eq!(
        texts(&segments),
        [
            "Author É. Smith wrote a long note about parsers.",
            "Next sentence follows."
        ]
    );
}
