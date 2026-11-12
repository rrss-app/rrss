//! 阶段 23「M2 离线回放」「阅读会话」「指标与读数」：只读快照与内存重排。

use std::{collections::BTreeMap, fmt::Write, path::Path};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::metrics::{self, Sample};
use crate::{
    Result, interests,
    rank::{
        engagement, focus,
        labels::{self, Label, LabelExposure},
        model::Model,
    },
    score,
    signals::{load_events, load_session_summaries, rows},
};

pub const MIN_LABELS: usize = 50;
pub const WINDOW_MS: i64 = 30 * 86_400_000;

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub rules: bool,
    pub interests: bool,
    pub b1: bool,
    pub b2: bool,
    pub b1_components: crate::rank::combine::Switches,
    pub hide_threshold: i64,
}

#[derive(Debug)]
pub struct Reading {
    pub auc: Option<f64>,
    pub ndcg: Option<f64>,
}

#[derive(Debug)]
pub struct Outcome {
    pub config: Config,
    pub labels: usize,
    pub hidden_positives: usize,
    pub ranked: Option<Reading>,
    pub chronological: Option<Reading>,
}

struct Candidate {
    label: Label,
    published_at: i64,
    rule_score: i64,
    interest_boost: i64,
    features: Vec<crate::features::FeatureChip>,
    feed_id: i64,
    cluster_id: Option<i64>,
    published: Option<i64>,
}

pub struct Replay {
    candidates: Vec<Candidate>,
    b1: Model,
    since: i64,
    until: i64,
}

