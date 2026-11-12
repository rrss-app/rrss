//! 阶段 23「B1 基础层」「B2 增强层」「接入方式」：只读加载与学习，排序作业与回放共用。

use crate::{
    Result,
    features::{FeatureChip, extract_features_with_bigrams},
    rank::{
        affinity,
        combine::{self, Input, Switches},
        coverage,
        features::{Feedback, Profile},
        freshness,
        labels::{self, LabelExposure},
        source,
    },
    signals::{ArticleEvent, EventKind, Surface, load_events, rows},
};

use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};

struct Article {
    title: String,
    feed: i64,
    chips: Vec<FeatureChip>,
    published: Option<i64>,
    cluster: Option<i64>,
}

pub struct Model {
    affinity: affinity::Profile,
    vectors: BTreeMap<i64, Vec<f32>>,
    articles: BTreeMap<i64, Article>,
    features: Profile,
    sources: BTreeMap<i64, f64>,
    half_lives: BTreeMap<i64, i64>,
    cluster_feeds: BTreeMap<i64, Vec<i64>>,
}

impl Model {
    pub fn load(conn: &Connection, before: i64) -> Result<Self> {
        Self::load_with_affinity(conn, before, true, None)
    }

    pub(crate) fn load_with_affinity(
        conn: &Connection,
        before: i64,
        enabled: bool,
        scope: Option<&[i64]>,
    ) -> Result<Self> {
        let mut articles: BTreeMap<i64, Article> = rows(
            conn,
            "SELECT id,feed_id,author,COALESCE(title,''),canonical_url,published_at,cluster_id
             FROM articles WHERE expires_at IS NULL AND created_at<?2",
            0,
            before,
            |r| {
                let author: Option<String> = r.get(2)?;
                let title: String = r.get(3)?;
                let url: Option<String> = r.get(4)?;
                Ok((
                    r.get(0)?,
                    Article {
                        feed: r.get(1)?,
                        chips: extract_features_with_bigrams(
                            author.as_deref(),
                            &[],
                            &title,
                            url.as_deref(),
                            None,
                            true,
                        ),
                        title,
                        published: r.get(5)?,
                        cluster: r.get(6)?,
                    },
                ))
            },
        )?
        .into_iter()
        .collect();
        // 一次查询所有候选文章标签，避免按篇执行数万次 SQL。
        for (id, tag) in rows(
            conn,
            "SELECT t.article_id,t.tag FROM article_tags t JOIN articles a ON a.id=t.article_id
             WHERE a.expires_at IS NULL AND a.created_at<?2 ORDER BY t.article_id,t.tag",
            0,
            before,
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )? {
            if let Some(article) = articles.get_mut(&id) {
                article.chips.extend(extract_features_with_bigrams(
                    None,
                    &[tag],
                    "",
                    None,
                    None,
                    true,
                ));
            }
        }
        let start = before.saturating_sub(crate::signals::RETENTION_MS);
        let sessions = crate::signals::load_session_summaries(conn, start, before)?;
        // 订阅时间线与焦点栏目的曝光不是今日焦点的排序结果，未打开也不计入源打开率。
        // 行为日志不按 surface 过滤，照常参与训练。
        let exposure_sql = format!(
            "SELECT e.id,e.article_id,e.position,e.shown_at,a.feed_id FROM exposures e
             JOIN articles a ON a.id=e.article_id
             WHERE a.expires_at IS NULL AND e.shown_at>=?1 AND e.shown_at<?2
               AND e.surface<>'{}' AND e.surface<>'{}'
             ORDER BY e.shown_at,e.id",
            Surface::Feeds.as_str(),
            Surface::FocusColumn.as_str()
        );
        let exposures = rows(conn, &exposure_sql, start, before, |r| {
            Ok((
                LabelExposure {
                    id: r.get(0)?,
                    article_id: r.get(1)?,
                    position: r.get(2)?,
                    shown_at: r.get(3)?,
                },
                r.get(4)?,
            ))
        })?;
        let events = load_events(conn, start, before)?;
        // B1 不需要位置权重；给无曝光但有行为的文章提供中性虚拟曝光，
        // 仍由 labels::generate 统一显式优先、unstar 和阅读分档，不另定义正负。
        let synthetic: Vec<_> = articles
            .keys()
            .map(|&id| LabelExposure {
                id,
                article_id: id,
                position: 0,
                shown_at: i64::MIN,
            })
            .collect();
        let mut votes = labels::generate(&synthetic, &events, &sessions, &[], 0);
        let legacy = rows(
            conn,
            "SELECT f.article_id,f.kind,f.created_at FROM feedback f JOIN articles a ON a.id=f.article_id WHERE a.expires_at IS NULL AND f.created_at<?2 AND NOT EXISTS (SELECT 1 FROM article_events e WHERE e.article_id=f.article_id AND e.at<?2) AND NOT EXISTS (SELECT 1 FROM read_sessions r WHERE r.article_id=f.article_id AND r.opened_at<?2)",
            0,
            before,
            |r| {
                Ok(ArticleEvent {
                    article_id: r.get(0)?,
                    kind: r.get(1)?,
                    exposure_id: None,
                    at: r.get(2)?,
                })
            },
        )?;
        votes.extend(labels::generate(&synthetic, &legacy, &[], &[], 0));
        let wanted = scope.map(|ids| {
            ids.iter()
                .copied()
                .chain(votes.iter().map(|v| v.article_id))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        });
        let vectors = if enabled {
            crate::rag::normalized_title_vectors(conn, wanted.as_deref())?
        } else {
            BTreeMap::new()
        };
        let affinity_feedback: Vec<_> = votes
            .iter()
            .filter_map(|v| {
                Some(affinity::Feedback {
                    id: v.article_id,
                    vector: vectors.get(&v.article_id)?,
                    weight: v.feedback_weight,
                })
            })
            .collect();
        let affinity = affinity::Profile::learn(&affinity_feedback);
        let corpus: Vec<_> = articles.values().map(|a| a.chips.clone()).collect();
        let feedback: Vec<_> = votes
            .iter()
            .filter_map(|v| {
                articles.get(&v.article_id).map(|a| Feedback {
                    features: &a.chips,
                    weight: v.feedback_weight,
                    at: v.at,
                })
            })
            .collect();
        let mut first_open = BTreeMap::<i64, i64>::new();
        for (id, at) in events
            .iter()
            .filter(|e| e.kind == EventKind::Open)
            .map(|e| (e.article_id, e.at))
            .chain(sessions.iter().map(|s| (s.article_id, s.opened_at)))
        {
            first_open
                .entry(id)
                .and_modify(|old| *old = (*old).min(at))
                .or_insert(at);
        }
        let delays: Vec<_> = first_open
            .into_iter()
            .filter_map(|(id, at)| {
                let a = articles.get(&id)?;
                let published = a.published?;
                (at >= published).then_some((a.feed, at.saturating_sub(published)))
            })
            .collect();
        let mut cluster_feeds = BTreeMap::<i64, Vec<i64>>::new();
        for (cluster, feed) in rows(
            conn,
            "SELECT cluster_id,feed_id FROM articles WHERE expires_at IS NULL AND cluster_id IS NOT NULL AND created_at<?2",
            0,
            before,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )? {
            cluster_feeds.entry(cluster).or_default().push(feed);
        }
        Ok(Self {
            affinity,
            vectors,
            features: Profile::learn(&corpus, &feedback, before),
            sources: source::learn(&exposures, &events, before),
            half_lives: freshness::learn(&delays),
            cluster_feeds,
            articles,
        })
    }

