use rusqlite::Connection;

use rrss_core::ai_store;
use rrss_core::articles::{self, FocusFilter};
use rrss_core::db;
use rrss_core::feed_scope::FeedScope;
use rrss_core::interests;
use rrss_core::rag;

use super::ai::digest_banner_label;
use crate::views::*;

pub fn parse_focus_filter(value: &str) -> FocusFilter {
    FocusFilter::parse_str(value)
}

pub fn focus_page(
    conn: &Connection,
    expanded_cluster: Option<i64>,
    exclude_ids: &[i64],
    exclude_cluster_ids: &[i64],
    more: bool,
    scope: FeedScope,
) -> rrss_core::Result<FocusPage> {
    let scope = articles::focus_column_scope(scope);
    let (cards, has_more) = articles::list_focus_page(
        conn,
        articles::FOCUS_PAGE_SIZE,
        expanded_cluster,
        exclude_ids,
        exclude_cluster_ids,
        scope,
    )?;
    let articles = article_views(conn, cards)?;
    // 续页与栏目都不带简报；栏目的排序原因仍用入库时的全局结果。
    let digest = if more || scope != FeedScope::All {
        None
    } else {
        ai_store::latest_digest(conn, "daily")
            .ok()
            .flatten()
            .or_else(|| ai_store::latest_digest(conn, "weekly").ok().flatten())
    };
    Ok(FocusPage {
        articles,
        digest_banner: digest
            .as_ref()
            .map(|d| digest_banner_label(&d.period, d.period_start))
            .unwrap_or_default(),
        digest_lede: digest
            .as_ref()
            .map(|d| digest_lede(&d.body))
            .unwrap_or_default(),
        filter: articles::focus_filter(conn)?.as_str().to_string(),
        list_scope: articles::list_scope(conn)?.as_str().to_string(),
        status: if more {
            String::new()
        } else {
            focus_status(conn)
        },
        ranking: ranking_settings(conn)?,
        has_more,
    })
}

pub fn focus_cluster_members(
    conn: &Connection,
    cluster_id: i64,
    scope: FeedScope,
) -> rrss_core::Result<Vec<ArticleView>> {
    article_views(
        conn,
        articles::focus_cluster_members(conn, cluster_id, scope)?,
    )
}