/// 不使用 db::open_at：不迁移、不设置 WAL、不初始化 settings。
pub fn open_read_only(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

impl Replay {
    /// UTC 半开窗口 [since, until)，只评测焦点曝光，避免混合 surface 的位置概率。
    /// 重算使用当前规则 / 兴趣 / 文章字段；E0 不保留历史配置和历史正文。
    pub fn load(conn: &Connection, since: i64, until: i64) -> Result<Self> {
        let tx = conn.unchecked_transaction()?;
        let exposures = rows(
            &tx,
            "SELECT e.id,e.article_id,e.position,e.shown_at FROM exposures e
            JOIN articles a ON a.id=e.article_id
            WHERE a.expires_at IS NULL AND e.surface='focus' AND e.shown_at>=?1 AND e.shown_at<?2 ORDER BY e.shown_at,e.id",
            since,
            until,
            |r| {
                Ok(LabelExposure {
                    id: r.get(0)?,
                    article_id: r.get(1)?,
                    position: r.get(2)?,
                    shown_at: r.get(3)?,
                })
            },
        )?;
        let events = load_events(&tx, since, until)?;
        let sessions = load_session_summaries(&tx, since, until)?;
        let training_sessions = load_session_summaries(
            &tx,
            since.saturating_sub(crate::signals::RETENTION_MS),
            since,
        )?;
        let history = engagement::merge_daily_sessions(&training_sessions, 0);
        let labels = labels::generate(&exposures, &events, &sessions, &history, 0);
        let b1 = Model::load(&tx, since)?;
        let compiled = score::load_compiled_rules(&tx)?;
        let prototypes = interests::load_prototypes(&tx)?;
        let mut candidates = Vec::new();
        let mut folder_cache = BTreeMap::new();
        for label in labels {
            let Some(article) = tx.query_row(
                "SELECT id,feed_id,title,author,canonical_url,content_html,published_at,cluster_id
                FROM articles WHERE expires_at IS NULL AND id=?1 AND EXISTS (SELECT 1 FROM feeds WHERE feeds.id=articles.feed_id)",
                [label.article_id],
                |r| {
                    // 延续窗口加载的容错：字段转换失败时跳过文章。
                    let fields = (|| -> rusqlite::Result<_> { Ok(ArticleFields {
                        id: r.get(0)?,
                        feed_id: r.get(1)?,
                        title: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        author: r.get(3)?,
                        canonical_url: r.get(4)?,
                        content_html: r.get(5)?,
                        published_at: r.get(6)?,
                        cluster_id: r.get(7)?,
                    }) })();
                    Ok(fields.ok())
                },
            ).optional()?.flatten() else {
                continue;
            };
            let folder: Option<i64> = tx.query_row(
                "SELECT folder_id FROM feeds WHERE id=?1",
                [article.feed_id],
                |r| r.get(0),
            )?;
            if let std::collections::btree_map::Entry::Vacant(entry) = folder_cache.entry(folder) {
                entry.insert(score::folder_chain(&tx, folder)?);
            }
            let tags = score::tags_for_article(&tx, article.id)?;
            let body = score::strip_html(article.content_html.as_deref().unwrap_or(""));
            let rule_score = score::match_rules(
                &compiled,
                &score::MatchInput {
                    feed_id: article.feed_id,
                    folder_ids: &folder_cache[&folder],
                    author: article.author.as_deref(),
                    tags: &tags,
                    title: &article.title,
                    url: article.canonical_url.as_deref(),
                    body: &body,
                },
            )
            .score;
            let vector = crate::rag::title_embedding(&tx, article.id)?.unwrap_or_default();
            let interest_boost = interests::calculate_boost(&vector, &prototypes).0;
            candidates.push(Candidate {
                label,
                published_at: article.published_at.unwrap_or(0),
                rule_score,
                interest_boost,
                features: crate::features::extract_features_with_bigrams(
                    article.author.as_deref(),
                    &tags,
                    &article.title,
                    article.canonical_url.as_deref(),
                    None,
                    true,
                ),
                feed_id: article.feed_id,
                cluster_id: article.cluster_id,
                published: article.published_at,
            });
        }
        tx.commit()?;
        Ok(Self {
            candidates,
            b1,
            since,
            until,
        })
    }

    /// 默认最近一次日志向前 30 天，纳入曝光之后的行为，固定数据窗口可复现。
    pub fn latest(conn: &Connection) -> Result<Self> {
        let latest: Option<i64> = conn.query_row(
            "SELECT MAX(at) FROM (
            SELECT shown_at AS at FROM exposures WHERE surface='focus'
            UNION ALL SELECT at FROM article_events
            UNION ALL SELECT opened_at AS at FROM read_sessions)",
            [],
            |r| r.get(0),
        )?;
        let until = latest.unwrap_or(0).saturating_add(1);
        Self::load(conn, until.saturating_sub(WINDOW_MS), until)
    }

    pub fn evaluate(&self, config: Config) -> Outcome {
        let rule = |c: &Candidate| if config.rules { c.rule_score } else { 0 };
        let mut visible: Vec<_> = self
            .candidates
            .iter()
            .filter(|c| rule(c) > config.hide_threshold)
            .collect();
        let hidden_positives = self
            .candidates
            .iter()
            .filter(|c| c.label.positive && rule(c) <= config.hide_threshold)
            .count();
        let mut outcome = Outcome {
            config,
            labels: visible.len(),
            hidden_positives,
            ranked: None,
            chronological: None,
        };
        if visible.len() < MIN_LABELS {
            return outcome;
        }
        // AUC 评估最终排法：使用序位包含焦点的时间 / id 打破同分规则，
        // 避免规则关闭时与时间排法完全相同，却因主分并列产生虚假的 AUC 提升。
        let read = |ordered: &[&Candidate]| {
            let samples: Vec<_> = ordered
                .iter()
                .enumerate()
                .map(|(i, c)| Sample {
                    positive: c.label.positive,
                    weight: c.label.weight,
                    score: -(i as f64),
                })
                .collect();
            Reading {
                auc: metrics::weighted_auc(&samples),
                ndcg: metrics::ndcg_at_10(&samples),
            }
        };
        visible.sort_by_key(|c| std::cmp::Reverse((c.published_at, c.label.article_id)));
        outcome.chronological = Some(read(&visible));
        visible.sort_by_cached_key(|c| {
            std::cmp::Reverse((
                focus::score(rule(c), c.interest_boost, config.interests, 0)
                    + if config.b1 {
                        self.b1
                            .boost(
                                (c.label.article_id, c.feed_id),
                                &c.features,
                                c.cluster_id,
                                c.published,
                                self.since,
                                crate::rank::combine::Switches {
                                    affinity: config.b2,
                                    ..config.b1_components
                                },
                            )
                            .score
                    } else {
                        0
                    },
                c.published_at,
                c.label.article_id,
            ))
        });
        outcome.ranked = Some(read(&visible));
        outcome
    }

    pub fn report(&self, configs: &[Config]) -> String {
        let mut text = format!(
            "E1 offline replay: focus, UTC [{}, {}), labeled={}\n",
            self.since,
            self.until,
            self.candidates.len()
        );
        text.push_str("IPW: position open-rate / first-position rate (>=20 exposures each), sparse fallback=DCG 1/log2(pos+2), cap=10\n");
        text.push_str("Current rules/interests/content; language inferred from current title/body (translation language unavailable); summary-only approximated by short feed text without fetched fulltext; daily merge=UTC\n");
        text.push_str("B1/B2 learn strictly before window start; active-space title_summary vectors are current snapshots; legacy votes only without prior E0 behavior; cluster membership/current metadata are snapshots; freshness evaluated at window start\n");
        text.push_str("Article score order only; cluster grouping, feed mixing, MMR and exploration breakdown not evaluated\n");
        text.push_str("AUC uses final order, including published_at/id tie breaks; deltas are absolute differences on each config's visible candidates\n");
        let value = |n: Option<f64>| {
            n.map(|v| format!("{v:.6}"))
                .unwrap_or_else(|| "未定义".into())
        };
        for &config in configs {
            let outcome = self.evaluate(config);
            let _ = write!(
                text,
                "rules={} interests={} b1={} b2={} threshold={} n={} hidden_positive={}: ",
                config.rules,
                config.interests,
                config.b1,
                config.b2,
                config.hide_threshold,
                outcome.labels,
                outcome.hidden_positives
            );
            match (outcome.ranked, outcome.chronological) {
                (Some(rank), Some(time)) => {
                    let _ = writeln!(
                        text,
                        "AUC={} time={} delta={}; NDCG@10={} time={} delta={}",
                        value(rank.auc),
                        value(time.auc),
                        value(rank.auc.zip(time.auc).map(|(a, b)| a - b)),
                        value(rank.ndcg),
                        value(time.ndcg),
                        value(rank.ndcg.zip(time.ndcg).map(|(a, b)| a - b))
                    );
                    if config.b1 && config.b2 {
                        let off = self
                            .evaluate(Config {
                                b2: false,
                                ..config
                            })
                            .ranked
                            .unwrap();
                        let _ = writeln!(
                            text,
                            "B2 on/off same candidates: n={} AUC_on={} AUC_off={} delta={}; NDCG@10_on={} NDCG@10_off={} delta={}",
                            outcome.labels,
                            value(rank.auc),
                            value(off.auc),
                            value(rank.auc.zip(off.auc).map(|(a, b)| a - b)),
                            value(rank.ndcg),
                            value(off.ndcg),
                            value(rank.ndcg.zip(off.ndcg).map(|(a, b)| a - b))
                        );
                    }
                    if config.b1 {
                        let off = self
                            .evaluate(Config {
                                b1: false,
                                b2: false,
                                ..config
                            })
                            .ranked
                            .unwrap();
                        let _ = writeln!(
                            text,
                            "B1 on/off same candidates: n={} AUC_on={} AUC_off={} delta={}; NDCG@10_on={} NDCG@10_off={} delta={}",
                            outcome.labels,
                            value(rank.auc),
                            value(off.auc),
                            value(rank.auc.zip(off.auc).map(|(a, b)| a - b)),
                            value(rank.ndcg),
                            value(off.ndcg),
                            value(rank.ndcg.zip(off.ndcg).map(|(a, b)| a - b))
                        );
                    }
                }
                _ => text.push_str("样本不足\n"),
            }
        }
        text
    }
}

struct ArticleFields {
    id: i64,
    feed_id: i64,
    title: String,
    author: Option<String>,
    canonical_url: Option<String>,
    content_html: Option<String>,
    published_at: Option<i64>,
    cluster_id: Option<i64>,
}
