use rrss_render::{
    Block, Document, Inline, RenderOpts, TranslateUnit, TranslationPlan, apply_units,
    document_from_json, document_to_json, document_to_plain, html_to_document,
    markdown_to_document,
};

#[test]
fn markdown_alignment_and_inline_formats() {
    let doc = markdown_to_document(
        "| **列1** | *列2* | `列3` | |\n| :--- | :---: | ---: | --- |\n| [链接](https://example.test/) | a | b | c |",
    );
    assert_eq!(
        serde_json::to_value(&doc.blocks[0]).unwrap(),
        serde_json::json!({
            "t": "table", "align": ["l", "c", "r", ""],
            "head": [[{"t":"s", "children":[{"t":"t", "text":"列1"}]}],
                     [{"t":"e", "children":[{"t":"t", "text":"列2"}]}],
                     [{"t":"c", "text":"列3"}], []],
            "rows": [[[{"t":"a", "href":"https://example.test/", "children":[{"t":"t", "text":"链接"}]}],
                      [{"t":"t", "text":"a"}], [{"t":"t", "text":"b"}], [{"t":"t", "text":"c"}]]]
        })
    );
    assert_eq!(doc.blocks.len(), 1);
}

#[test]
fn markdown_ragged_rows_follow_header_width() {
    let doc = markdown_to_document(
        "| A | B |\n| --- | --- |\n| one |\n| two | three | discarded |\n| | |",
    );
    assert_eq!(document_to_plain(&doc), "A\tB\none\t\ntwo\tthree\n\t");
}

#[test]
fn markdown_limits_report_only_exceeded_dimensions() {
    for (rows, cols, note) in [
        (200, 16, None),
        (201, 2, Some("200 行")),
        (2, 17, Some("16 列")),
        (201, 17, Some("200 行 16 列")),
    ] {
        let row = format!("|{}\n", " x |".repeat(cols));
        let md = format!("{row}|{}\n{}", " --- |".repeat(cols), row.repeat(rows));
        let doc = markdown_to_document(&md);
        let Block::Table {
            align,
            head,
            rows: body,
        } = &doc.blocks[0]
        else {
            panic!("table");
        };
        assert_eq!(head.len(), cols.min(16));
        assert_eq!(align.len(), head.len());
        assert_eq!(body.len(), rows.min(200));
        assert!(body.iter().all(|r| r.len() == head.len()));
        if let Some(note) = note {
            assert_eq!(doc.blocks.len(), 2);
            assert_eq!(
                rrss_render::block_plain(&doc.blocks[1]),
                format!("（表格共 {rows} 行 {cols} 列，只显示前 {note}）")
            );
        } else {
            assert_eq!(doc.blocks.len(), 1);
        }
    }
}

#[test]
fn markdown_table_keeps_surrounding_paragraphs_separate() {
    let doc = markdown_to_document("之前\n\n| H |\n| --- |\n| value |\n\n之后");
    assert!(matches!(
        doc.blocks.as_slice(),
        [
            Block::Paragraph { .. },
            Block::Table { .. },
            Block::Paragraph { .. }
        ]
    ));
    assert_eq!(document_to_plain(&doc), "之前\n\nH\nvalue\n\n之后");
}

#[test]
fn markdown_table_after_list_does_not_leak_cells() {
    let doc = markdown_to_document("- list item\n\n| H |\n| --- |\n| value |");
    assert!(matches!(
        doc.blocks.as_slice(),
        [Block::ListItem { .. }, Block::Table { .. }]
    ));
    assert_eq!(document_to_plain(&doc), "list item\n\nH\nvalue");
}

#[test]
fn markdown_cell_html_cannot_emit_outer_blocks() {
    let doc = markdown_to_document(
        "| H |\n| --- |\n| <img src=\"https://example.test/a.png\" alt=\"inside\"> tail |",
    );
    assert!(matches!(doc.blocks.as_slice(), [Block::Table { .. }]));
    assert_eq!(document_to_plain(&doc), "H\ninside tail");
}

#[test]
fn html_simple_table_preserves_sections_alignment_and_inline_content() {
    let doc = html_to_document(
        r#"<p>之前</p><table><thead><tr><th align="left"><b>H1</b></th><th align="CENTER">H2</th><th align="right">H3</th></tr></thead><tbody><tr><td><i>A</i></td><td><code>B</code></td><td><a href="https://example.test/">C</a></td></tr></tbody><tfoot><tr><td colspan="1" rowspan="1">尾</td><td></td><td></td></tr></tfoot></table><p>之后</p>"#,
        &RenderOpts::default(),
    );
    assert!(matches!(
        doc.blocks.as_slice(),
        [
            Block::Paragraph { .. },
            Block::Table { .. },
            Block::Paragraph { .. }
        ]
    ));
    let Block::Table { align, head, rows } = &doc.blocks[1] else {
        panic!("table");
    };
    assert_eq!(align, &["l", "c", "r"]);
    assert!(matches!(head[0].as_slice(), [Inline::Strong { .. }]));
    assert!(matches!(rows[0][0].as_slice(), [Inline::Emphasis { .. }]));
    assert!(matches!(rows[0][1].as_slice(), [Inline::Code { .. }]));
    assert!(matches!(rows[0][2].as_slice(), [Inline::Link { .. }]));
    assert_eq!(
        document_to_plain(&doc),
        "之前\n\nH1\tH2\tH3\nA\tB\tC\n尾\t\t\n\n之后"
    );
}

