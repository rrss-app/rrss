use super::nav::strip_navigation;
use super::*;
use judge::{Suspicion::*, judge, not_article};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../testdata/web/{name}.html",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn extract(name: &str) -> Extracted {
    extract_article(
        &fixture(name),
        &Url::parse("https://example.test/news/story").unwrap(),
        None,
    )
    .unwrap()
}

#[test]
fn english_and_chinese_articles() {
    for name in ["english", "chinese"] {
        let a = extract(name);
        assert_eq!(a.layer, Layer::General, "{name}");
        assert!(a.suspicious.is_empty(), "{name}: {:?}", a.suspicious);
        assert!(a.text_chars >= 200);
        assert_eq!(
            a.text_chars,
            rrss_render::document_to_plain(&a.document)
                .trim()
                .chars()
                .count()
        );
    }
    let a = extract("english");
    assert_eq!(a.byline.as_deref(), Some("Ada Gardener"));
    assert_eq!(a.published_time.as_deref(), Some("2026-10-20"));
}

#[test]
fn container_rescues_text_discarded_by_both_libraries() {
    let a = extract("container");
    assert_eq!(a.layer, Layer::Container);
    assert!(a.suspicious.is_empty());
    assert!(
        rrss_render::document_to_plain(&a.document)
            .contains("Supplementary laboratory observations")
    );
    let siblings = extract("siblings");
    assert_eq!(siblings.layer, Layer::General);
    assert!(siblings.suspicious.is_empty());
}

#[test]
fn lazy_images_and_base() {
    let a = extract("lazy");
    let images: Vec<_> = a
        .document
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Image { src, .. } => Some(src.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        images,
        [
            "https://example.test/assets/notes/first.jpg",
            "https://example.test/assets/second.png"
        ]
    );
    assert_eq!(a.image_count, 2);
}

#[test]
fn fallback_resolves_lazy_images_against_base_too() {
    let dom = Dom::from(fixture("lazy"));
    normalize_images(&dom);
    let page = Page {
        base: Url::parse("https://example.test/assets/notes/").unwrap(),
        title: String::new(),
        article_chars: 0,
        metadata: RuleMetadata::default(),
        stripped_elements: 0,
    };
    let a = page.fallback(&dom.html()).unwrap();
    assert_eq!(a.layer, Layer::Fallback);
    assert!(a.suspicious.is_empty());
    let json = rrss_render::document_to_json(&a.document).unwrap();
    assert!(json.contains("https://example.test/assets/notes/first.jpg"));
    assert!(json.contains("https://example.test/assets/second.png"));
    assert_eq!(a.image_count, 2);
}

#[test]
fn sanitized_output() {
    let a = extract("unsafe");
    let json = rrss_render::document_to_json(&a.document).unwrap();
    for forbidden in ["evil_marker", "javascript:", "onclick", "<script"] {
        assert!(!json.contains(forbidden), "{forbidden}");
    }
    assert!(json.contains("safe label"));
}

#[test]
fn listing_is_not_article() {
    assert_eq!(
        extract_article(
            &fixture("listing"),
            &Url::parse("https://example.test/").unwrap(),
            None
        )
        .unwrap_err(),
        ExtractError::NotArticle
    );
    assert!(!not_article(false, 100, 100, 20, 20));
    assert!(!not_article(true, 1000, 200, 20, 0));
    assert!(not_article(true, 1000, 200, 20, 18));
}

#[test]
fn short_text() {
    assert_eq!(judge(199, 3, 0, "Title"), [ShortText]);
}
#[test]
fn chinese_short_text_is_not_inflated_by_utf8_bytes() {
    let page = Page {
        base: Url::parse("https://example.test/").unwrap(),
        title: "短文".into(),
        article_chars: 0,
        metadata: RuleMetadata::default(),
        stripped_elements: 0,
    };
    let a = page.candidate(
        &format!("<p>{}</p><p>二</p><p>三</p>", "中".repeat(100)),
        String::new(),
        None,
        None,
        Layer::Container,
    );
    assert_eq!(a.suspicious, [ShortText]);
    assert!(a.text_chars < 200);
}

