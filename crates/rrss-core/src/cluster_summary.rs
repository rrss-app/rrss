//! 相近报道综述（AI 规格 · 相近报道综述）：输入组装、输出校验与按簇存取。调用模型在 `rrss-app`。
//!
//! 输入只用这组在当前范围内可见的成员；〈序号〉从 1 开始，与 `ClusterInput::members` 同序。
//! 存库时把序号换成文章 id，成员列表另存为「文章 id + 摘要指纹」，用来判断有没有新报道、新来的是哪几篇。

use std::sync::OnceLock;

use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::articles::{self, ArticleCard};
use crate::digest::{self, DigestItem};
use crate::error::Result;
use crate::feed_scope::FeedScope;
use crate::summary_extract::{self, InputTier};

/// 「说法不一」最多留几条；更多的多半是模型把补充写成了分歧。
const MAX_CONFLICTS: usize = 4;
/// 没有摘要的篇取正文摘录的下限，避免成员很多时每篇只剩一句。
const MIN_EXCERPT_CHARS: usize = 300;

/// 输入含模板，按 0.75 token / 字加 800 输出 token，向上取整到 500。
pub fn estimate_tokens(input_chars: usize) -> i64 {
    ((input_chars * 3 / 4 + 800).div_ceil(500) * 500) as i64
}

/// 一篇成员在输入里的样子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub article_id: i64,
    pub feed_id: i64,
    pub feed_title: String,
    pub title: String,
    pub published_at: Option<i64>,
    /// 已有的一句话（没有摘要时为空），没配模型时界面列出它。
    pub one_liner: String,
}

pub struct ClusterInput {
    pub prompt: String,
    pub members: Vec<Member>,
    pub marks: Vec<MemberMark>,
}