    pub(crate) fn article_boost(
        &self,
        id: i64,
        at: i64,
        switches: Switches,
    ) -> Option<combine::Boost> {
        let article = self.articles.get(&id)?;
        Some(self.boost(
            (id, article.feed),
            &article.chips,
            article.cluster,
            article.published,
            at,
            switches,
        ))
    }

    pub(crate) fn article_ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.articles.keys().copied()
    }

    pub(crate) fn boost(
        &self,
        identity: (i64, i64),
        chips: &[FeatureChip],
        cluster: Option<i64>,
        published: Option<i64>,
        at: i64,
        switches: Switches,
    ) -> combine::Boost {
        let (id, feed) = identity;
        let (affinity, representative) = self
            .vectors
            .get(&id)
            .filter(|_| switches.affinity)
            .map(|v| self.affinity.score(v))
            .unwrap_or_default();
        let title = representative
            .and_then(|id| self.articles.get(&id))
            .map(|a| a.title.as_str());
        let features = self.features.weighted(chips);
        let feeds = cluster.and_then(|c| self.cluster_feeds.get(&c));
        combine::combine(
            Input {
                affinity: (affinity, title),
                features: &features,
                source: self.sources.get(&feed).copied().unwrap_or(0.0),
                coverage: coverage::score(feeds.map(Vec::as_slice).unwrap_or(&[])),
                freshness: freshness::penalty(
                    published,
                    at,
                    self.half_lives
                        .get(&feed)
                        .copied()
                        .unwrap_or(freshness::DEFAULT_HALF_LIFE_MS),
                ),
            },
            switches,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn affinity_article(
        conn: &Connection,
        id: i64,
        title: &str,
        space: (&str, i64),
        vector: &[f32],
        feedback: Option<&str>,
    ) {
        let article_sql = "INSERT INTO articles(id,feed_id,title,created_at) VALUES(?1,1,?2,0)";
        conn.execute(article_sql, rusqlite::params![id, title])
            .unwrap();
        conn.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
            VALUES(?1,0,'title_summary','',?2,?3,?4)",
            rusqlite::params![id, crate::rag::f32s_to_le(vector), space.0, space.1],
        )
        .unwrap();
        if let Some(kind) = feedback {
            let feedback_sql = "INSERT INTO feedback(article_id,kind,created_at) VALUES(?1,?2,1)";
            conn.execute(feedback_sql, rusqlite::params![id, kind])
                .unwrap();
        }
    }

    #[test]
    fn scoped_vectors_and_incremental_ranks_match_full_rebuild() {
        use crate::{db, rank::store};
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("scoped-rank.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid',0);
            INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
            VALUES(1,'','fixture',2,1,0,'active',0);").unwrap();
        for id in 1..=600 {
            affinity_article(
                &conn,
                id,
                &format!("Article {id}"),
                ("fixture", 1),
                if id % 2 == 0 {
                    &[1.0, 0.0]
                } else {
                    &[0.0, 1.0]
                },
                (id <= 20).then_some(if id <= 15 { "up" } else { "down" }),
            );
        }
        // 跨两批且候选与反馈有重叠；无关的有向量文章不能读入。
        let ids: Vec<_> = (101..=550).chain([1]).collect();
        let scoped = Model::load_with_affinity(&conn, 3, true, Some(&ids)).unwrap();
        let expected: BTreeSet<_> = (1..=20).chain(ids.iter().copied()).collect();
        assert_eq!(
            scoped.vectors.keys().copied().collect::<BTreeSet<_>>(),
            expected
        );
        assert_eq!(Model::load(&conn, 3).unwrap().vectors.len(), 600);
        assert_eq!(
            Model::load_with_affinity(&conn, 3, true, Some(&[]))
                .unwrap()
                .vectors
                .len(),
            20
        );
        let ranks = || {
            conn.prepare(
                "SELECT article_id,boost,reasons_json FROM article_rank ORDER BY article_id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        assert_eq!(store::rebuild(&conn, 2).unwrap(), 600);
        let full = ranks();
        conn.execute("UPDATE article_rank SET boost=999,reasons_json='[]' WHERE article_id BETWEEN 101 AND 550 OR article_id=1",[]).unwrap();
        assert_eq!(store::update(&conn, &ids, 2).unwrap(), 451);
        assert_eq!(ranks(), full);
        // 版本升级的 update 会改为整表重建，也必须加载所有候选向量。
        conn.execute(
            "UPDATE article_rank SET algo_version=0 WHERE article_id=101",
            [],
        )
        .unwrap();
        assert_eq!(store::update(&conn, &[101], 2).unwrap(), 600);
        assert_eq!(ranks(), full);
    }

    #[test]
    fn affinity_requires_active_title_summary_vectors_and_never_crosses_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("affinity.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://affinity.invalid',0);
            INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
            VALUES(1,'','a',2,1,0,'retired',0),(2,'','b',2,1,0,'retired',0);").unwrap();
        for id in 1..=11 {
            affinity_article(
                &conn,
                id,
                "liked",
                ("a", if id <= 10 { 1 } else { 2 }),
                &[1.0, 0.0],
                (id <= 10).then_some("up"),
            );
        }
        let only_affinity = Switches {
            features: false,
            source: false,
            coverage: false,
            freshness: false,
            affinity: true,
        };
        let boost = |id| {
            Model::load(&conn, 2)
                .unwrap()
                .article_boost(id, 2, only_affinity)
                .unwrap()
        };
        assert_eq!(boost(11).score, 0); // 没有生效空间。
        conn.execute("UPDATE vector_spaces SET state='active' WHERE id=1", [])
            .unwrap();
        assert_eq!(boost(11).score, 0); // 同维度但不同空间。
        assert_eq!(boost(1).score, 2);
        conn.execute("UPDATE chunks SET kind='body' WHERE article_id<=10", [])
            .unwrap();
        assert_eq!(boost(1).score, 0); // 不回退到正文段。
        conn.execute(
            "UPDATE chunks SET kind='title_summary',space_id=1 WHERE article_id=11",
            [],
        )
        .unwrap();
        assert_eq!(boost(11).score, 0); // 只有候选向量，历史正反馈不足。
    }

    #[test]
    fn loads_tags_in_bulk_and_learns_before_cutoff_only() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("rank.db")).unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0)",
            [],
        )
        .unwrap();
        for id in 1..=10 {
            conn.execute(
                "INSERT INTO articles(id,feed_id,title,created_at) VALUES(?1,1,'',0)",
                [id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO article_tags(article_id,tag) VALUES(?1,'Rust'),(?1,'systems')",
                [id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feedback(article_id,kind,created_at) VALUES(?1,'up',1)",
                [id],
            )
            .unwrap();
        }
        let chips = extract_features_with_bigrams(
            None,
            &["Rust".into(), "systems".into()],
            "",
            None,
            None,
            true,
        );
        let model = Model::load(&conn, 2).unwrap();
        assert_eq!(
            model
                .boost((1, 1), &chips, None, None, 2, Switches::ALL)
                .score,
            4
        );
        let future = Model::load(&conn, 1).unwrap();
        assert_eq!(
            future
                .boost((1, 1), &chips, None, None, 1, Switches::ALL)
                .score,
            0
        );
        conn.execute_batch(
            "DELETE FROM feedback;
             INSERT INTO read_sessions(article_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,
                 max_block,reached_end,translated,ended_by,via)
             SELECT id,1,CASE WHEN id<=5 THEN 200000 ELSE 1000 END,0,
                 CASE WHEN id<=5 THEN 900 ELSE 0 END,0,1000,0,1,0,'back','focus' FROM articles;
             INSERT INTO article_events(article_id,kind,at) VALUES(5,'down',1);",
        )
        .unwrap();
        let weighted = Model::load(&conn, 2).unwrap().features.weighted(&chips);
        // 4 个细读、5 个跳出，另一个细读被显式踩覆盖；只衰减一次，不混入位置 IPW。
        let decay = 2_f64.powf(-1.0 / crate::rank::features::HALF_LIFE_MS as f64);
        let expected = ((8.0 * decay + 1.0) / (2.25 * decay + 1.0)).ln();
        assert_eq!(weighted.len(), 2);
        assert!(
            weighted
                .iter()
                .all(|(_, weight)| (*weight - expected).abs() < 1e-10)
        );
    }

    #[test]
    fn feeds_exposures_do_not_change_source_preference() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("feeds-source.db")).unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://src.example/rss',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'a',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
             VALUES(1,?1,0,0,0,0,'baseline','focus',1)",
            [Surface::Focus.as_str()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO article_events(article_id,kind,at) VALUES(1,'open',1)",
            [],
        )
        .unwrap();
        let baseline = Model::load(&conn, 3).unwrap().sources;
        assert!(baseline.contains_key(&1));
        for session in ["f1", "f2", "f3", "f4"] {
            conn.execute(
                "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                 VALUES(1,?1,0,0,0,0,'time',?2,2)",
                rusqlite::params![Surface::Feeds.as_str(), session],
            )
            .unwrap();
        }
        assert_eq!(Model::load(&conn, 3).unwrap().sources, baseline);
        for session in ["c1", "c2", "c3", "c4"] {
            conn.execute(
                "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                 VALUES(1,?1,0,0,0,0,'baseline',?2,2)",
                rusqlite::params![Surface::FocusColumn.as_str(), session],
            )
            .unwrap();
        }
        assert_eq!(Model::load(&conn, 3).unwrap().sources, baseline);
    }
}