/// 相近报道综述：存着的综述、与当前成员比较的新旧，以及没配模型时列出的各篇一句话。
/// `scope` 与焦点页相同（`all` / `folder:<id>`）；成员口径同 `focus_cluster_members`。
pub fn cluster_summary(
    conn: &Connection,
    cluster_id: i64,
    scope: Option<&str>,
) -> rrss_core::Result<ClusterSummaryView> {
    use rrss_core::cluster_summary as summary;
    let scope = rrss_core::feed_scope::resolve(conn, FeedScope::parse_str(scope.unwrap_or("all")))?;
    let cards = summary::visible_members(conn, cluster_id, scope)?;
    let input = summary::build_input(conn, &cards, ai_store::input_tier(conn)?)?;
    let icons = super::feeds::feed_icon_hrefs(conn)?;
    let resolved = ai_store::resolve_task(conn, ai_store::TASK_CLUSTER_SUMMARY).ok();
    let template_chars = resolved
        .as_ref()
        .map_or(0, |task| task.template.body.chars().count());
    let estimate = summary::estimate_tokens(template_chars + input.prompt.chars().count());
    let article = |member: &summary::Member| ClusterArticleView {
        article_id: member.article_id,
        feed_id: member.feed_id,
        feed_title: member.feed_title.clone(),
        icon_href: icons.get(&member.feed_id).cloned().unwrap_or_default(),
        title: member.title.clone(),
        published_at: member.published_at,
        one_liner: member.one_liner.clone(),
    };
    let members: Vec<ClusterArticleView> = input.members.iter().map(article).collect();
    // 综述里提到的篇可能已不在当前成员里（被规则隐藏、换了范围）：从库里补来源，删掉的就不列。
    let lookup = |id: i64| -> rrss_core::Result<Option<ClusterArticleView>> {
        if let Some(found) = members.iter().find(|m| m.article_id == id) {
            return Ok(Some(found.clone()));
        }
        use rusqlite::OptionalExtension;
        Ok(conn
            .query_row(
                "SELECT a.feed_id, COALESCE(f.title, f.url), a.title, a.published_at
                 FROM articles a JOIN feeds f ON f.id = a.feed_id WHERE a.id = ?1",
                [id],
                |row| {
                    Ok(ClusterArticleView {
                        article_id: id,
                        feed_id: row.get(0)?,
                        feed_title: row.get(1)?,
                        icon_href: String::new(),
                        title: row.get(2)?,
                        published_at: row.get(3)?,
                        one_liner: String::new(),
                    })
                },
            )
            .optional()?
            .map(|mut found| {
                found.icon_href = icons.get(&found.feed_id).cloned().unwrap_or_default();
                found
            }))
    };
    let stored = summary::load(conn, cluster_id)?;
    let (state, fresh, overview, additions, conflicts, basis, generated_at) = match stored {
        None => (
            "none",
            Vec::new(),
            String::new(),
            Vec::new(),
            Vec::new(),
            0,
            None,
        ),
        Some(stored) => {
            let (fresh_ids, changed) = summary::freshness(&stored.members, &input.marks);
            // 11-08 之前存的综述正文里可能还留着〈n〉：按生成时的成员顺序换成来源名。
            let mut names = Vec::with_capacity(stored.members.len());
            for mark in &stored.members {
                names.push(
                    lookup(mark.id)?.map(|found| summary::short_source_name(&found.feed_title)),
                );
            }
            let named = |text: &str| {
                summary::name_refs(text, |i| names.get(i.wrapping_sub(1)).cloned().flatten())
            };
            let fresh = members
                .iter()
                .filter(|m| fresh_ids.contains(&m.article_id))
                .cloned()
                .collect();
            let mut additions = Vec::new();
            for note in &stored.digest.additions {
                if let Some(source) = lookup(note.article_id)? {
                    additions.push(ClusterNoteView {
                        text: named(&note.text),
                        sources: vec![source],
                    });
                }
            }
            let mut conflicts = Vec::new();
            for note in &stored.digest.conflicts {
                let mut sources = Vec::new();
                for id in &note.article_ids {
                    sources.extend(lookup(*id)?);
                }
                if !sources.is_empty() {
                    conflicts.push(ClusterNoteView {
                        text: named(&note.text),
                        sources,
                    });
                }
            }
            (
                if changed { "stale" } else { "ready" },
                fresh,
                named(&stored.digest.overview),
                additions,
                conflicts,
                stored.members.len() as i64,
                Some(stored.generated_at),
            )
        }
    };
    Ok(ClusterSummaryView {
        cluster_id,
        state: state.into(),
        model_ready: resolved.is_some_and(|task| !task.provider.base_url.trim().is_empty()),
        estimate_tokens: estimate,
        overview,
        additions,
        conflicts,
        basis,
        generated_at,
        fresh,
        members,
    })
}

fn article_views(
    conn: &Connection,
    cards: Vec<rrss_core::ArticleCard>,
) -> rrss_core::Result<Vec<ArticleView>> {
    let icons = super::feeds::feed_icon_hrefs(conn)?;
    let groups: Vec<(i64, Vec<i64>)> = cards
        .iter()
        .filter(|card| card.row_kind == articles::ROW_HEADER)
        .filter_map(|card| Some((card.cluster_id?, card.cluster_members.clone())))
        .collect();
    let fresh = rrss_core::cluster_summary::fresh_counts(conn, &groups)?;
    cards
        .into_iter()
        .map(|card| {
            let header = card.row_kind == articles::ROW_HEADER;
            let mut article = article_from_card(card);
            if header {
                article.cluster_fresh = fresh.get(&article.cluster_id).copied();
            }
            article.feed_icon_href = icons.get(&article.feed_id).cloned().unwrap_or_default();
            for source in article.cluster_sources.iter_mut().flatten() {
                source.icon_href = icons.get(&source.feed_id).cloned().unwrap_or_default();
            }
            if article.spot_check {
                let raw: Option<String> = conn.query_row(
                    "SELECT reasons_json FROM articles WHERE id=?1",
                    [article.id],
                    |row| row.get(0),
                )?;
                article.hide_reasons = Some(
                    rrss_core::score::parse_reasons(raw.as_deref())
                        .into_iter()
                        .filter(|r| r.weight < 0)
                        .map(ReasonView::from)
                        .collect(),
                );
            }
            Ok(article)
        })
        .collect()
}