#[test]
fn html_without_th_uses_first_row_and_normalizes_width() {
    let doc = html_to_document(
        "<table><tr><td align=right><small>H1</small></td><td>H2</td></tr><tr><td>A</td></tr><tr><td>B</td><td>C</td><td>discarded</td></tr></table>",
        &RenderOpts::default(),
    );
    let Block::Table { align, .. } = &doc.blocks[0] else {
        panic!("table");
    };
    assert_eq!(align, &["r", ""]);
    assert_eq!(document_to_plain(&doc), "H1\tH2\nA\t\nB\tC");
}

#[test]
fn html_complex_tables_keep_legacy_fallback() {
    for (html, expected) in [
        (
            "<table><tr><th rowspan=2>A</th><td>B</td></tr><tr><td>C</td></tr></table>",
            "ABC",
        ),
        (
            "<table><tr><td colspan=2>A</td></tr><tr><td>B</td><td>C</td></tr></table>",
            "ABC",
        ),
        (
            "<table><tr><td><p>A</p><p>B</p></td></tr></table>",
            "A\n\nB",
        ),
        ("<table><tr><td><section>A</section></td></tr></table>", "A"),
        (
            "<table><tr><td>A<table><tr><td>B</td></tr></table>C</td></tr></table>",
            "ABC",
        ),
    ] {
        let doc = html_to_document(html, &RenderOpts::default());
        assert!(
            doc.blocks.iter().all(|b| !matches!(b, Block::Table { .. })),
            "{html}: {doc:?}"
        );
        assert_eq!(document_to_plain(&doc), expected, "{html}");
    }
}

#[test]
fn html_structure_guard_keeps_table_positions_after_rawtext_and_nesting() {
    let doc = html_to_document(
        r#"<script>const s = '<table><tr><td><section>ignored</section></td></tr></table>';</script><table><tr><td>A<table><tr><td>B</td></tr></table></td></tr></table><table><tr><td>H</td></tr><tr><td>value</td></tr></table>"#,
        &RenderOpts::default(),
    );
    assert!(matches!(
        doc.blocks.as_slice(),
        [Block::Paragraph { .. }, Block::Table { .. }]
    ));
    assert_eq!(document_to_plain(&doc), "AB\n\nH\nvalue");
}

#[test]
fn table_json_roundtrip_and_plain_text_keep_empty_cells() {
    let json = r#"{"v":1,"blocks":[{"t":"table","align":["l",""],"head":[[{"t":"t","text":"列1"}],[]],"rows":[[[],[]]]}]}"#;
    let doc = document_from_json(json).unwrap();
    assert_eq!(document_to_json(&doc).unwrap(), json);
    assert_eq!(document_to_plain(&doc), "列1\t\n\t");
}

#[test]
fn translation_skips_table_and_retains_original_between_patches() {
    let doc = markdown_to_document("before\n\n| H |\n| --- |\n| original |\n\nafter");
    let plan = TranslationPlan::new("", &doc);
    assert_eq!(plan.block_ids, [Some(0), Some(2)]);
    assert_eq!(
        plan.units.iter().map(|u| u.t.as_str()).collect::<Vec<_>>(),
        ["before", "after"]
    );
    let translated = [
        TranslateUnit {
            i: 0,
            t: "前".into(),
        },
        TranslateUnit {
            i: 1,
            t: "后".into(),
        },
    ];
    let (_, result) = apply_units("", &doc, &translated);
    let mut progressive = doc.clone();
    for unit in translated {
        let rrss_render::TranslationPatch::Block { index, block } =
            plan.patch(&doc, unit.i, &unit.t).unwrap()
        else {
            panic!("block patch");
        };
        progressive.blocks[index] = block;
    }
    assert_eq!(progressive, result);
    assert_eq!(result.blocks[1], doc.blocks[1]);
    assert_eq!(document_to_plain(&result), "前\n\nH\noriginal\n\n后");
    assert!(
        TranslationPlan::new("", &Document::new(vec![doc.blocks[1].clone()]))
            .units
            .is_empty()
    );
}