#[test]
fn few_blocks() {
    assert_eq!(judge(200, 2, 0, "Title"), [FewBlocks]);
}
#[test]
fn article_ratio() {
    assert_eq!(judge(200, 3, 401, "Title"), [LowArticleRatio]);
}
#[test]
fn missing_title() {
    assert_eq!(judge(200, 3, 0, " "), [MissingTitle]);
}
#[test]
fn all_reasons_and_boundaries() {
    assert_eq!(
        judge(100, 1, 400, ""),
        [ShortText, FewBlocks, LowArticleRatio, MissingTitle]
    );
    assert!(judge(200, 3, 400, "Title").is_empty());
}

#[test]
fn malformed_and_empty_inputs() {
    let url = Url::parse("https://example.test/").unwrap();
    for html in [
        "",
        "<script>hidden()</script>",
        "<title>Only a title</title>",
    ] {
        assert_eq!(
            extract_article(html, &url, None).unwrap_err(),
            ExtractError::Empty
        );
    }
    let a = extract_article("<main><p>残缺的正文<p>第二段", &url, None).unwrap();
    assert!(a.suspicious.contains(&MissingTitle));
    assert!(rrss_render::document_to_plain(&a.document).contains("残缺的正文"));
}

#[test]
fn library_metadata_survives_container_rescue() {
    let page = Page {
        base: Url::parse("https://example.test/").unwrap(),
        title: String::new(),
        article_chars: 0,
        metadata: RuleMetadata::default(),
        stripped_elements: 0,
    };
    let primary = page.candidate(
        "<p>Short</p>",
        "Recognized title".into(),
        Some("Author".into()),
        Some("2026".into()),
        Layer::General,
    );
    let rescue = page.candidate(
        "<p>A longer rescue paragraph.</p>",
        String::new(),
        None,
        None,
        Layer::Container,
    );
    let a = choose([Some(primary), Some(rescue)]).unwrap();
    assert_eq!(a.layer, Layer::Container);
    assert_eq!(a.title, "Recognized title");
    assert_eq!(a.byline.as_deref(), Some("Author"));
    assert_eq!(a.published_time.as_deref(), Some("2026"));
    assert!(!a.suspicious.contains(&MissingTitle));
}

#[test]
fn cascade_selection_is_lazy_and_retains_longest_suspicious() {
    let page = Page {
        base: Url::parse("https://example.test/").unwrap(),
        title: "Title".into(),
        article_chars: 0,
        metadata: RuleMetadata::default(),
        stripped_elements: 0,
    };
    let candidate = |text: &str, layer| page.candidate(text, String::new(), None, None, layer);
    let good = fixture("english");
    let mut calls = 0;
    let a = choose((0..3).map(|i| {
        calls += 1;
        Some(if i == 0 {
            candidate("<p>Short</p>", Layer::General)
        } else {
            candidate(&good, Layer::Fallback)
        })
    }))
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(a.layer, Layer::Fallback);
    let a = choose([
        Some(candidate("<p>Small</p>", Layer::General)),
        None,
        Some(candidate(
            "<p>A longer but still suspicious rescue.</p>",
            Layer::Container,
        )),
    ])
    .unwrap();
    assert_eq!(a.layer, Layer::Container);
    assert!(!a.suspicious.is_empty());
}

#[test]
fn recognized_metadata_precedes_page_title() {
    let a = extract("metadata");
    assert_eq!(a.title, "Watching an orchard through the seasons");
    assert_eq!(a.byline.as_deref(), Some("Ada Gardener"));
    assert_eq!(a.published_time.as_deref(), Some("2026-10-20"));
    let dom = Dom::from(fixture("metadata"));
    let page = Page {
        base: Url::parse("https://example.test/").unwrap(),
        title: "Example publication archive".into(),
        article_chars: 0,
        metadata: RuleMetadata::default(),
        stripped_elements: 0,
    };
    let fallback = page.fallback(&dom.html()).unwrap();
    assert_eq!(fallback.title, a.title);
    assert_eq!(fallback.byline, a.byline);
    assert_eq!(fallback.published_time, a.published_time);
    let candidate = page.candidate("<p>Text</p>", String::new(), None, None, Layer::General);
    assert_eq!(candidate.title, page.title);
}