// 只取正文首句；直接读 Markdown 事件，保留被块 IR 折成空格的 SoftBreak。
fn digest_lede(body: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let mut plain = String::new();
    let mut skip_depth = 0;
    for event in Parser::new(body) {
        match event {
            Event::Start(_) if skip_depth > 0 => skip_depth += 1,
            Event::End(_) if skip_depth > 0 => skip_depth -= 1,
            _ if skip_depth > 0 => {}
            Event::Start(Tag::Heading { .. } | Tag::CodeBlock(_) | Tag::Image { .. }) => {
                skip_depth = 1
            }
            Event::Text(text) | Event::Code(text) => plain.push_str(&text),
            Event::SoftBreak | Event::HardBreak | Event::End(TagEnd::Paragraph | TagEnd::Item) => {
                if !plain.trim().is_empty() {
                    break;
                }
            }
            _ => {}
        }
    }
    let plain = plain.trim();
    let end = plain
        .char_indices()
        .find(|(i, c)| match c {
            '。' | '？' | '！' | '?' | '!' => true,
            // 版本号、小数里的点不是句号。
            '.' => {
                !(plain[..*i].ends_with(|c: char| c.is_ascii_digit())
                    && plain[*i + 1..].starts_with(|c: char| c.is_ascii_digit()))
            }
            _ => false,
        })
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(plain.len());
    let sentence = &plain[..end];
    const MAX: usize = 96;
    match sentence.char_indices().nth(MAX) {
        Some((end, _)) => format!("{}…", &sentence[..end]),
        None => sentence.to_string(),
    }
}

pub fn ranking_settings(conn: &Connection) -> rrss_core::Result<RankingSettings> {
    let learned = db::rank_switches(conn).unwrap_or(rrss_core::rank::combine::Switches::ALL);
    Ok(RankingSettings {
        show_details: db::setting_enabled(conn, db::FOCUS_SHOW_DETAILS_KEY, false)?,
        interest_boost: interests::boost_enabled(conn).unwrap_or(true),
        rerank: interests::rerank_enabled(conn).unwrap_or(false),
        cluster_group: interests::cluster_group_enabled(conn).unwrap_or(true),
        hide_noisy: interests::hide_noisy_enabled(conn).unwrap_or(false),
        interest_ready: interests::interest_ready(conn).unwrap_or(false),
        ranking_note: interests::ranking_note(conn).unwrap_or_default(),
        hide_threshold: articles::hide_threshold(conn).unwrap_or(-10),
        theme: db::theme_preference(conn).unwrap_or_else(|_| "system".into()),
        rank_features: learned.features,
        rank_source: learned.source,
        rank_coverage: learned.coverage,
        rank_freshness: learned.freshness,
        rank_affinity: learned.affinity,
    })
}

