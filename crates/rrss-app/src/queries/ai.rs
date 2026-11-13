use rusqlite::Connection;

use rrss_core::ai_store::{self, ALL_TASKS};
use rrss_core::translations;
use rrss_render::document_to_json;

use crate::views::*;

pub fn model_bindings(conn: &Connection) -> rrss_core::Result<ModelBindingsView> {
    Ok(ModelBindingsView {
        tasks: task_bindings(conn)?,
        chat_uses_own_config: rrss_core::pi_store::use_own_config(conn)?,
    })
}

/// 每个用途选的模型（阶段 19-7：没有继承）。
fn task_bindings(conn: &Connection) -> rrss_core::Result<Vec<TaskBindView>> {
    ALL_TASKS
        .iter()
        .map(|task| {
            let b = ai_store::get_task_binding(conn, task)?;
            let chosen = ai_store::resolve_binding_model(conn, &b)?;
            let (provider_id, model) = chosen.map(|(p, m)| (p.id, m)).unwrap_or_default();
            Ok(TaskBindView {
                task: (*task).into(),
                title: ai_store::task_title(task).into(),
                provider_id,
                model,
            })
        })
        .collect()
}

pub(crate) fn secret_view(value: Option<&str>, missing: bool) -> SecretView {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        Some(key) => {
            let chars: Vec<char> = key.chars().collect();
            SecretView {
                state: KeyState::Set,
                // 16 位以上显示首尾各 4 位，9–15 位只显示末 4 位，更短的不显示。
                hint: match chars.len() {
                    16.. => format!(
                        "{}…{}",
                        chars[..4].iter().collect::<String>(),
                        chars[chars.len() - 4..].iter().collect::<String>()
                    ),
                    9..=15 => format!("…{}", chars[chars.len() - 4..].iter().collect::<String>()),
                    _ => String::new(),
                },
            }
        }
        None => SecretView {
            state: if missing {
                KeyState::Missing
            } else {
                KeyState::None
            },
            hint: String::new(),
        },
    }
}

/// 连接视图不带完整密钥（阶段 19-9），只有状态与首尾各 4 位。
pub(crate) fn provider_view(
    conn: &Connection,
    p: ai_store::AiProvider,
) -> rrss_core::Result<ProviderView> {
    let compat_json = ai_store::provider_compat(conn, p.id)?.to_string();
    let SecretView {
        state: key,
        hint: key_hint,
    } = secret_view(p.api_key.as_deref(), p.api_key.as_deref() == Some(""));
    Ok(ProviderView {
        id: p.id,
        kind: p.kind,
        name: p.name,
        base_url: p.base_url,
        protocol: p.protocol,
        key,
        key_hint,
        last_test: p.last_test.map(|t| ProviderTestView {
            at: t.at,
            ok: t.ok,
            note: t.note,
        }),
        compat_json: Some(compat_json),
    })
}

fn chat_tokens(conn: &Connection, prefix: &str) -> i64 {
    rrss_core::pi_store::pi_usage_with_prefix(conn, prefix)
        .map(|u| u.input_tokens + u.output_tokens)
        .unwrap_or(0)
}

/// 用途绑定了模型、连接地址不为空。
fn model_ready(conn: &Connection, task: &str) -> bool {
    ai_store::resolve_task(conn, task).is_ok_and(|task| !task.provider.base_url.trim().is_empty())
}

fn round_tokens(tokens: f64, step: i64) -> i64 {
    (tokens / step as f64).round() as i64 * step
}

fn auto_summary(
    conn: &Connection,
    today: &ai_store::UsageDay,
    generation_budget: i64,
) -> rrss_core::Result<AutoSummaryView> {
    let tier = ai_store::input_tier(conn)?;
    let notice = rrss_core::db::get_setting(conn, ai_store::FOCUS_AUTO_SUMMARY_NOTICE_KEY)?;
    Ok(AutoSummaryView {
        today: today.auto_summaries,
        daily_limit: ai_store::FOCUS_DAILY_LIMIT,
        window_days: ai_store::FOCUS_WINDOW_DAYS,
        batch_size: ai_store::FOCUS_BATCH,
        batch_minutes: ai_store::FOCUS_BATCH_MINUTES as i64,
        model_ready: model_ready(conn, ai_store::TASK_FOCUS_SUMMARY),
        per_article_tokens: round_tokens(tier.summary_budget(false) as f64 * 0.75 + 1_000.0, 500),
        digest_daily_tokens: round_tokens(
            (10_000.0 + tier.digest_total_chars() as f64 * 0.75) * 8.0 / 7.0,
            5_000,
        ),
        notice: match notice.as_deref() {
            Some(value @ ("default" | "kept")) => Some(AutoSummaryNotice {
                budget_defaulted: value == "default",
                generation_budget,
            }),
            _ => None,
        },
        cluster_today: today.auto_cluster_summaries,
        cluster_daily_limit: ai_store::CLUSTER_DAILY_LIMIT,
        // 典型组三篇已有摘要，各约 400 字，加模板与来源信息约 400 字；不按正文档位变化。
        per_cluster_tokens: rrss_core::cluster_summary::estimate_tokens(1_600),
        cluster_model_ready: model_ready(conn, ai_store::TASK_CLUSTER_SUMMARY),
    })
}

