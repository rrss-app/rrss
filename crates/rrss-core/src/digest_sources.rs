//! 简报规范成稿与来源快照版本。旧稿 v1 只做保守的槽位读取。

use std::collections::HashSet;

use pulldown_cmark::{Event, Options, Parser, Tag};
use rrss_render::{Block, Document, Inline, inlines_to_plain, markdown_to_document};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::digest::{
    DigestItem, DigestSource, count_digest_list_items, parse_digest_item_ns,
    parse_digest_items_json, sanitize_digest_body, sanitize_digest_body_with_item_refs,
};
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct DigestDraft {
    pub body: String,
    pub items_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    V1,
    V2,
    Mismatch,
    Unavailable,
}

impl SourceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "legacy",
            Self::V2 => "verified",
            Self::Mismatch => "mismatch",
            Self::Unavailable => "unavailable",
        }
    }
}

pub struct DecodedDigestSources {
    pub state: SourceState,
    pub lines: Vec<Vec<DigestSource>>,
    pub unlocated: Vec<DigestSource>,
}

#[derive(Serialize)]
struct V2Item {
    section: usize,
    refs: Vec<DigestSource>,
}

#[derive(Serialize)]
struct V2Package {
    v: u8,
    body_hash: String,
    items: Vec<V2Item>,
}

fn digest_error(message: &str) -> Error {
    Error::Parse(format!("简报成稿无效：{message}"))
}