pub fn focus_status(conn: &Connection) -> String {
    let day = ai_store::local_day();
    let used = ai_store::usage_on(conn, &day).unwrap_or_default().tokens;
    let budget = ai_store::daily_budget_tokens(conn).unwrap_or(0);
    let pending_emb = rag::pending_embed_article_count(conn).unwrap_or(0);
    let budget_label = if budget <= 0 {
        used.to_string()
    } else {
        format!("{used}/{budget}")
    };
    format!("今日 token {budget_label} · 待嵌入 {pending_emb}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::feed_scope::FeedScope;

    #[test]
    fn focus_page_exposes_spots_and_defaults_ordinary_cards_to_false() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("spot.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://spot.invalid',0);
             INSERT INTO articles(id,feed_id,title,score,created_at) VALUES
             (1,1,'one',5,0), (2,1,'two',5,0);",
        )
        .unwrap();
        conn.execute(
            "WITH RECURSIVE ids(id) AS (SELECT 100 UNION ALL SELECT id+1 FROM ids WHERE id<2100)
             INSERT INTO articles(id,feed_id,title,score,created_at)
             SELECT id,1,'hidden',-10,?1 FROM ids",
            [rrss_core::feeds::now_ms()],
        )
        .unwrap();
        let reasons = r#"[{"rule_id":1,"dimension":"title_kw","snippet":"融资","weight":-10},{"rule_id":2,"dimension":"author","snippet":"作者","weight":5},{"rule_id":3,"dimension":"title_kw","snippet":"零分","weight":0},{"rule_id":4,"dimension":"title_kw","snippet":"广告","weight":-5}]"#;
        conn.execute("UPDATE articles SET reasons_json=?1", [reasons])
            .unwrap();
        let page = focus_page(&conn, None, &[], &[], false, FeedScope::All).unwrap();
        assert_eq!(page.articles.iter().filter(|a| a.spot_check).count(), 2);
        assert!(
            page.articles
                .iter()
                .filter(|a| a.id < 100)
                .all(|a| !a.spot_check)
        );
        assert!(!page.ranking.show_details);
        let spot = page.articles.iter().find(|a| a.spot_check).unwrap();
        let hidden = spot.hide_reasons.as_ref().unwrap();
        assert_eq!(
            hidden
                .iter()
                .map(|r| (r.rule_id, r.weight))
                .collect::<Vec<_>>(),
            [(1, -10), (4, -5)]
        );
        assert_eq!(hidden[0].label, "标题：融资");
        assert!(
            page.articles
                .iter()
                .filter(|a| !a.spot_check)
                .all(|a| a.hide_reasons.is_none())
        );
        assert!(
            serde_json::to_value(spot).unwrap()["spot_check"]
                .as_bool()
                .unwrap()
        );
        // 旧快照没有这个字段时仍可读取。
        let mut old = serde_json::to_value(&page.articles[0]).unwrap();
        old.as_object_mut().unwrap().remove("spot_check");
        old.as_object_mut().unwrap().remove("hide_reasons");
        assert!(
            !serde_json::from_value::<ArticleView>(old)
                .unwrap()
                .spot_check
        );
    }

    #[test]
    fn focus_cards_include_learned_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rank.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'one',0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO article_rank VALUES(1,2,?1,2,0)",
            [r#"[{"component":"source","text":"你常读这个源"}]"#],
        )
        .unwrap();
        let page = focus_page(&conn, None, &[], &[], false, FeedScope::All).unwrap();
        assert_eq!(
            page.articles[0].rank_reasons.as_ref().unwrap()[0].component,
            "source"
        );
        assert_eq!(
            page.articles[0].rank_reasons.as_ref().unwrap()[0].text,
            "你常读这个源"
        );
        conn.execute("UPDATE article_rank SET reasons_json='invalid'", [])
            .unwrap();
        assert!(
            focus_page(&conn, None, &[], &[], false, FeedScope::All)
                .unwrap()
                .articles[0]
                .rank_reasons
                .as_ref()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn focus_digest_lede_uses_first_sentence_of_the_same_latest_digest() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        let page = || focus_page(&conn, None, &[], &[], false, FeedScope::All).unwrap();
        assert!(page().digest_banner.is_empty());
        assert!(page().digest_lede.is_empty());
        ai_store::insert_digest(&conn, "weekly", 100, "周报首句。下一句。", "[]").unwrap();
        assert_eq!(page().digest_lede, "周报首句。");
        assert!(page().digest_banner.starts_with("本周简报"));
        let long = "字".repeat(120);
        for (body, expected) in [
            (
                "# 标题\n\n**首句**含[链接](https://example.test)。第二句。",
                "首句含链接。".to_string(),
            ),
            ("# 标题\n\n首行\n次行", "首行".to_string()),
            ("标题\n====\n\n- 首条？\n- 后条", "首条？".to_string()),
            ("# 标题\n\n首句！后句", "首句！".to_string()),
            ("Rust 1.97 发布。第二句。", "Rust 1.97 发布。".to_string()),
            (
                "First sentence. Next sentence.",
                "First sentence.".to_string(),
            ),
            (long.as_str(), format!("{}…", "字".repeat(96))),
            ("", String::new()),
            ("# 只有标题\n\n## 小标题", String::new()),
            (
                "# 带 ![图](https://example.test/icon) 的**标题**",
                String::new(),
            ),
            ("只有标题\n====", String::new()),
        ] {
            ai_store::insert_digest(&conn, "daily", 1, body, "[]").unwrap();
            assert_eq!(page().digest_lede, expected, "{body}");
            assert!(page().digest_banner.starts_with("今日简报"));
        }
        ai_store::insert_digest(&conn, "daily", 2, "最新日报。后句。", "[]").unwrap();
        assert_eq!(page().digest_lede, "最新日报。");
        let more = focus_page(&conn, None, &[], &[], true, FeedScope::All).unwrap();
        assert!(more.digest_banner.is_empty());
        assert!(more.digest_lede.is_empty());
    }

    #[test]
    fn focus_cluster_members_match_expanded_focus_page() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("cluster.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,title,created_at) VALUES
             (1,'https://a.invalid/feed','A',0),(2,'https://b.invalid/feed','B',0);
             INSERT INTO clusters(id,label,member_count,first_published_at,last_published_at,created_at)
             VALUES(7,'同题',3,1000,5000,0);
             INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,created_at) VALUES
             (1,1,'early',5,1000,7,0),(2,2,'late',8,5000,7,0),
             (4,1,'hidden',-100,2000,7,0),(3,1,'other',4,3000,NULL,0);",
        )
        .unwrap();
        let views = focus_cluster_members(&conn, 7, FeedScope::All).unwrap();
        let page = articles::list_focus_page(&conn, 40, Some(7), &[], &[], FeedScope::All)
            .unwrap()
            .0;
        let page_members: Vec<_> = page
            .iter()
            .filter(|card| card.row_kind == articles::ROW_CARD && card.cluster_id == Some(7))
            .collect();
        assert_eq!(
            views
                .iter()
                .map(|view| (
                    view.id,
                    view.cluster_size,
                    view.cluster_label.as_str(),
                    view.cluster_span.as_str()
                ))
                .collect::<Vec<_>>(),
            page_members
                .iter()
                .map(|card| (
                    card.id,
                    card.cluster_size,
                    card.cluster_label.as_str(),
                    card.cluster_span.as_str()
                ))
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            views.iter().map(|view| view.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(views.iter().all(|view| view.feed_icon_href.is_empty()
            && !view.spot_check
            && view.hide_reasons.is_none()));
        assert!(
            focus_cluster_members(&conn, 99, FeedScope::All)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn folder_scope_clears_digest_and_limits_cluster_members() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("column.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO folders(id,parent_id,name,created_at) VALUES
             (1,NULL,'in',0),(3,1,'child',0),(2,NULL,'out',0);
             INSERT INTO feeds(id,url,folder_id,created_at) VALUES
             (1,'https://a.invalid/rss',1,0),(3,'https://c.invalid/rss',3,0),
             (2,'https://b.invalid/rss',2,0);
             INSERT INTO clusters(id,label,member_count,created_at) VALUES(7,'同题',3,0);
             INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,created_at) VALUES
             (1,1,'in',5,10,7,0),(3,3,'child',5,20,7,0),(2,2,'out',5,30,7,0);",
        )
        .unwrap();
        ai_store::insert_digest(&conn, "daily", 1, "栏目不显示。", "[]").unwrap();
        let page = focus_page(&conn, None, &[], &[], false, FeedScope::Folder(1)).unwrap();
        assert!(page.digest_banner.is_empty() && page.digest_lede.is_empty());
        assert_eq!(
            focus_cluster_members(&conn, 7, FeedScope::Folder(1))
                .unwrap()
                .iter()
                .map(|view| view.id)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
    }
}