pub fn ai_page(conn: &Connection, selected_id: i64) -> rrss_core::Result<AiPage> {
    let day = ai_store::local_day();
    let providers = ai_store::list_providers(conn)?
        .into_iter()
        .map(|p| provider_view(conn, p))
        .collect::<rrss_core::Result<Vec<_>>>()?;
    let selected_id = if selected_id > 0 {
        selected_id
    } else {
        providers.first().map(|p| p.id).unwrap_or(0)
    };
    let month = &day[..7];
    let today = ai_store::usage_on(conn, &day)?;
    let (generation_month, embedding_month) = conn.query_row(
        "SELECT coalesce(sum(tokens),0), coalesce(sum(embed_tokens),0)
         FROM ai_usage WHERE substr(day,1,7)=?1",
        [month],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let generation_budget = ai_store::daily_budget_tokens(conn)?;
    let embedding_budget = ai_store::embed_daily_budget_tokens(conn)?;
    let line = |today, month, budget| UsageLine {
        today,
        month,
        paused: budget > 0 && today >= budget,
    };
    let usage = AiUsageView {
        generation: line(today.tokens, generation_month, generation_budget),
        embedding: line(today.embed_tokens, embedding_month, embedding_budget),
        chat: line(chat_tokens(conn, &day), chat_tokens(conn, month), 0),
    };
    Ok(AiPage {
        providers,
        selected_id,
        usage,
        auto_summary: auto_summary(conn, &today, generation_budget)?,
    })
}

pub fn translate_page(conn: &Connection) -> rrss_core::Result<TranslatePage> {
    let (deepl_url, deepl_key) = translations::deepl_config(conn).unwrap_or_default();
    let (deeplx_url, deeplx_token) =
        translations::deeplx_config(conn).unwrap_or(("http://127.0.0.1:1188".into(), None));
    let (http_url, http_key) =
        translations::http_config(conn).unwrap_or(("http://127.0.0.1:5000".into(), None));

    let secret = |value: Option<&str>, key: &str| -> rrss_core::Result<SecretView> {
        let missing = rrss_core::db::get_setting(conn, &translations::missing_key(key))?.as_deref()
            == Some("1");
        Ok(secret_view(value, missing))
    };
    Ok(TranslatePage {
        selection_config: crate::translate::selection::configuration(conn)?,
        enabled: translations::is_enabled(conn).unwrap_or(true),
        backend: translations::backend_id(conn)
            .unwrap_or_else(|_| translations::ENGINE_COMPLETE.into()),
        target_lang: translations::target_lang(conn).unwrap_or_else(|_| "zh".into()),
        deepl_key: secret(deepl_key.as_deref(), translations::TRANSLATE_DEEPL_KEY_KEY)?,
        deepl_url: deepl_url.unwrap_or_default(),
        deeplx_url,
        deeplx_token: secret(
            deeplx_token.as_deref(),
            translations::TRANSLATE_DEEPLX_TOKEN_KEY,
        )?,
        http_url,
        http_key: secret(http_key.as_deref(), translations::TRANSLATE_HTTP_KEY_KEY)?,
    })
}

pub fn digest_summaries(conn: &Connection, period: &str) -> Vec<DigestSummary> {
    let period = if period == "weekly" {
        "weekly"
    } else {
        "daily"
    };
    ai_store::list_digests(conn, period, 60)
        .unwrap_or_default()
        .into_iter()
        .map(|d| DigestSummary {
            id: d.id,
            period: d.period.clone(),
            period_label: if d.period == "weekly" {
                "周报".into()
            } else {
                "日报".into()
            },
            period_start: d.period_start,
            date_label: ai_store::digest_date_label(&d.period, d.period_start),
        })
        .collect()
}

pub fn digest(conn: &Connection, id: i64) -> rrss_core::Result<DigestView> {
    let d = ai_store::digest_by_id(conn, id)?.ok_or(rrss_core::error::Error::DigestNotFound)?;
    let parsed = crate::digest::parse_digest(conn, d);
    let items = parsed
        .context
        .items
        .iter()
        .map(|line| DigestLineView {
            block_id: line.block_id as i64,
            sources: line
                .sources
                .iter()
                .filter_map(|source| {
                    parsed
                        .source_index
                        .iter()
                        .find(|entry| entry.article_id == source.article_id)
                        .cloned()
                })
                .collect(),
        })
        .collect();
    let document = serde_json::to_value(&parsed.document).ok();
    let source_index = parsed.source_index;
    let unlocated_sources = parsed.unlocated_sources;
    let d = parsed.context;
    let source_note = match d.source_state.as_str() {
        "legacy" => "旧稿来源仅保留原有位置，无法验证历史映射。",
        "mismatch" => "来源索引与正文不一致，已关闭条目来源。",
        "unavailable" if !unlocated_sources.is_empty() => "旧稿来源未定位，已关闭条目来源。",
        "unavailable" => "本期没有可用来源。",
        _ => "",
    }
    .to_string();
    Ok(DigestView {
        id: d.id,
        period: d.period.clone(),
        period_label: if d.period == "weekly" {
            "周报".into()
        } else {
            "日报".into()
        },
        period_start: d.period_start,
        date_label: ai_store::digest_date_label(&d.period, d.period_start),
        document,
        body: d.body,
        revision: d.revision,
        source_state: d.source_state,
        source_note,
        source_index,
        unlocated_sources,
        items,
    })
}

pub fn markdown_document_json(body: &str) -> Option<serde_json::Value> {
    let doc = rrss_render::markdown_to_document(body);
    serde_json::from_str(&document_to_json(&doc).unwrap_or_else(|_| "{}".into())).ok()
}

pub fn digest_banner_label(period: &str, period_start: i64) -> String {
    let date = ai_store::digest_date_label(period, period_start);
    let kind = if period == "weekly" {
        "本周简报"
    } else {
        "今日简报"
    };
    format!("{kind} · {date}")
}

pub fn digest_banner_text(body: &str) -> String {
    if body.trim().is_empty() {
        return String::new();
    }
    let doc = rrss_render::markdown_to_document(body);
    let plain = rrss_render::document_to_plain(&doc);
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "今日 Digest 已生成".into();
    }
    const MAX: usize = 96;
    let mut n = 0;
    for (i, _) in collapsed.char_indices() {
        if n >= MAX {
            return format!("{}…", &collapsed[..i]);
        }
        n += 1;
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;

    fn test_db() -> (tempfile::TempDir, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("ai-page.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn dismiss_hides_both_upgrade_notices() {
        let (_dir, conn) = test_db();
        for (marker, budget, defaulted) in [("default", "200000", true), ("kept", "42000", false)] {
            db::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY_NOTICE_KEY, marker).unwrap();
            crate::actions::set_setting(&conn, ai_store::GENERATION_BUDGET.id, budget).unwrap();
            let notice = ai_page(&conn, 0).unwrap().auto_summary.notice.unwrap();
            assert_eq!(notice.budget_defaulted, defaulted);
            assert_eq!(notice.generation_budget, budget.parse::<i64>().unwrap());
            crate::actions::dismiss_auto_summary_notice(&conn).unwrap();
            assert!(ai_page(&conn, 0).unwrap().auto_summary.notice.is_none());
            assert_eq!(
                db::get_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY_NOTICE_KEY)
                    .unwrap()
                    .as_deref(),
                Some("done")
            );
        }
    }

    #[test]
    fn enabling_auto_summary_dismisses_notice_atomically() {
        let (_dir, conn) = test_db();
        db::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY_NOTICE_KEY, "kept").unwrap();
        crate::actions::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY.id, "false").unwrap();
        assert!(ai_page(&conn, 0).unwrap().auto_summary.notice.is_some());
        conn.execute_batch(
            "CREATE TRIGGER reject_notice BEFORE UPDATE ON settings
            WHEN NEW.key='focus_auto_summary_notice' BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
        assert!(
            crate::actions::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY.id, "true").is_err()
        );
        assert!(!ai_store::FOCUS_AUTO_SUMMARY.enabled(&conn).unwrap());
        assert!(ai_page(&conn, 0).unwrap().auto_summary.notice.is_some());
        conn.execute_batch("DROP TRIGGER reject_notice;").unwrap();
        crate::actions::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY.id, "true").unwrap();
        assert!(ai_store::FOCUS_AUTO_SUMMARY.enabled(&conn).unwrap());
        assert!(ai_page(&conn, 0).unwrap().auto_summary.notice.is_none());
    }

    #[test]
    fn low_tier_estimates() {
        let (_dir, conn) = test_db();
        crate::actions::set_setting(&conn, ai_store::AI_INPUT_TIER_KEY, "low").unwrap();
        let view = ai_page(&conn, 0).unwrap().auto_summary;
        assert_eq!(
            (
                view.per_article_tokens,
                view.digest_daily_tokens,
                view.per_cluster_tokens
            ),
            (2500, 25000, 2000)
        );
    }

    #[test]
    fn high_tier_estimates() {
        let (_dir, conn) = test_db();
        crate::actions::set_setting(&conn, ai_store::AI_INPUT_TIER_KEY, "high").unwrap();
        let view = ai_page(&conn, 0).unwrap().auto_summary;
        assert_eq!(
            (
                view.per_article_tokens,
                view.digest_daily_tokens,
                view.per_cluster_tokens
            ),
            (8500, 65000, 2000)
        );
    }

    #[test]
    fn usage_separates_days_months_budgets_and_auto_counts() {
        let (_dir, conn) = test_db();
        let empty = ai_page(&conn, 0).unwrap();
        assert_eq!(
            (
                empty.usage.generation.today,
                empty.usage.embedding.month,
                empty.auto_summary.cluster_today
            ),
            (0, 0, 0)
        );
        assert!(!empty.auto_summary.model_ready);
        assert!(empty.auto_summary.notice.is_none());
        let day = ai_store::local_day();
        let other_day = format!(
            "{}-{}",
            &day[..7],
            if day.ends_with("01") { "02" } else { "01" }
        );
        for (date, generation, embedding) in [
            (day.as_str(), 200000, 1000000),
            (other_day.as_str(), 50, 80),
            ("2000-01-01", 9000000, 9000000),
        ] {
            ai_store::add_usage(&conn, date, generation, 0).unwrap();
            ai_store::add_embed_usage(&conn, date, embedding).unwrap();
            rrss_core::pi_store::add_pi_usage(&conn, date, 10, 5, 0.0).unwrap();
        }
        conn.execute(
            "UPDATE ai_usage SET auto_summaries=7, auto_cluster_summaries=3 WHERE day=?1",
            [&day],
        )
        .unwrap();
        let page = ai_page(&conn, 0).unwrap();
        assert_eq!(
            (
                page.usage.generation.today,
                page.usage.generation.month,
                page.usage.generation.paused
            ),
            (200000, 200050, true)
        );
        assert_eq!(
            (
                page.usage.embedding.today,
                page.usage.embedding.month,
                page.usage.embedding.paused
            ),
            (1000000, 1000080, true)
        );
        assert_eq!(
            (
                page.usage.chat.today,
                page.usage.chat.month,
                page.usage.chat.paused
            ),
            (15, 30, false)
        );
        let auto = page.auto_summary;
        assert_eq!(
            (
                auto.today,
                auto.daily_limit,
                auto.window_days,
                auto.batch_size,
                auto.batch_minutes
            ),
            (7, 100, 3, 8, 15)
        );
        assert_eq!((auto.cluster_today, auto.cluster_daily_limit), (3, 20));
        for id in [ai_store::GENERATION_BUDGET.id, ai_store::EMBED_BUDGET.id] {
            crate::actions::set_setting(&conn, id, "0").unwrap();
        }
        let provider =
            crate::test_net::fake_provider(&conn, "fixture", "http://model.invalid".into());
        ai_store::set_task_binding(
            &conn,
            ai_store::TASK_FOCUS_SUMMARY,
            Some(provider),
            Some("fixture"),
        )
        .unwrap();
        let page = ai_page(&conn, 0).unwrap();
        assert!(!page.usage.generation.paused && !page.usage.embedding.paused);
        assert!(page.auto_summary.model_ready);
        assert!(!page.auto_summary.cluster_model_ready);
        ai_store::set_task_binding(
            &conn,
            ai_store::TASK_CLUSTER_SUMMARY,
            Some(provider),
            Some("fixture"),
        )
        .unwrap();
        assert!(ai_page(&conn, 0).unwrap().auto_summary.cluster_model_ready);
    }

    #[test]
    fn saved_keys_show_ends_by_length() {
        let hint = |key: &str| super::secret_view(Some(key), false).hint;
        assert_eq!(hint("sk-or-v1-0123456789abcd"), "sk-o…abcd");
        assert_eq!(hint("deepl-12:fx"), "…2:fx");
        assert_eq!(hint("short"), "");
    }
}