#[test]
fn chinese_article_with_recommendations_is_not_a_listing() {
    let a = extract("chinese-sidebar");
    assert_eq!(a.layer, Layer::General);
    assert!(a.suspicious.is_empty());
    let plain = rrss_render::document_to_plain(&a.document);
    assert!(plain.contains("调查持续了数周"));
    assert!(!plain.contains("社区观察专题"));
}

#[test]
fn good_primary_article_does_not_pull_in_fallback_comments() {
    let a = extract("comments");
    assert_eq!(a.layer, Layer::General);
    assert!(a.suspicious.is_empty());
    assert!(!rrss_render::document_to_plain(&a.document).contains("COMMENT_SENTINEL"));
    let page = Page {
        base: Url::parse("https://example.test/").unwrap(),
        title: String::new(),
        article_chars: 0,
        metadata: RuleMetadata::default(),
        stripped_elements: 0,
    };
    let fallback = page.fallback(&fixture("comments")).unwrap();
    assert!(rrss_render::document_to_plain(&fallback.document).contains("COMMENT_SENTINEL"));
}

fn plain(extracted: &Extracted) -> String {
    rrss_render::document_to_plain(&extracted.document)
}

#[test]
fn hover_navigation_stays_out_of_english_and_chinese_articles() {
    let english = extract("nav-english");
    let english_text = plain(&english);
    assert!(english.suspicious.is_empty(), "{:?}", english.suspicious);
    assert!(english_text.contains("An orchard is a small ecosystem"));
    assert!(english_text.contains("birds returned to nest"));
    for sentinel in [
        "NAV_SENTINEL_BANNER",
        "NAV_SENTINEL_MENUBAR",
        "NAV_SENTINEL_MENU",
        "NAV_SENTINEL_ROLE",
        "NAV_SENTINEL_ROLE_DESK",
        "NAV_SENTINEL_HOME",
        "NAV_SENTINEL_DESK",
        "NAV_SENTINEL_SEASONS",
        "NAV_SENTINEL_SOIL",
        "NAV_SENTINEL_BIRDS",
        "NAV_SENTINEL_WATER",
        "FOOTER_NAV_SENTINEL",
    ] {
        assert!(!english_text.contains(sentinel), "{sentinel}");
    }
    let english_dom = Dom::from(fixture("nav-english"));
    assert!(strip_navigation(&english_dom) >= 1);
    let english_html = english_dom.html().to_string();
    assert!(!english_html.contains("NAV_SENTINEL_HOME"));
    assert!(!english_html.contains("NAV_SENTINEL_ROLE"));
    assert!(!english_html.contains("FOOTER_NAV_SENTINEL"));
    assert!(english_html.contains("An orchard is a small ecosystem"));
    assert!(english_html.contains("ASIDE_SENTINEL"));
    assert!(english_html.contains("FOOTER_SENTINEL"));
    assert!(english_html.contains("CLASS_NAV_SENTINEL"));

    let chinese = extract("nav-chinese");
    let chinese_text = plain(&chinese);
    assert!(chinese.suspicious.is_empty(), "{:?}", chinese.suspicious);
    assert!(chinese_text.contains("调查持续了数周"));
    assert!(chinese_text.contains("保留部分落叶"));
    for sentinel in [
        "导航哨兵首页",
        "导航哨兵甲",
        "导航哨兵乙",
        "导航哨兵植物",
        "导航哨兵昆虫",
        "导航哨兵树木",
        "导航哨兵步道",
    ] {
        assert!(!chinese_text.contains(sentinel), "{sentinel}");
    }
    let chinese_dom = Dom::from(fixture("nav-chinese"));
    assert!(strip_navigation(&chinese_dom) >= 1);
    let chinese_html = chinese_dom.html().to_string();
    assert!(!chinese_html.contains("导航哨兵首页"));
    assert!(!chinese_html.contains("导航哨兵甲"));
    assert!(chinese_html.contains("调查持续了数周"));
}