pub fn sha256_hex(input: &[u8]) -> String {
    Sha256::digest(input)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn digest_revision(body: &str, items_json: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((body.len() as u64).to_be_bytes());
    hasher.update(body.as_bytes());
    hasher.update((items_json.len() as u64).to_be_bytes());
    hasher.update(items_json.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 规范文档只允许三级标题与顶层无序条目，每节至少一条。
/// 返回每条所属小节及纯文本；生成时再与中间对象逐条比对。
pub fn canonical_digest_items(doc: &Document) -> Option<Vec<(usize, String)>> {
    let mut section = None;
    let mut section_has_item = false;
    let mut items = Vec::new();
    for block in &doc.blocks {
        match block {
            Block::Heading { level: 3, inlines }
                if !inlines_to_plain(inlines).trim().is_empty() =>
            {
                if section.is_some() && !section_has_item {
                    return None;
                }
                section = Some(section.map_or(0, |previous: usize| previous + 1));
                section_has_item = false;
            }
            Block::ListItem {
                indent: 0,
                marker,
                inlines,
            } if section.is_some()
                && marker == "•"
                && !inlines_to_plain(inlines).trim().is_empty() =>
            {
                section_has_item = true;
                items.push((section?, inlines_to_plain(inlines)));
            }
            _ => return None,
        }
    }
    (section_has_item && !items.is_empty()).then_some(items)
}

fn safe_inlines(inlines: &[Inline]) -> bool {
    inlines.iter().all(|inline| match inline {
        Inline::Text { .. } | Inline::Code { .. } => true,
        Inline::Strong { children } | Inline::Emphasis { children } => safe_inlines(children),
        Inline::Link { href, children } => {
            matches!(
                url::Url::parse(href)
                    .ok()
                    .map(|url| url.scheme().to_string())
                    .as_deref(),
                Some("http" | "https")
            ) && safe_inlines(children)
        }
        Inline::HardBreak => false,
    })
}

/// 不把嵌套列表 / 标题 / HTML 和多块文本折叠成看似合法的行内文本。
fn normalize_item_text(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains("\n\n") || raw.contains("\r\r") {
        return None;
    }
    let mut lines = raw.lines();
    let first = lines.next()?.trim();
    let first = if let Some(rest) = first
        .strip_prefix("- ")
        .or_else(|| first.strip_prefix("* "))
        .or_else(|| first.strip_prefix("+ "))
    {
        rest
    } else if let Some((digits, rest)) = first.split_once(". ") {
        if !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()) {
            rest
        } else {
            first
        }
    } else {
        first
    };
    let mut parts = vec![first];
    for line in lines {
        let next = line.trim();
        if next.is_empty()
            || next.starts_with(['-', '*', '+', '#', '>', '`', '~'])
            || next.chars().next().is_some_and(|ch| ch.is_ascii_digit()) && next.contains(". ")
        {
            return None;
        }
        parts.push(next);
    }
    let normalized = parts.join(" ");
    let normalized = normalized.trim();
    if normalized.is_empty() || normalized.starts_with(['#', '>']) {
        return None;
    }
    // Renderer 会把原始 HTML 解析为块；即便标签恰好生成一个段落也不能当安全条目。
    // 用同一 Markdown 解析器判定，行内代码里的 `<tag>` 仍可作为文字保留。
    if Parser::new_ext(normalized, Options::empty()).any(|event| {
        matches!(
            event,
            Event::Html(_)
                | Event::InlineHtml(_)
                | Event::Start(Tag::Image { .. })
                | Event::Start(Tag::MetadataBlock(_))
        )
    }) {
        return None;
    }
    let doc = markdown_to_document(normalized);
    match doc.blocks.as_slice() {
        [Block::Paragraph { inlines }]
            if safe_inlines(inlines) && !inlines_to_plain(inlines).trim().is_empty() =>
        {
            Some(normalized.to_string())
        }
        _ => None,
    }
}

fn safe_title(raw: &str) -> String {
    let title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.is_empty() {
        "本期要闻"
    } else {
        title.as_str()
    };
    title
        .chars()
        .flat_map(|ch| {
            if "\\`*_{}[]()#+-.!>|~<>".contains(ch) {
                vec!['\\', ch]
            } else {
                vec![ch]
            }
        })
        .collect()
}

fn resolve_refs(refs: &[Value], numbered: &[DigestItem]) -> Vec<DigestSource> {
    let mut seen = HashSet::new();
    refs.iter()
        .filter_map(|value| value.as_u64().and_then(|n| usize::try_from(n).ok()))
        .filter_map(|n| n.checked_sub(1).and_then(|index| numbered.get(index)))
        .filter(|item| seen.insert(item.article_id))
        .take(3)
        .map(|item| DigestSource {
            article_id: item.article_id,
            title: item.title.clone(),
            feed_title: item.feed_title.clone(),
        })
        .collect()
}

fn serialize_v2(body: &str, items: Vec<V2Item>) -> Result<String> {
    serde_json::to_string(&V2Package {
        v: 2,
        body_hash: sha256_hex(body.as_bytes()),
        items,
    })
    .map_err(|err| digest_error(&format!("来源序列化失败：{err}")))
}

pub fn prepare_digest_draft(output: &Value, numbered: &[DigestItem]) -> Result<DigestDraft> {
    let object = output
        .as_object()
        .ok_or_else(|| digest_error("顶层不是对象"))?;
    if let Some(sections) = object.get("sections") {
        let sections = sections
            .as_array()
            .filter(|arr| !arr.is_empty())
            .ok_or_else(|| digest_error("sections 为空或格式错误"))?;
        let mut rendered = Vec::new();
        let mut expected = Vec::new();
        let mut items = Vec::new();
        let mut cited = HashSet::new();
        for section in sections {
            let Some(section) = section.as_object() else {
                continue;
            };
            let Some(title) = section.get("title").and_then(Value::as_str) else {
                continue;
            };
            let Some(raw_items) = section.get("items").and_then(Value::as_array) else {
                continue;
            };
            let mut lines = Vec::new();
            for item in raw_items {
                let Some(item) = item.as_object() else {
                    continue;
                };
                let Some(text) = item
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(normalize_item_text)
                else {
                    continue;
                };
                // 旧空话规则逐条执行，文字与来源作为一个对象一同去留。
                let cleaned = sanitize_digest_body(&format!("- {text}"));
                let Some(cleaned) = cleaned.strip_prefix("- ").and_then(normalize_item_text) else {
                    continue;
                };
                let parsed = markdown_to_document(&cleaned);
                let [Block::Paragraph { inlines }] = parsed.blocks.as_slice() else {
                    continue;
                };
                let sources = item
                    .get("refs")
                    .and_then(Value::as_array)
                    .map(|refs| resolve_refs(refs, numbered))
                    .unwrap_or_default();
                // 来源集合与前面某条完全相同，是模型把同一事件拆成了两条。
                let mut key: Vec<i64> = sources.iter().map(|source| source.article_id).collect();
                key.sort_unstable();
                if !key.is_empty() && !cited.insert(key) {
                    continue;
                }
                let section_id = rendered.len();
                expected.push((section_id, inlines_to_plain(inlines)));
                items.push(V2Item {
                    section: section_id,
                    refs: sources,
                });
                lines.push(format!("- {cleaned}"));
            }
            if !lines.is_empty() {
                rendered.push(format!("### {}\n\n{}", safe_title(title), lines.join("\n")));
            }
        }
        if rendered.is_empty() {
            return Err(digest_error("清洗后没有有效条目"));
        }
        let body = rendered.join("\n\n");
        if canonical_digest_items(&markdown_to_document(&body)).as_ref() != Some(&expected) {
            return Err(digest_error("规范正文与来源位置不一致"));
        }
        return Ok(DigestDraft {
            items_json: serialize_v2(&body, items)?,
            body,
        });
    }

    let raw = object
        .get("body")
        .and_then(Value::as_str)
        .filter(|body| !body.trim().is_empty())
        .ok_or_else(|| digest_error("缺少可用正文"))?;
    let refs = parse_digest_item_ns(output);
    let (body, aligned_refs) = sanitize_digest_body_with_item_refs(raw, &refs);
    if body.trim().is_empty() || count_digest_list_items(&body) == 0 {
        return Err(digest_error("清洗后没有有效条目"));
    }
    let doc = markdown_to_document(&body);
    if let Some(expected) = canonical_digest_items(&doc) {
        let items = expected
            .into_iter()
            .enumerate()
            .map(|(idx, (section, _))| V2Item {
                section,
                refs: aligned_refs
                    .get(idx)
                    .filter(|_| aligned_refs.len() == count_digest_list_items(&body))
                    .map(|refs| {
                        resolve_refs(
                            &refs.iter().map(|n| Value::from(*n)).collect::<Vec<_>>(),
                            numbered,
                        )
                    })
                    .unwrap_or_default(),
            })
            .collect();
        Ok(DigestDraft {
            items_json: serialize_v2(&body, items)?,
            body,
        })
    } else {
        // 无法证明旧 Markdown 一条恰好是一个顶层列表块：保留正文，不贴条目来源。
        Ok(DigestDraft {
            body,
            items_json: "[]".into(),
        })
    }
}

fn unique_sources(lines: &[Vec<DigestSource>]) -> Vec<DigestSource> {
    let mut seen = HashSet::new();
    lines
        .iter()
        .flatten()
        .filter(|source| seen.insert(source.article_id))
        .cloned()
        .collect()
}

fn decode_v2_refs(item: &Value) -> Vec<DigestSource> {
    let Some(refs) = item.get("refs").and_then(Value::as_array) else {
        return vec![];
    };
    let mut seen = HashSet::new();
    let mut valid = Vec::new();
    for source in refs {
        let Some(object) = source.as_object() else {
            return vec![];
        };
        let (Some(article_id), Some(title), Some(feed_title)) = (
            object.get("article_id").and_then(Value::as_i64),
            object.get("title").and_then(Value::as_str),
            object.get("feed_title").and_then(Value::as_str),
        ) else {
            return vec![];
        };
        if article_id <= 0 {
            return vec![];
        }
        if seen.insert(article_id) {
            valid.push(DigestSource {
                article_id,
                title: title.into(),
                feed_title: feed_title.into(),
            });
        }
    }
    valid.truncate(3);
    valid
}

fn safely_decoded_sources(item: &Value) -> Vec<DigestSource> {
    item.get("refs")
        .and_then(Value::as_array)
        .map(|refs| {
            refs.iter()
                .filter_map(|source| {
                    let id = source.get("article_id").and_then(Value::as_i64)?;
                    (id > 0).then(|| DigestSource {
                        article_id: id,
                        title: source
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        feed_title: source
                            .get("feed_title")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn decode_digest_sources(body: &str, raw: &str, doc: &Document) -> DecodedDigestSources {
    let count = doc
        .blocks
        .iter()
        .filter(|block| matches!(block, Block::ListItem { .. }))
        .count();
    let empty = || vec![Vec::new(); count];
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return DecodedDigestSources {
            state: SourceState::Unavailable,
            lines: empty(),
            unlocated: vec![],
        };
    };
    if value.is_array() {
        let stored = parse_digest_items_json(raw);
        let aligned = count > 0 && stored.len() == count;
        return DecodedDigestSources {
            state: if aligned {
                SourceState::V1
            } else {
                SourceState::Unavailable
            },
            lines: if aligned { stored.clone() } else { empty() },
            unlocated: if aligned {
                vec![]
            } else {
                unique_sources(&stored)
            },
        };
    }
    let Some(object) = value.as_object() else {
        return DecodedDigestSources {
            state: SourceState::Unavailable,
            lines: empty(),
            unlocated: vec![],
        };
    };
    let stored_items = object.get("items").and_then(Value::as_array);
    let candidates: Vec<Vec<DigestSource>> = stored_items
        .map(|items| items.iter().map(decode_v2_refs).collect())
        .unwrap_or_default();
    let unlocated = stored_items
        .map(|items| unique_sources(&items.iter().map(safely_decoded_sources).collect::<Vec<_>>()))
        .unwrap_or_default();
    let sections = canonical_digest_items(doc);
    let valid = object.get("v").and_then(Value::as_u64) == Some(2)
        && object.get("body_hash").and_then(Value::as_str)
            == Some(sha256_hex(body.as_bytes()).as_str())
        && sections
            .as_ref()
            .is_some_and(|sections| sections.len() == candidates.len())
        && object
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                sections.as_ref().is_some_and(|sections| {
                    items.iter().zip(sections).all(|(item, (section, _))| {
                        item.get("section").and_then(Value::as_u64) == Some(*section as u64)
                    })
                })
            });
    if !valid {
        return DecodedDigestSources {
            state: SourceState::Mismatch,
            lines: empty(),
            unlocated,
        };
    }
    DecodedDigestSources {
        state: SourceState::V2,
        lines: candidates,
        unlocated: vec![],
    }
}

/// 一期日报里的一条：日期、小节、正文与来源文章。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyDigestLine {
    pub day: String,
    pub section: String,
    pub text: String,
    pub article_ids: Vec<i64>,
}

/// 时间范围内各期日报的条目，按日期先后。只收来源能对上的条目；来源错位或丢失的旧稿整期跳过。
pub fn week_daily_lines(
    conn: &rusqlite::Connection,
    since_ms: i64,
    until_ms: i64,
) -> Result<Vec<DailyDigestLine>> {
    let mut stmt = conn.prepare(
        "SELECT period_start, body, COALESCE(items_json, '[]') FROM digests
         WHERE period = 'daily' AND period_start >= ?1 AND period_start <= ?2
         ORDER BY period_start, id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![since_ms, until_ms], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (start, body, raw) in rows {
        let doc = markdown_to_document(&body);
        let decoded = decode_digest_sources(&body, &raw, &doc);
        if !matches!(decoded.state, SourceState::V1 | SourceState::V2) {
            continue;
        }
        let day = crate::digest::digest_date_label("daily", start);
        let mut section = String::new();
        let mut line = 0usize;
        for block in &doc.blocks {
            match block {
                Block::Heading { inlines, .. } => {
                    section = inlines_to_plain(inlines).trim().to_string()
                }
                Block::ListItem { inlines, .. } => {
                    let ids: Vec<i64> = decoded
                        .lines
                        .get(line)
                        .map(|refs| refs.iter().map(|r| r.article_id).collect())
                        .unwrap_or_default();
                    line += 1;
                    let text = inlines_to_markdown(inlines);
                    if !ids.is_empty() && !text.trim().is_empty() {
                        out.push(DailyDigestLine {
                            day: day.clone(),
                            section: section.clone(),
                            text,
                            article_ids: ids,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// 周报输入保留日报条目里的粗体，其余行内格式按纯文本。
fn inlines_to_markdown(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Strong { children } => format!("**{}**", inlines_to_plain(children)),
            other => inlines_to_plain(std::slice::from_ref(other)),
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, title: &str) -> DigestItem {
        DigestItem {
            article_id: id,
            title: title.into(),
            feed_title: "测试".into(),
            summary: String::new(),
            summary_json: None,
            cluster_id: None,
        }
    }

    #[test]
    fn structured_output_keeps_source_slots_and_rejects_unsafe_blocks() {
        let input = serde_json::json!({"sections":[
            {"title":"AI *模型*", "items":[
                {"text":"**Alpha** 发布 2.0", "refs":[0,2,2,1,999]},
                {"text":"- 网友热议某话题", "refs":[1]},
                {"text":"- Beta 更新", "refs":[2]},
                {"text":"- 额外\n  - 嵌套", "refs":[1]},
                {"text":"<script>alert(1)</script>", "refs":[1]}
            ]},
            {"title":"空章节", "items":[{"text":"", "refs":[1]}]}
        ]});
        let out = prepare_digest_draft(&input, &[item(11, "Alpha"), item(22, "Beta")]).unwrap();
        let doc = markdown_to_document(&out.body);
        assert_eq!(canonical_digest_items(&doc).unwrap().len(), 2);
        let decoded = decode_digest_sources(&out.body, &out.items_json, &doc);
        assert_eq!(decoded.state, SourceState::V2);
        assert_eq!(
            decoded.lines[0]
                .iter()
                .map(|s| s.article_id)
                .collect::<Vec<_>>(),
            [22, 11]
        );
        assert_eq!(decoded.lines[1][0].article_id, 22);
        assert!(!out.body.contains("空章节"));
    }

    #[test]
    fn invalid_sections_never_fall_back_to_legacy_body() {
        let numbered = [item(11, "A")];
        for sections in [
            Value::Null,
            serde_json::json!([]),
            serde_json::json!("invalid"),
        ] {
            assert!(
                prepare_digest_draft(
                    &serde_json::json!({"sections":sections,"body":"- valid"}),
                    &numbered
                )
                .is_err()
            );
        }
        assert!(
            prepare_digest_draft(
                &serde_json::json!({"sections":[{"title":"x","items":[]}],"body":"- valid"}),
                &numbered
            )
            .is_err()
        );
    }

    #[test]
    fn v2_detects_hash_shape_section_and_version_mismatch_without_rebinding() {
        let out = prepare_digest_draft(&serde_json::json!({"sections":[{"title":"科技","items":[{"text":"Alpha 更新", "refs":[1]},{"text":"Beta 变化", "refs":[2]}]}]}), &[item(11,"Alpha"),item(22,"Beta")]).unwrap();
        let doc = markdown_to_document(&out.body);
        assert_eq!(
            decode_digest_sources(&out.body, &out.items_json, &doc).state,
            SourceState::V2
        );
        for changed in [
            out.body.replace("Alpha", "Gamma"),
            out.body
                .replace("- Alpha 更新\n- Beta 变化", "- Beta 变化\n- Alpha 更新"),
            format!("{}\n- 新条目", out.body),
        ] {
            let decoded =
                decode_digest_sources(&changed, &out.items_json, &markdown_to_document(&changed));
            assert_eq!(decoded.state, SourceState::Mismatch);
            assert!(decoded.lines.iter().all(Vec::is_empty));
            assert_eq!(decoded.unlocated.len(), 2);
        }
        for key in ["section", "v"] {
            let mut value: Value = serde_json::from_str(&out.items_json).unwrap();
            if key == "section" {
                value["items"][0]["section"] = Value::from(8);
            } else {
                value["v"] = Value::from(99);
            }
            assert_eq!(
                decode_digest_sources(&out.body, &value.to_string(), &doc).state,
                SourceState::Mismatch
            );
        }
        let mut value: Value = serde_json::from_str(&out.items_json).unwrap();
        value["items"][0]["refs"] = Value::from("invalid");
        let decoded = decode_digest_sources(&out.body, &value.to_string(), &doc);
        assert_eq!(decoded.state, SourceState::V2);
        assert!(decoded.lines[0].is_empty());
        assert_eq!(decoded.lines[1][0].article_id, 22);
        let mut value: Value = serde_json::from_str(&out.items_json).unwrap();
        value["items"][0]["refs"] = serde_json::json!([
            {"article_id": 11, "title": "Alpha", "feed_title": "测试"},
            {"article_id": "invalid", "title": "bad", "feed_title": "测试"}
        ]);
        let decoded = decode_digest_sources(&out.body, &value.to_string(), &doc);
        assert_eq!(decoded.state, SourceState::V2);
        assert!(decoded.lines[0].is_empty());
        assert_eq!(decoded.lines[1][0].article_id, 22);
    }

    #[test]
    fn structured_inline_validation_rejects_html_and_media_but_preserves_code() {
        let input = serde_json::json!({"sections":[{"title":"安全", "items":[
            {"text":"<b>非法标签</b>", "refs":[1]},
            {"text":"前缀 <img src='https://example.test/a'> 图片", "refs":[1]},
            {"text":"![远程图](https://example.test/a)", "refs":[1]},
            {"text":"1. **有效** `<b>` 正文", "refs":[1]}
        ]}]});
        let draft = prepare_digest_draft(&input, &[item(11, "Alpha")]).unwrap();
        let doc = markdown_to_document(&draft.body);
        assert_eq!(canonical_digest_items(&doc).unwrap().len(), 1);
        assert!(draft.body.contains("**有效**"));
        assert!(!draft.body.contains("非法标签"));
        assert_eq!(
            decode_digest_sources(&draft.body, &draft.items_json, &doc).lines[0][0].article_id,
            11
        );
    }

    #[test]
    fn fallback_only_promotes_canonical_body_and_never_guesses_missing_refs() {
        let numbered = [item(11, "Alpha")];
        let output = serde_json::json!({"body":"### 技术\n\n- Alpha 更新\n- Beta 更新", "item_refs":[{"ns":[1]}]});
        let draft = prepare_digest_draft(&output, &numbered).unwrap();
        let decoded = decode_digest_sources(
            &draft.body,
            &draft.items_json,
            &markdown_to_document(&draft.body),
        );
        assert_eq!(decoded.state, SourceState::V2);
        assert!(decoded.lines.iter().all(Vec::is_empty));
        let complex = serde_json::json!({"body":"- Alpha 更新\n  - nested"});
        assert_eq!(
            prepare_digest_draft(&complex, &numbered)
                .unwrap()
                .items_json,
            "[]"
        );
    }

    #[test]
    fn fallback_drops_out_of_range_refs_before_capping_at_three() {
        let output = serde_json::json!({"body":"### 技术\n\n- Alpha 更新", "item_refs":[{"ns":[999,998,997,1]}]});
        let draft = prepare_digest_draft(&output, &[item(11, "Alpha")]).unwrap();
        let doc = markdown_to_document(&draft.body);
        let decoded = decode_digest_sources(&draft.body, &draft.items_json, &doc);
        assert_eq!(decoded.lines[0][0].article_id, 11);
    }

    #[test]
    fn fallback_refuses_slots_when_duplicate_text_makes_deletion_ambiguous() {
        let body = "### 技术\n\n- 网友热议某话题\n- **网友热议某话题**\n- Beta 更新";
        let output =
            serde_json::json!({"body": body, "item_refs":[{"ns":[1]},{"ns":[2]},{"ns":[3]}]});
        let numbered = [item(11, "A"), item(22, "B"), item(33, "C")];
        let draft = prepare_digest_draft(&output, &numbered).unwrap();
        let doc = markdown_to_document(&draft.body);
        let decoded = decode_digest_sources(&draft.body, &draft.items_json, &doc);
        assert_eq!(decoded.lines.len(), 2);
        assert!(decoded.lines.iter().all(Vec::is_empty));
    }

    #[test]
    fn repeated_source_set_drops_the_later_split_item_only() {
        let input = serde_json::json!({"sections":[
            {"title":"就业", "items":[
                {"text":"**斯坦福研究**显示入门岗位受冲击", "refs":[1]},
                {"text":"Alpha 与 Beta 合并报道", "refs":[2,3]}
            ]},
            {"title":"观察", "items":[
                {"text":"**斯坦福研究之外**冲击仍待观察", "refs":[1]},
                {"text":"Beta 的另一面", "refs":[3]},
                {"text":"无来源条目", "refs":[]},
                {"text":"另一条无来源条目", "refs":[]}
            ]}
        ]});
        let numbered = [item(11, "S"), item(22, "A"), item(33, "B")];
        let draft = prepare_digest_draft(&input, &numbered).unwrap();
        assert!(!draft.body.contains("斯坦福研究之外"));
        assert!(draft.body.contains("Beta 的另一面"));
        assert!(draft.body.contains("另一条无来源条目"));
        let doc = markdown_to_document(&draft.body);
        assert_eq!(canonical_digest_items(&doc).unwrap().len(), 5);
    }

    #[test]
    fn revision_includes_raw_snapshots_even_if_body_is_unchanged() {
        assert_ne!(
            digest_revision("body", "[]"),
            digest_revision("body", "[ ]")
        );
        assert_ne!(digest_revision("a", "bc"), digest_revision("ab", "c"));
    }
}
