//! 阶段 23「B1 基础层」「B2 增强层」「接入方式」：只读加载与学习，排序作业与回放共用。

use crate::{
    Result,
    features::{FeatureChip, extract_features_with_bigrams},
    rank::{
        affinity,
        combine::{self, Input, Switches},
        coverage, evidence,
        features::Profile,
        freshness,
        labels::{self, LabelExposure},
        source,
    },
    signals::{ArticleEvent, EventKind, load_events, rows},
};

use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};

struct Article {
    title: String,
    feed: i64,
    chips: Vec<FeatureChip>,
    published: Option<i64>,
    cluster: Option<i64>,
    created: i64,
}

pub struct Model {
    affinity: affinity::Profile,
    recent_affinity: affinity::Profile,
    affinity_epoch: String,
    vectors: BTreeMap<i64, Vec<f32>>,
    articles: BTreeMap<i64, Article>,
    features: Profile,
    sources: BTreeMap<i64, f64>,
    fitted: Option<super::read_model::Fit<5>>,
    fitted_before: i64,
    half_lives: BTreeMap<i64, i64>,
    /// Freshness policy and, under `Shelf`, per-feed shelf life and quiet feeds.
    policy: freshness::Policy,
    windows: BTreeMap<i64, i64>,
    exempt: BTreeSet<i64>,
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
        Self::load_with_fitted(
            conn,
            before,
            enabled,
            scope,
            super::store::fitted_enabled(conn)?,
        )
    }

    pub(super) fn load_with_fitted(
        conn: &Connection,
        before: i64,
        enabled: bool,
        scope: Option<&[i64]>,
        fitted_enabled: bool,
    ) -> Result<Self> {
        // From the requested model, not the stored setting: a switch computes its
        // scores before it writes the setting. Too little data falls back to the
        // exact manual model; the Focus pool still applies the shelf life.
        let policy = freshness::policy(fitted_enabled);
        let mut articles: BTreeMap<i64, Article> = rows(
            conn,
            "SELECT id,feed_id,author,COALESCE(title,''),canonical_url,published_at,cluster_id,created_at
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
                        created: r.get(7)?,
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
        let start = if fitted_enabled {
            i64::MIN
        } else {
            before.saturating_sub(crate::signals::RETENTION_MS)
        };
        let sessions = crate::signals::load_session_summaries(conn, start, before)?;
        // Source opportunities only come from the global ranked Focus stream.
        let exposures = rows(
            conn,
            "SELECT e.id,e.article_id,e.position,e.shown_at,a.feed_id FROM exposures e
             JOIN articles a ON a.id=e.article_id WHERE a.expires_at IS NULL
             AND e.shown_at>=?1 AND e.shown_at<?2 AND e.surface='focus' ORDER BY e.shown_at,e.id",
            start,
            before,
            |r| {
                Ok((
                    LabelExposure {
                        id: r.get(0)?,
                        article_id: r.get(1)?,
                        position: r.get(2)?,
                        shown_at: r.get(3)?,
                    },
                    r.get(4)?,
                ))
            },
        )?;
        let events = load_events(conn, start, before)?;
        let legacy_sql = format!("SELECT f.article_id,f.kind,f.created_at FROM feedback f JOIN articles a ON a.id=f.article_id
             WHERE a.expires_at IS NULL AND f.created_at>=?1 AND f.created_at<?2
             {}", if fitted_enabled { "" } else {
                "AND NOT EXISTS (SELECT 1 FROM article_events e WHERE e.article_id=f.article_id
                 AND e.kind IN ('up','down','star','unstar','cite','ask') AND e.at<?2)"
             });
        let legacy = rows(conn, &legacy_sql, start, before, |r| {
            Ok(ArticleEvent {
                article_id: r.get(0)?,
                kind: r.get(1)?,
                exposure_id: None,
                at: r.get(2)?,
            })
        })?;
        let mut training_events = legacy;
        training_events.extend(events.iter().cloned());
        let mut votes = labels::evidence(&training_events, &sessions, &[], 0);
        if fitted_enabled {
            fitted::add_skips(&mut votes, &exposures, &sessions, &training_events);
        }
        let wanted = scope.map(|ids| {
            ids.iter()
                .copied()
                .chain(votes.iter().map(|v| v.article_id))
                .chain(
                    exposures
                        .iter()
                        .filter(|_| fitted_enabled)
                        .map(|(e, _)| e.article_id),
                )
                .chain(
                    sessions
                        .iter()
                        .filter(|_| fitted_enabled)
                        .map(|s| s.article_id),
                )
                .chain(
                    training_events
                        .iter()
                        .filter(|_| fitted_enabled)
                        .map(|e| e.article_id),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        });
        let vectors = if enabled {
            crate::rag::normalized_title_vectors(conn, wanted.as_deref())?
        } else {
            BTreeMap::new()
        };
        let fitted = if fitted_enabled {
            let subscribed: BTreeMap<i64, i64> = rows(
                conn,
                "SELECT id,created_at FROM feeds WHERE created_at>=?1 AND created_at<?2",
                before - fitted::SUBSCRIPTION_WINDOW,
                before,
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
            .into_iter()
            .collect();
            let fit = fitted::train(
                &articles,
                &vectors,
                &exposures,
                &sessions,
                &training_events,
                &subscribed,
                before,
                policy,
            );
            if fit.is_none() {
                return Self::load_with_fitted(conn, before, enabled, scope, false);
            }
            fit
        } else {
            None
        };
        let (feature_profile, affinity, recent_affinity) =
            fitted::profiles(&articles, &vectors, &votes, before);
        let space_id = if enabled && !vectors.is_empty() {
            crate::vector_space::active(conn)?.map(|space| space.id)
        } else {
            None
        };
        // Do not infer entry point from the frontend's launch-wide exposure map.
        // Old sessions without a valid link remain useful for topic learning only.
        let contexts: BTreeMap<(i64, i64), Option<i64>> = rows(conn,
            "SELECT r.article_id,r.opened_at,CASE WHEN r.via='focus' AND e.surface='focus'
                 AND e.shown_at<=r.opened_at AND r.opened_at-e.shown_at<=86400000
                 THEN e.id ELSE NULL END
             FROM read_sessions r LEFT JOIN exposures e ON e.id=r.exposure_id AND e.article_id=r.article_id
             WHERE r.opened_at>=?1 AND r.opened_at<?2 ORDER BY r.opened_at,r.id",
            start, before, |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?.into_iter().collect();
        let attributed = |article, at| {
            contexts
                .range((article, i64::MIN)..=(article, at))
                .next_back()
                .filter(|((_, opened), _)| at.saturating_sub(*opened) <= evidence::DAY_MS)
                .and_then(|(_, id)| *id)
        };
        let source_sessions: Vec<_> = sessions
            .iter()
            .filter(|s| {
                contexts
                    .get(&(s.article_id, s.opened_at))
                    .copied()
                    .flatten()
                    .is_some()
            })
            .cloned()
            .collect();
        // Resolve explicit state over the complete event sequence. Unstar/down
        // from another entry point must revoke an earlier attributable reward.
        let source_events: Vec<_> = events
            .iter()
            .filter(|e| {
                e.kind != EventKind::OpenOriginal
                    || (e.exposure_id.is_some() && attributed(e.article_id, e.at) == e.exposure_id)
            })
            .cloned()
            .collect();
        let source_votes = labels::evidence(&source_events, &source_sessions, &[], 0);
        let source_feedback: Vec<_> = source_votes
            .iter()
            .filter_map(|v| {
                let exposure_id = if v.explicit {
                    let id = v.exposure_id?;
                    (attributed(v.article_id, v.at) == Some(id)).then_some(id)?
                } else {
                    attributed(v.article_id, v.at)?
                };
                Some(source::Feedback {
                    evidence: *v,
                    exposure_id,
                    features: &articles.get(&v.article_id)?.chips,
                })
            })
            .collect();
        let sources = source::learn(&exposures, &source_feedback, before);
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
        // Per-feed publishing rhythm, a fitted-only feature.
        let half_lives = if fitted_enabled {
            fitted::intervals(&articles, before)
        } else {
            BTreeMap::new()
        };
        let (windows, exempt) = if matches!(policy, freshness::Policy::Shelf { .. }) {
            let rates = freshness::rates(
                articles.values().map(|a| {
                    (
                        a.feed,
                        a.published
                            .filter(|p| *p > 0)
                            .map_or(a.created, |p| p.min(a.created)),
                    )
                }),
                before,
            );
            (
                rates
                    .iter()
                    .map(|(f, r)| (*f, freshness::window_for_rate(*r)))
                    .collect(),
                rates
                    .iter()
                    .filter(|(_, r)| freshness::exempt(**r))
                    .map(|(f, _)| *f)
                    .collect(),
            )
        } else {
            (BTreeMap::new(), BTreeSet::new())
        };
        Ok(Self {
            policy,
            windows,
            exempt,
            affinity_epoch: affinity.signature(space_id),
            affinity,
            recent_affinity,
            features: feature_profile,
            fitted,
            fitted_before: before,
            vectors,
            sources,
            half_lives,
            cluster_feeds,
            articles,
        })
    }

    #[cfg(test)]
    pub(crate) fn article_boost(
        &self,
        id: i64,
        at: i64,
        switches: Switches,
    ) -> Option<combine::Boost> {
        self.article_rank(id, at, switches).map(|(boost, _)| boost)
    }

    pub(crate) fn article_rank(
        &self,
        id: i64,
        at: i64,
        switches: Switches,
    ) -> Option<(combine::Boost, Option<super::topics::Evidence>)> {
        let article = self.articles.get(&id)?;
        Some(self.boost_record(
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

    /// Evaluation only: unrounded points per component in switch order
    /// (features, source, coverage, freshness, affinity) from the model a full
    /// rebuild at `now` loads with `fitted` requested; `true` when a fit was used.
    #[cfg(feature = "evaluation-clock")]
    pub fn evaluation_points(
        conn: &Connection,
        now: i64,
        fitted: bool,
        ids: &[i64],
    ) -> Result<(bool, BTreeMap<i64, [f64; 5]>)> {
        let on = crate::rank::store::switches(conn)?;
        let model = Self::load_with_fitted(conn, now.saturating_add(1), on.affinity, None, fitted)?;
        let only = |k: usize| Switches {
            features: on.features && k == 0,
            source: on.source && k == 1,
            coverage: on.coverage && k == 2,
            freshness: on.freshness && k == 3,
            affinity: on.affinity && k == 4,
        };
        let points = ids
            .iter()
            .filter(|id| model.articles.contains_key(id))
            .map(|&id| {
                let part = |k| {
                    model
                        .article_rank(id, now, only(k))
                        .map_or(0.0, |(b, _)| b.raw_score)
                };
                (id, std::array::from_fn(part))
            })
            .collect();
        Ok((model.fitted.is_some(), points))
    }

    fn boost_record(
        &self,
        identity: (i64, i64),
        chips: &[FeatureChip],
        cluster: Option<i64>,
        published: Option<i64>,
        at: i64,
        switches: Switches,
    ) -> (combine::Boost, Option<super::topics::Evidence>) {
        let (id, feed) = identity;
        let (affinity, representative, semantic) = self
            .vectors
            .get(&id)
            .filter(|_| switches.affinity)
            .map(|v| {
                let long = self.affinity.score(v);
                let recent = self.recent_affinity.score(v);
                (
                    affinity::combined_score(long.0, recent.0),
                    long.1.or(recent.1),
                    long.1
                        .and_then(|id| self.affinity.semantic_representative(id))
                        .filter(|_| {
                            long.0
                                * self
                                    .fitted
                                    .as_ref()
                                    .map_or(combine::AFFINITY_WEIGHT, |f| f.report.beta[1])
                                >= super::topics::MIN_SUPPORT
                        }),
                )
            })
            .unwrap_or_default();
        let title = representative
            .and_then(|id| self.articles.get(&id))
            .map(|a| a.title.as_str());
        let features = self.features.weighted(chips);
        let feeds = cluster.and_then(|c| self.cluster_feeds.get(&c));
        // Shelf life: quiet feeds never age; others pay an unbounded cost.
        let shelf = match self.policy {
            freshness::Policy::Shelf { quadratic } if !self.exempt.contains(&feed) => Some((
                self.windows
                    .get(&feed)
                    .copied()
                    .unwrap_or(30 * evidence::DAY_MS),
                quadratic,
            )),
            _ => None,
        };
        let shelf_on = matches!(self.policy, freshness::Policy::Shelf { .. });
        let input = Input {
            affinity: (affinity, title),
            features: &features,
            source: self.sources.get(&feed).copied().unwrap_or(0.0),
            coverage: coverage::score(feeds.map(Vec::as_slice).unwrap_or(&[])),
            freshness: match shelf {
                Some((window, quadratic)) => {
                    freshness::shelf_cost(published, at, window, quadratic)
                }
                None if shelf_on => 0.0,
                None => freshness::penalty(published, at, fitted::ABSOLUTE_HALF_LIFE),
            },
        };
        let mut topic_contributions = None;
        let contributions = self.fitted.as_ref().map(|fit| {
            let interval = self
                .half_lives
                .get(&feed)
                .copied()
                .unwrap_or(14 * evidence::DAY_MS);
            let published = published.or_else(|| self.articles.get(&id).map(|a| a.created));
            let raw = if shelf_on {
                [
                    self.features.raw_score(chips),
                    affinity,
                    shelf.map_or(0.0, |(window, quadratic)| {
                        -freshness::shelf_cost(published, self.fitted_before, window, quadratic)
                    }),
                    input.coverage.1,
                    0.0,
                ]
            } else {
                [
                    self.features.raw_score(chips),
                    affinity,
                    freshness::value(published, self.fitted_before, interval),
                    input.coverage.1,
                    freshness::value(published, self.fitted_before, fitted::ABSOLUTE_HALF_LIFE),
                ]
            };
            let (c, source) = fit.contributions(feed, raw);
            let source = source + fitted::source_draw(fit, feed, at);
            topic_contributions = Some((c[0], c[1]));
            [c[0], source, c[3], c[2] + c[4], c[1]]
        });
        let boost = combine::with_contributions(input, switches, contributions);
        let mut evidence = super::topics::Evidence {
            raw_score: boost.raw_score,
            features: if switches.features {
                features
                    .iter()
                    .map(|(_, w)| w)
                    .sum::<f64>()
                    .clamp(-combine::MAX_FEATURE_SCORE, combine::MAX_FEATURE_SCORE)
            } else {
                0.0
            },
            affinity: affinity * combine::AFFINITY_WEIGHT,
            lexical: if switches.features {
                self.features.reliable_keys(&features)
            } else {
                Vec::new()
            },
            semantic: semantic.map(|id| (self.affinity_epoch.clone(), id)),
        };
        if let Some((lexical, vector)) = topic_contributions {
            evidence.features = if switches.features { lexical } else { 0.0 };
            evidence.affinity = if switches.affinity { vector } else { 0.0 };
            let slope = self.fitted.as_ref().unwrap().report.beta[0];
            evidence.lexical.retain_mut(|(_, weight)| {
                *weight *= slope;
                *weight > 0.0
            });
        }
        let evidence = evidence
            .allocation(switches, true)
            .eligible()
            .then_some(evidence);
        (boost, evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::Surface;

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
    fn default_learning_score_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("snapshot.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://snapshot.example/rss',0);
            INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
            VALUES(1,'','fixture',2,1,0,'active',0);").unwrap();
        for id in 1..=12 {
            affinity_article(
                &conn,
                id,
                if id <= 10 {
                    "orbital mechanics"
                } else {
                    "harbor sailing"
                },
                ("fixture", 1),
                if id <= 10 { &[1.0, 0.0] } else { &[0.0, 1.0] },
                (id <= 10).then_some(if id <= 8 { "up" } else { "down" }),
            );
        }
        conn.execute("UPDATE articles SET published_at=0", [])
            .unwrap();
        let model = Model::load(&conn, evidence::DAY_MS).unwrap();
        let snapshot: Vec<_> = [1, 9, 11]
            .into_iter()
            .map(|id| {
                let boost = model
                    .article_boost(id, evidence::DAY_MS, Switches::ALL)
                    .unwrap();
                (id, boost.score, boost.raw_score.to_bits())
            })
            .collect();
        assert_eq!(
            snapshot,
            vec![
                (1, 3, 4614801218869968552),
                (9, 3, 4614801218869968552),
                (11, 0, 13824476378647119628),
            ]
        );
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
                .boost_record((1, 1), &chips, None, None, 2, Switches::ALL)
                .0
                .score,
            4
        );
        let future = Model::load(&conn, 1).unwrap();
        assert_eq!(
            future
                .boost_record((1, 1), &chips, None, None, 1, Switches::ALL)
                .0
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
        // The one-day implicit burst remains recent only. The explicit down
        // stays independent, so it can outweigh the bounded short-term reading.
        assert_eq!(weighted.len(), 2);
        assert!(weighted.iter().all(|(_, weight)| *weight < 0.0));
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

#[cfg(test)]
#[path = "learning_tests.rs"]
mod learning_tests;

#[cfg(test)]
#[path = "behavioral_eval_tests.rs"]
mod behavioral_eval_tests;

#[cfg(test)]
#[path = "fitted_tests.rs"]
mod fitted_tests;

#[path = "model_fitted.rs"]
mod fitted;
#[cfg(any(test, feature = "evaluation-clock"))]
pub use fitted::{with_fine_positions, with_source_draw, with_source_penalty};