#[test]
fn article_further_reading_links_stay() {
    let extracted = extract("further-reading");
    let text = plain(&extracted);
    assert!(
        extracted.suspicious.is_empty(),
        "{:?}",
        extracted.suspicious
    );
    assert!(text.contains("An orchard is a small ecosystem"));
    assert!(text.contains("Field notes kept beside the south slope"));
    for title in [
        "Further shade along the rows",
        "Further rain on the hillside",
        "Further native flowers",
        "Further winter ground cover",
        "Further birds at the boundary",
        "Margin note alpha",
        "Margin note alpha detail",
        "Margin note epsilon",
    ] {
        assert!(text.contains(title), "{title}");
    }
    let dom = Dom::from(fixture("further-reading"));
    assert_eq!(strip_navigation(&dom), 0);
    let html = dom.html().to_string();
    assert!(html.contains("Further shade along the rows"));
    assert!(html.contains("Margin note alpha detail"));
    assert!(html.contains("Field notes kept beside the south slope"));
    assert!(html.contains("Class name menu must stay in the orchard report."));
}

#[test]
fn navigation_thresholds_keep_prose_and_flat_lists() {
    let html = r#"
        <header><a href="/a">AAAAAAAAAAAA</a>BBBBBBBB</header>
        <header><a href="/a">AAA</a>HEADER_PROSE_SENTINEL_BBBBBBBBBBBBBBBBBBBB</header>
        <header>
          <p>HEADER_MENU_PROSE is a long title that is not a link and would fail the sixty percent test on its own.</p>
          <ul>
            <li><a href="/a">HEADER_MENU_A</a><ul><li><a href="/a1">HEADER_MENU_SUB</a></li></ul></li>
            <li><a href="/b">HEADER_MENU_B</a></li>
            <li><a href="/c">HEADER_MENU_C</a></li>
            <li><a href="/d">HEADER_MENU_D</a></li>
            <li><a href="/e">HEADER_MENU_E</a></li>
          </ul>
        </header>
        <ul>
          <li><a href="/a">MENU80_A</a><ul><li><a href="/a1">MENU80_SUB</a></li></ul></li>
          <li><a href="/b">MENU80_B</a></li>
          <li><a href="/c">MENU80_C</a></li>
          <li><a href="/d">MENU80_D</a></li>
          <li>MENU80_PROSE words without any anchor</li>
        </ul>
        <ul>
          <li><a href="/a">BELOW80_A</a><ul><li><a href="/a1">BELOW80_SUB</a></li></ul></li>
          <li><a href="/b">BELOW80_B</a></li>
          <li><a href="/c">BELOW80_C</a></li>
          <li>BELOW80_PROSE one</li>
          <li>BELOW80_PROSE two</li>
        </ul>
        <ul>
          <li><a href="/a">FOUR_A</a><ul><li><a href="/a1">FOUR_SUB</a></li></ul></li>
          <li><a href="/b">FOUR_B</a></li>
          <li><a href="/c">FOUR_C</a></li>
        </ul>
        <ul>
          <li><a href="/1">FLAT_LIST_SENTINEL_1</a></li>
          <li><a href="/2">FLAT_LIST_SENTINEL_2</a></li>
          <li><a href="/3">FLAT_LIST_SENTINEL_3</a></li>
          <li><a href="/4">FLAT_LIST_SENTINEL_4</a></li>
          <li><a href="/5">FLAT_LIST_SENTINEL_5</a></li>
          <li><a href="/6">FLAT_LIST_SENTINEL_6</a></li>
          <li><a href="/7">FLAT_LIST_SENTINEL_7</a></li>
          <li><a href="/8">FLAT_LIST_SENTINEL_8</a></li>
        </ul>
        <article><header><a href="/t">ARTICLE_HEADER_LINK_SENTINEL</a></header><p>Article body.</p></article>
    "#;
    let dom = Dom::from(html);
    assert!(strip_navigation(&dom) >= 1);
    let cleaned = dom.html().to_string();
    assert!(!cleaned.contains("AAAAAAAAAAAA"), "{cleaned}");
    assert!(cleaned.contains("HEADER_PROSE_SENTINEL"));
    assert!(!cleaned.contains("HEADER_MENU_PROSE"), "{cleaned}");
    assert!(!cleaned.contains("HEADER_MENU_A"));
    assert!(!cleaned.contains("MENU80_A"));
    assert!(!cleaned.contains("MENU80_PROSE"));
    assert!(cleaned.contains("BELOW80_PROSE"));
    assert!(cleaned.contains("FOUR_A"));
    assert!(cleaned.contains("FLAT_LIST_SENTINEL_8"));
    assert!(cleaned.contains("ARTICLE_HEADER_LINK_SENTINEL"));
}