/// 成员签名的一项：文章 id 与生成时它的摘要指纹。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberMark {
    pub id: i64,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Addition {
    pub article_id: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conflict {
    pub article_ids: Vec<i64>,
    pub text: String,
}

/// 校验过的综述，来源已换成文章 id。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterDigest {
    pub overview: String,
    #[serde(default)]
    pub additions: Vec<Addition>,
    #[serde(default)]
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSummary {
    pub digest: ClusterDigest,
    pub members: Vec<MemberMark>,
    pub generated_at: i64,
}

/// 这组在范围内可见的成员（不足两篇为空），口径同焦点展开。
pub fn visible_members(
    conn: &Connection,
    cluster_id: i64,
    scope: FeedScope,
) -> Result<Vec<ArticleCard>> {
    articles::focus_cluster_members(conn, cluster_id, scope)
}

/// 全部焦点范围内的自动综述候选；先处理还没有综述的组。
pub fn due_auto_summaries(conn: &Connection, now: i64, limit: i64) -> Result<Vec<i64>> {
    auto_candidates(conn, now, limit, None)
}

/// 领取任务和写回前复核，避免排队期间的手动生成、隐藏或成员变化被覆盖。
pub fn auto_summary_due(conn: &Connection, cluster_id: i64, now: i64) -> Result<bool> {
    Ok(!auto_candidates(conn, now, 1, Some(cluster_id))?.is_empty())
}

fn auto_candidates(conn: &Connection, now: i64, limit: i64, only: Option<i64>) -> Result<Vec<i64>> {
    use crate::ai_store::{CLUSTER_REFRESH_HOURS, CLUSTER_WINDOW_DAYS};
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let visible = articles::FOCUS_VISIBLE_SQL.replace("$threshold", "?1");
    let mut stmt = conn.prepare(&format!(
        "SELECT c.id FROM clusters c JOIN articles a ON a.cluster_id=c.id
         JOIN feeds f ON f.id=a.feed_id
         WHERE {visible} AND c.member_count>=2 AND (?3 IS NULL OR c.id=?3)
         GROUP BY c.id
         HAVING COUNT(DISTINCT a.feed_id)>=2 AND MAX(a.created_at)>=?2
         ORDER BY c.summary_json IS NOT NULL, MAX(a.created_at) DESC, c.id DESC"
    ))?;
    let rows = stmt.query_map(
        params![
            articles::hide_threshold(conn)?,
            now - CLUSTER_WINDOW_DAYS * 86_400_000,
            only
        ],
        |row| row.get::<_, i64>(0),
    )?;
    let mut out = Vec::new();
    for id in rows {
        let id = id?;
        if let Some(stored) = load(conn, id)? {
            if now.saturating_sub(stored.generated_at) <= CLUSTER_REFRESH_HOURS * 3_600_000 {
                continue;
            }
            let members = visible_members(conn, id, FeedScope::All)?;
            let marks = member_marks(conn, &members)?;
            if freshness(&stored.members, &marks).0.is_empty() {
                continue;
            }
        }
        out.push(id);
        if out.len() as i64 == limit {
            break;
        }
    }
    Ok(out)
}

struct Source {
    title: String,
    summary: String,
    summary_json: Option<String>,
    blocks_json: Option<String>,
    content_html: Option<String>,
}

fn read_source(conn: &Connection, id: i64) -> Result<Option<Source>> {
    conn.query_row(
        "SELECT title, COALESCE(summary, ''), summary_json, blocks_json, content_html FROM articles WHERE id = ?1",
        [id],
        |row| {
            Ok(Source {
                title: row.get(0)?,
                summary: row.get(1)?,
                summary_json: row.get(2)?,
                blocks_json: row.get(3)?,
                content_html: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn has_summary(source: &Source) -> bool {
    !source.summary.trim().is_empty()
        || source
            .summary_json
            .as_deref()
            .is_some_and(|raw| !raw.trim().is_empty())
}

fn mark(id: i64, source: &Source) -> MemberMark {
    MemberMark {
        id,
        hash: crate::ai_store::hash_input(
            &serde_json::json!([
                source.summary,
                source.summary_json.as_deref().unwrap_or_default()
            ])
            .to_string(),
        ),
    }
}

/// 当前可见成员的签名，与 `StoredSummary::members` 比较得出「有新报道」。
pub fn member_marks(conn: &Connection, members: &[ArticleCard]) -> Result<Vec<MemberMark>> {
    let mut out = Vec::with_capacity(members.len());
    for card in members {
        if let Some(source) = read_source(conn, card.id)? {
            out.push(mark(card.id, &source));
        }
    }
    Ok(out)
}

fn clock(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// 输入：每篇给〈序号〉、来源、发布时间、标题和已存的一句话 / 要点 / 关键事实；没有摘要的篇取正文摘录，
/// 摘录预算是档位的单篇摘要上限按篇平分（整段挑选规则同单篇摘要）。
pub fn build_input(
    conn: &Connection,
    members: &[ArticleCard],
    tier: InputTier,
) -> Result<ClusterInput> {
    let per_item = (tier.summary_budget(false) / members.len().max(1)).max(MIN_EXCERPT_CHARS);
    let mut blocks = Vec::new();
    let mut out = Vec::new();
    let mut marks = Vec::new();
    let mut feeds = std::collections::HashSet::new();
    for card in members {
        let Some(source) = read_source(conn, card.id)? else {
            continue;
        };
        let summarized = has_summary(&source);
        let item = DigestItem {
            article_id: card.id,
            title: source.title.clone(),
            summary: source.summary.clone(),
            summary_json: source.summary_json.clone(),
            cluster_id: card.cluster_id,
            feed_title: card.feed_title.clone(),
        };
        let one_liner = if summarized {
            digest::parse_summary_details_for_digest(&item.summary, item.summary_json.as_deref()).0
        } else {
            String::new()
        };
        let n = out.len() + 1;
        let time = clock(card.published_at);
        let mut block = format!(
            "〈{n}〉{} · {}\n标题：{}",
            short_source_name(&card.feed_title),
            if time.is_empty() {
                "时间未知"
            } else {
                &time
            },
            source.title
        );
        if summarized {
            block.push_str(digest::brief_lines(&item).trim_end());
        } else {
            let excerpt = summary_extract::extract_body_excerpt(
                source.blocks_json.as_deref(),
                source.content_html.as_deref(),
                per_item,
            );
            if !excerpt.trim().is_empty() {
                block.push_str(&format!("\n正文摘录：\n{}", excerpt.trim()));
            }
        }
        blocks.push(block);
        feeds.insert(card.feed_id);
        marks.push(mark(card.id, &source));
        out.push(Member {
            article_id: card.id,
            feed_id: card.feed_id,
            feed_title: card.feed_title.clone(),
            title: source.title,
            published_at: card.published_at,
            one_liner,
        });
    }
    let prompt = if blocks.is_empty() {
        String::new()
    } else {
        format!(
            "同一事件的 {} 篇报道（来自 {} 个来源）。〈序号〉只填在 ref / refs 里，正文提到某篇时写它的来源名。\n\n{}",
            out.len(),
            feeds.len(),
            blocks.join("\n\n---\n\n")
        )
    };
    Ok(ClusterInput {
        prompt,
        members: out,
        marks,
    })
}

/// 补全请求的 JSON Schema。
pub fn json_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "overview": { "type": "string" },
            "additions": { "type": "array", "items": { "type": "object", "properties": {
                "ref": { "type": "integer" }, "text": { "type": "string" } }, "required": ["ref", "text"] } },
            "conflicts": { "type": "array", "items": { "type": "object", "properties": {
                "refs": { "type": "array", "items": { "type": "integer" } }, "text": { "type": "string" } }, "required": ["refs", "text"] } }
        },
        "required": ["overview", "additions", "conflicts"]
    })
}

/// 序号可能写成 2、"2" 或「〈2〉」；只认 1..=n。
fn parse_ref(value: &Value, n: usize) -> Option<usize> {
    let number = match value {
        Value::Number(num) => num.as_u64().map(|v| v as usize),
        Value::String(text) => text
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok(),
        _ => None,
    }?;
    (1..=n).contains(&number).then_some(number)
}

/// 来源短名：「名称 - 说明」这类标题只取分隔符前的名称，正文里不塞「- Telegram Channel」。
pub fn short_source_name(feed_title: &str) -> String {
    let title = feed_title.trim();
    for separator in [" - ", " – ", " — ", " | "] {
        if let Some((head, _)) = title.split_once(separator) {
            let head = head.trim();
            if head.chars().count() >= 2 {
                return head.to_string();
            }
        }
    }
    title.to_string()
}

/// 正文里的〈n〉（以及 <n>、[n]、【n】等写法）换成第 n 篇的来源名；对不上的序号删掉。
/// 模型常照抄输入里的序号，界面上读者看不懂它（阶段 14 · 2026-11-08 调整）。
pub fn name_refs(text: &str, name: impl Fn(usize) -> Option<String>) -> String {
    static REFS: OnceLock<Regex> = OnceLock::new();
    let refs = REFS.get_or_init(|| Regex::new(r"[〈<＜\[【]\s*(\d{1,3})\s*[〉>＞\]】]").unwrap());
    refs.replace_all(text, |caps: &regex::Captures| {
        caps[1]
            .parse::<usize>()
            .ok()
            .and_then(&name)
            .unwrap_or_default()
    })
    .into_owned()
}

fn text_of(value: &Value) -> Option<String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

/// 校验模型输出：序号对不上的条目丢掉；「各家补充」每个来源至多一条；综述为空返回 `None`（按失败处理）。
pub fn parse_output(value: &Value, members: &[Member]) -> Option<ClusterDigest> {
    let n = members.len();
    let named = |text: &str| {
        name_refs(text, |i| {
            (1..=n)
                .contains(&i)
                .then(|| short_source_name(&members[i - 1].feed_title))
        })
        .trim()
        .to_string()
    };
    let overview = named(
        value
            .get("overview")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    if overview.is_empty() {
        return None;
    }
    let mut seen_feeds = std::collections::HashSet::new();
    let additions = value
        .get("additions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let member = &members[parse_ref(entry.get("ref")?, n)? - 1];
            let text = text_of(entry)
                .map(|text| named(&text))
                .filter(|t| !t.is_empty())?;
            seen_feeds.insert(member.feed_id).then(|| Addition {
                article_id: member.article_id,
                text,
            })
        })
        .collect();
    let conflicts = value
        .get("conflicts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let mut ids: Vec<i64> = Vec::new();
            for raw in entry
                .get("refs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(index) = parse_ref(raw, n) {
                    let id = members[index - 1].article_id;
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            let text = text_of(entry)
                .map(|text| named(&text))
                .filter(|t| !t.is_empty())?;
            (!ids.is_empty()).then_some(Conflict {
                article_ids: ids,
                text,
            })
        })
        .take(MAX_CONFLICTS)
        .collect();
    Some(ClusterDigest {
        overview,
        additions,
        conflicts,
    })
}

pub fn save(
    conn: &Connection,
    cluster_id: i64,
    digest: &ClusterDigest,
    members: &[MemberMark],
    at: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE clusters SET summary_json = ?1, summary_members = ?2, summary_at = ?3 WHERE id = ?4",
        // 这两类只含字符串与整数，序列化不会失败。
        params![
            serde_json::to_string(digest).unwrap_or_default(),
            serde_json::to_string(members).unwrap_or_default(),
            at,
            cluster_id
        ],
    )?;
    Ok(())
}

/// 读回存着的综述；解析不了的旧数据当作没有。
pub fn load(conn: &Connection, cluster_id: i64) -> Result<Option<StoredSummary>> {
    let row: Option<(Option<String>, Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT summary_json, summary_members, summary_at FROM clusters WHERE id = ?1",
            [cluster_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((Some(digest), Some(members), Some(generated_at))) = row else {
        return Ok(None);
    };
    match (
        serde_json::from_str(&digest),
        serde_json::from_str(&members),
    ) {
        (Ok(digest), Ok(members)) => Ok(Some(StoredSummary {
            digest,
            members,
            generated_at,
        })),
        _ => Ok(None),
    }
}

/// 上次汇总之后新来的成员（按当前成员顺序），以及签名是否变了（含成员减少、摘要更新）。
pub fn freshness(stored: &[MemberMark], current: &[MemberMark]) -> (Vec<i64>, bool) {
    let known: std::collections::HashSet<i64> = stored.iter().map(|m| m.id).collect();
    let fresh = current
        .iter()
        .filter(|m| !known.contains(&m.id))
        .map(|m| m.id)
        .collect();
    let mut a: Vec<_> = stored.iter().map(|m| (m.id, m.hash.as_str())).collect();
    let mut b: Vec<_> = current.iter().map(|m| (m.id, m.hash.as_str())).collect();
    a.sort_unstable();
    b.sort_unstable();
    (fresh, a != b)
}

/// 标题行上的「新 N」：只在这组存过综述时，数可见成员里不在综述签名中的几篇。
pub fn fresh_counts(
    conn: &Connection,
    groups: &[(i64, Vec<i64>)],
) -> Result<std::collections::HashMap<i64, i64>> {
    let mut out = std::collections::HashMap::new();
    for (cluster_id, ids) in groups {
        let raw: Option<Option<String>> = conn
            .query_row(
                "SELECT summary_members FROM clusters WHERE id = ?1",
                [cluster_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(Some(raw)) = raw else { continue };
        let Ok(marks) = serde_json::from_str::<Vec<MemberMark>>(&raw) else {
            continue;
        };
        let known: std::collections::HashSet<i64> = marks.iter().map(|m| m.id).collect();
        let fresh = ids.iter().filter(|id| !known.contains(id)).count() as i64;
        if fresh > 0 {
            out.insert(*cluster_id, fresh);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seeded() -> (tempfile::TempDir, crate::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("cluster-summary.db")).unwrap();
        conn.execute_batch(
            r#"INSERT INTO feeds(id,url,title,created_at) VALUES
                 (1,'https://a.example/rss','甲报',0),(2,'https://b.example/rss','乙报',0);
               INSERT INTO clusters(id,label,member_count,created_at) VALUES (7,'新规',3,0);
               INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,summary,summary_json,content_html,created_at) VALUES
                 (1,1,'新规公布',5,1000,7,'新规今日公布。','{"one_liner":"新规今日公布","key_points":["门槛提高"],"facts":["过渡期 90 天"]}',NULL,1),
                 (2,2,'厂商回应',5,2000,7,'',NULL,'<p>多家厂商表示现有订单照常交付，下一代产品需要重新送审。</p>',1),
                 (3,1,'细则解读',5,3000,7,'细则给出算法。',NULL,NULL,1);"#,
        )
        .unwrap();
        (dir, conn)
    }

    fn members(conn: &Connection) -> Vec<ArticleCard> {
        visible_members(conn, 7, FeedScope::All).unwrap()
    }

    #[test]
    fn auto_candidates_require_visible_sources_recent_members_and_new_reports() {
        let (_dir, conn) = seeded();
        let now = 1_800_000_000_000;
        let window = crate::ai_store::CLUSTER_WINDOW_DAYS * 86_400_000;
        let interval = crate::ai_store::CLUSTER_REFRESH_HOURS * 3_600_000;
        conn.execute("UPDATE articles SET created_at=?1", [now])
            .unwrap();
        let due = || due_auto_summaries(&conn, now, 4).unwrap();
        assert_eq!(due(), [7]);
        conn.execute("UPDATE articles SET feed_id=1 WHERE id=2", [])
            .unwrap();
        assert!(due().is_empty());
        conn.execute(
            "UPDATE articles SET feed_id=2,score=-10,interest_boost=100 WHERE id=2",
            [],
        )
        .unwrap();
        assert!(due().is_empty(), "兴趣不能放行规则隐藏的第二个来源");
        conn.execute("UPDATE articles SET released_at=?1 WHERE id=2", [now])
            .unwrap();
        assert_eq!(due(), [7]);
        conn.execute("UPDATE articles SET created_at=?1", [now - window - 1])
            .unwrap();
        assert!(due().is_empty());
        conn.execute(
            "UPDATE articles SET created_at=?1,score=-10 WHERE id=3",
            [now],
        )
        .unwrap();
        assert!(due().is_empty(), "隐藏的新成员不能扩大时间窗口");
        conn.execute(
            "UPDATE articles SET score=5,created_at=?1 WHERE id=3",
            [now - window],
        )
        .unwrap();
        assert_eq!(due(), [7]);
        let digest = ClusterDigest {
            overview: "已存综述".into(),
            additions: vec![],
            conflicts: vec![],
        };
        let marks = member_marks(&conn, &members(&conn)).unwrap();
        save(&conn, 7, &digest, &marks, now - interval - 1).unwrap();
        assert!(due().is_empty());
        conn.execute("UPDATE articles SET summary='新摘要' WHERE id=1", [])
            .unwrap();
        assert!(due().is_empty(), "只有摘要更新不能触发自动重做");
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,score,cluster_id,created_at)
            VALUES(4,1,'新成员',5,7,?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "UPDATE clusters SET summary_at=?1 WHERE id=7",
            [now - interval],
        )
        .unwrap();
        assert!(due().is_empty(), "恰好 24 小时仍不更新");
        conn.execute(
            "UPDATE clusters SET summary_at=?1 WHERE id=7",
            [now - interval - 1],
        )
        .unwrap();
        assert_eq!(due(), [7]);
        conn.execute_batch(
            "INSERT INTO clusters(id,label,member_count,created_at) VALUES(8,'未汇总',2,0);
            INSERT INTO articles(id,feed_id,title,score,cluster_id,created_at) VALUES
            (5,1,'另一组甲',0,8,0),(6,2,'另一组乙',0,8,0);",
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET created_at=?1 WHERE cluster_id=8",
            [now - window],
        )
        .unwrap();
        assert_eq!(due(), [8, 7]);
        assert_eq!(due_auto_summaries(&conn, now, 1).unwrap(), [8]);
    }

    #[test]
    fn input_numbers_members_and_uses_summaries_or_body_excerpts() {
        let (_dir, conn) = seeded();
        let input = build_input(&conn, &members(&conn), InputTier::Medium).unwrap();
        assert_eq!(
            input
                .members
                .iter()
                .map(|m| m.article_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(
            input
                .prompt
                .starts_with("同一事件的 3 篇报道（来自 2 个来源）")
        );
        assert!(input.prompt.contains("〈1〉甲报"));
        assert!(input.prompt.contains("过渡期 90 天"));
        // 没有摘要的篇取正文摘录，不拿标题冒充一句话。
        assert!(input.prompt.contains("〈2〉乙报"));
        assert!(
            input
                .prompt
                .contains("正文摘录：\n多家厂商表示现有订单照常交付")
        );
        assert_eq!(input.members[1].one_liner, "");
        assert_eq!(input.members[0].one_liner, "新规今日公布");
        assert_eq!(input.marks.len(), 3);
    }

    #[test]
    fn text_names_sources_instead_of_input_numbers() {
        let (_dir, conn) = seeded();
        let input = build_input(&conn, &members(&conn), InputTier::Medium).unwrap();
        assert!(input.prompt.contains("正文提到某篇时写它的来源名"));
        let parsed = parse_output(
            &json!({
                "overview": "〈1〉与[2]都报道了新规。",
                "additions": [{"ref": 2, "text": "〈9〉"}],
                "conflicts": [{"refs": [1, 2], "text": "〈1〉称 3 月生效，＜2＞称 5 月"}]
            }),
            &input.members,
        )
        .unwrap();
        assert_eq!(parsed.overview, "甲报与乙报都报道了新规。");
        // 只剩对不上的序号，删掉后为空，这条丢掉。
        assert!(parsed.additions.is_empty());
        assert_eq!(parsed.conflicts[0].text, "甲报称 3 月生效，乙报称 5 月");
        assert_eq!(
            short_source_name("在花科技圈 - Telegram Channel"),
            "在花科技圈"
        );
        assert_eq!(short_source_name("A - 说明"), "A - 说明");
        assert_eq!(short_source_name("小众软件"), "小众软件");
    }

    #[test]
    fn output_keeps_only_known_refs_and_one_addition_per_source() {
        let (_dir, conn) = seeded();
        let input = build_input(&conn, &members(&conn), InputTier::Medium).unwrap();
        let parsed = parse_output(
            &json!({
                "overview": " 新规公布，**厂商**回应。 ",
                "additions": [
                    {"ref": 3, "text": "给出算法"},
                    {"ref": "〈1〉", "text": "同一来源的第二条，丢掉"},
                    {"ref": 9, "text": "对不上的序号"},
                    {"ref": 2, "text": "  "},
                    {"ref": 2, "text": "订单照常交付"}
                ],
                "conflicts": [
                    {"refs": [1, "3", 1, 42], "text": "过渡期说法不一"},
                    {"refs": [42], "text": "全对不上，丢掉"}
                ]
            }),
            &input.members,
        )
        .unwrap();
        assert_eq!(parsed.overview, "新规公布，**厂商**回应。");
        assert_eq!(
            parsed.additions,
            vec![
                Addition {
                    article_id: 3,
                    text: "给出算法".into()
                },
                Addition {
                    article_id: 2,
                    text: "订单照常交付".into()
                },
            ]
        );
        assert_eq!(
            parsed.conflicts,
            vec![Conflict {
                article_ids: vec![1, 3],
                text: "过渡期说法不一".into()
            }]
        );
        assert!(
            parse_output(
                &json!({"overview": " ", "additions": [], "conflicts": []}),
                &input.members
            )
            .is_none()
        );
    }

    #[test]
    fn stored_summary_round_trips_and_reports_new_members() {
        let (_dir, conn) = seeded();
        let cards = members(&conn);
        let input = build_input(&conn, &cards[..2], InputTier::Low).unwrap();
        let digest = ClusterDigest {
            overview: "综述".into(),
            additions: vec![],
            conflicts: vec![],
        };
        save(&conn, 7, &digest, &input.marks, 42).unwrap();
        let stored = load(&conn, 7).unwrap().unwrap();
        assert_eq!((stored.digest, stored.generated_at), (digest, 42));
        let current = member_marks(&conn, &cards).unwrap();
        assert_eq!(freshness(&stored.members, &current), (vec![3], true));
        assert_eq!(freshness(&stored.members, &stored.members), (vec![], false));
        // 摘要更新也算有变化，但不算新来的报道。
        conn.execute("UPDATE articles SET summary='改过的摘要' WHERE id=1", [])
            .unwrap();
        let changed = member_marks(&conn, &cards[..2]).unwrap();
        assert_eq!(freshness(&stored.members, &changed), (vec![], true));
        let counts = fresh_counts(&conn, &[(7, vec![1, 2, 3]), (8, vec![5])]).unwrap();
        assert_eq!(counts.get(&7), Some(&1));
        assert!(!counts.contains_key(&8));
        assert!(load(&conn, 8).unwrap().is_none());
    }
}
