//! 设置选项注册表（阶段 19-1）：简单选项的描述符、读取与按描述符校验的写入。
//! 复杂对象（连接、用途绑定、工具、技能、MCP）继续走各自的表和命令，不在这里。
//! 前端文案由生成的 SettingId 检查，预览和测试复用同一份导出描述符。

use rrss_core::ai_store::{self, InputTier};
use rrss_core::{articles, db, pi_store, translations};
use rusqlite::Connection;

use crate::views::{
    SettingApply, SettingDomain, SettingKind, SettingLevel, SettingOption, SettingSpec,
    SettingValue, SettingsSchema,
};

enum Kind {
    Bool,
    Int(i64, i64),
    /// `(id, 标签, 说明)`。
    Enum(&'static [(&'static str, &'static str, &'static str)]),
    /// 输入档位：说明里带着 rrss-core 的预算数值，运行时生成。
    InputTier,
    /// 去掉首尾空白后的字数上限。
    Text(usize),
    /// 沿用旧单值入口的规范化与旧库读取语义。
    LegacyEnum(
        &'static [(&'static str, &'static str, &'static str)],
        fn(&str) -> String,
    ),
    Language,
}

struct Def {
    id: &'static str,
    domain: SettingDomain,
    level: SettingLevel,
    kind: Kind,
    default: &'static str,
    apply: SettingApply,
    affects_pi: bool,
    write: Option<fn(&Connection, &str, &str) -> rrss_core::Result<()>>,
}

const CACHE_WARMING: &[(&str, &str, &str)] = &[
    ("off", "关闭", ""),
    ("streaming", "回复时", "回复进行中保持服务商的提示缓存。"),
    (
        "idle",
        "空闲时也保持",
        "两次提问之间也保持，没有对话时也可能产生费用。",
    ),
];

const fn rank_switch(id: &'static str) -> Def {
    Def {
        id,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: Some(write_rank_switch),
    }
}

const fn ai_setting(setting: ai_store::SettingDefault, kind: Kind, apply: SettingApply) -> Def {
    Def {
        id: setting.id,
        domain: SettingDomain::Model,
        level: SettingLevel::L1,
        kind,
        default: setting.default,
        apply,
        affects_pi: false,
        write: None,
    }
}

/// 注册表。键名与各自的存取函数一致（`rrss_core` 里的常量）。
const DEFS: &[Def] = &[
    Def {
        id: "web_javascript",
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: "web_open_mode",
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Enum(&[
            ("reader", "在 RRSS 阅读视图打开", ""),
            ("web", "打开原网页", ""),
            ("browser", "在系统浏览器打开", ""),
        ]),
        default: "reader",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: "web_original_tip",
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: "web_first_tip",
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: rrss_core::vector_space::ALLOW_MODEL_CHANGE,
        domain: SettingDomain::Model,
        level: SettingLevel::L1,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: rrss_core::feed_icon::SETTING_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::THEME_PREFERENCE_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::LegacyEnum(
            &[
                ("system", "跟随系统", ""),
                ("light", "亮色", ""),
                ("dark", "暗色", ""),
            ],
            |raw| {
                if matches!(raw, "light" | "dark") {
                    raw.into()
                } else {
                    "system".into()
                }
            },
        ),
        default: "system",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::FOCUS_FILTER_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::LegacyEnum(
            &[
                ("focus", "只看焦点", ""),
                ("hide_faded", "隐藏淡化", ""),
                ("all", "全部", ""),
            ],
            |raw| articles::FocusFilter::parse_str(raw).as_str().into(),
        ),
        default: "hide_faded",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::FOCUS_COLUMNS_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Text(scope_list_chars(5)),
        default: "",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::LIST_SCOPE_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::LegacyEnum(
            &[
                ("all", "全部", ""),
                ("unread", "未读", ""),
                ("starred", "收藏", ""),
            ],
            |raw| articles::ListScope::parse_str(raw).as_str().into(),
        ),
        default: "all",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::FEEDS_SCOPE_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Text(32),
        default: "all",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::FEEDS_PINNED_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Text(scope_list_chars(10)),
        default: "",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::FEEDS_LIST_SCOPE_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::LegacyEnum(
            &[
                ("all", "全部", ""),
                ("unread", "未读", ""),
                ("starred", "收藏", ""),
            ],
            |raw| articles::ListScope::parse_str(raw).as_str().into(),
        ),
        default: "all",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::CLUSTER_GROUP_ENABLED_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::INTEREST_BOOST_ENABLED_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    rank_switch(db::RANK_FEATURES_ENABLED_KEY),
    rank_switch(db::RANK_SOURCE_ENABLED_KEY),
    rank_switch(db::RANK_COVERAGE_ENABLED_KEY),
    rank_switch(db::RANK_FRESHNESS_ENABLED_KEY),
    rank_switch(db::RANK_AFFINITY_ENABLED_KEY),
    Def {
        id: db::FOCUS_SHOW_DETAILS_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::HIDDEN_SPOT_CHECK_RATE_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Int(0, 5),
        default: "3",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::FOCUS_RERANK_ENABLED_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::HIDE_NOISY_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::FEED_FETCH_PER_HOST_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L1,
        kind: Kind::Int(
            db::FEED_FETCH_PER_HOST_RANGE.0 as i64,
            db::FEED_FETCH_PER_HOST_RANGE.1 as i64,
        ),
        default: "4",
        apply: SettingApply::Draft,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::HIDE_THRESHOLD_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Int(i64::MIN, i64::MAX),
        default: "-10",
        apply: SettingApply::Draft,
        affects_pi: false,
        write: None,
    },
    Def {
        id: translations::TRANSLATE_TARGET_LANG_KEY,
        domain: SettingDomain::Reader,
        level: SettingLevel::L0,
        kind: Kind::Language,
        default: "zh",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: Some(|conn, id, value| {
            crate::configuration::write(conn, |tx| db::set_setting(tx, id, value))
        }),
    },
    Def {
        id: ai_store::DEVELOPER_TOOLS_KEY,
        domain: SettingDomain::Assistant,
        level: SettingLevel::L2,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: true,
        write: None,
    },
    Def {
        id: ai_store::FETCH_URL_ENABLED_KEY,
        domain: SettingDomain::Assistant,
        level: SettingLevel::L1,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: true,
        write: None,
    },
    Def {
        id: "ui_debug_mode",
        domain: SettingDomain::Reader,
        level: SettingLevel::L2,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        write: Some(write_focus_auto_summary),
        ..ai_setting(
            ai_store::FOCUS_AUTO_SUMMARY,
            Kind::Bool,
            SettingApply::Immediate,
        )
    },
    ai_setting(
        ai_store::CLUSTER_AUTO_SUMMARY,
        Kind::Bool,
        SettingApply::Immediate,
    ),
    ai_setting(
        ai_store::GENERATION_BUDGET,
        Kind::Int(0, 1_000_000_000),
        SettingApply::Draft,
    ),
    ai_setting(
        ai_store::EMBED_BUDGET,
        Kind::Int(0, 1_000_000_000),
        SettingApply::Draft,
    ),
    Def {
        id: db::FOCUS_SUMMARY_NEUTRAL_LIMIT_KEY,
        domain: SettingDomain::Model,
        level: SettingLevel::L1,
        kind: Kind::Int(0, 100),
        default: "0",
        apply: SettingApply::Draft,
        affects_pi: false,
        write: None,
    },
    Def {
        id: ai_store::AI_INPUT_TIER_KEY,
        domain: SettingDomain::Model,
        level: SettingLevel::L1,
        kind: Kind::InputTier,
        default: "medium",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: rrss_core::digest::DIGEST_GENERATE_MINUTES_KEY,
        domain: SettingDomain::Model,
        level: SettingLevel::L1,
        kind: Kind::Int(0, 24 * 60 - 1),
        default: "480",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: db::PI_IDLE_TIMEOUT_SECS_KEY,
        domain: SettingDomain::Assistant,
        level: SettingLevel::L1,
        kind: Kind::Int(0, 7 * 24 * 3600),
        default: "1800",
        apply: SettingApply::Draft,
        affects_pi: false,
        write: None,
    },
    Def {
        id: pi_store::PI_ASSISTANT_NOTE_KEY,
        domain: SettingDomain::Assistant,
        level: SettingLevel::L1,
        kind: Kind::Text(pi_store::ASSISTANT_NOTE_MAX_CHARS),
        default: "",
        apply: SettingApply::Draft,
        affects_pi: true,
        write: None,
    },
    Def {
        id: pi_store::PI_AUTO_DOWNLOAD_KEY,
        domain: SettingDomain::Assistant,
        level: SettingLevel::L1,
        kind: Kind::Bool,
        default: "1",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
    Def {
        id: pi_store::PI_CACHE_WARMING_KEY,
        domain: SettingDomain::Assistant,
        level: SettingLevel::L2,
        kind: Kind::Enum(CACHE_WARMING),
        default: "off",
        apply: SettingApply::Immediate,
        affects_pi: true,
        write: None,
    },
    Def {
        id: pi_store::PI_AUTO_UPDATE_KEY,
        domain: SettingDomain::Assistant,
        level: SettingLevel::L1,
        kind: Kind::Bool,
        default: "0",
        apply: SettingApply::Immediate,
        affects_pi: false,
        write: None,
    },
];

/// 描述符与当前值，一次读出给设置页。
pub fn schema(conn: &Connection) -> rrss_core::Result<SettingsSchema> {
    let mut specs = Vec::with_capacity(DEFS.len());
    let mut values = Vec::with_capacity(DEFS.len());
    for def in DEFS {
        specs.push(spec(def));
        values.push(SettingValue {
            id: def.id.into(),
            value: read(conn, def)?,
        });
    }
    Ok(SettingsSchema { specs, values })
}

/// 按描述符校验并写入，返回规范化后的值。未登记的键、越界或不认识的值都拒绝，原值不动。
pub fn set(conn: &Connection, id: &str, raw: &str) -> rrss_core::Result<SettingValue> {
    let def = DEFS
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| rrss_core::Error::InvalidSetting(format!("没有名为 {id} 的设置项")))?;
    let value = normalize(def, raw)?;
    match def.write {
        Some(write) => write(conn, def.id, &value)?,
        None => db::set_setting(conn, def.id, &value)?,
    }
    Ok(SettingValue {
        id: def.id.into(),
        value,
    })
}

fn write_focus_auto_summary(conn: &Connection, key: &str, value: &str) -> rrss_core::Result<()> {
    db::atomic_write(conn, |conn| {
        db::set_setting(conn, key, value)?;
        if value == "1" {
            conn.execute(
                "UPDATE settings SET value='done' WHERE key=?1 AND value IN ('default','kept')",
                [ai_store::FOCUS_AUTO_SUMMARY_NOTICE_KEY],
            )?;
        }
        Ok(())
    })
}

fn write_rank_switch(conn: &Connection, key: &str, value: &str) -> rrss_core::Result<()> {
    rrss_core::rank::store::set_switch(conn, key, value == "1")
}

fn spec(def: &Def) -> SettingSpec {
    let options = |items: &[(&str, &str, &str)]| {
        items
            .iter()
            .map(|(id, label, hint)| SettingOption {
                id: (*id).into(),
                label: (*label).into(),
                hint: (*hint).into(),
            })
            .collect()
    };
    let kind = match &def.kind {
        Kind::Bool => SettingKind::Bool,
        Kind::Int(min, max) => SettingKind::Int {
            min: *min,
            max: *max,
        },
        Kind::Enum(items) | Kind::LegacyEnum(items, _) => SettingKind::Enum {
            options: options(items),
        },
        Kind::InputTier => SettingKind::Enum {
            options: input_tier_options(),
        },
        Kind::Text(max) => SettingKind::Text { max: *max as i64 },
        Kind::Language => SettingKind::Text { max: 16 },
    };
    SettingSpec {
        id: def.id.into(),
        domain: def.domain,
        level: def.level,
        kind,
        default: def.default.into(),
        apply: def.apply,
        affects_pi: def.affects_pi,
    }
}

fn enum_ids(def: &Def) -> Vec<String> {
    match &def.kind {
        Kind::Enum(items) => items.iter().map(|(id, _, _)| (*id).to_string()).collect(),
        Kind::InputTier => [InputTier::Low, InputTier::Medium, InputTier::High]
            .iter()
            .map(|t| t.as_str().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// `folder:<i64>` 一项的字符预算（含分隔逗号），两项设置共用。
const fn scope_list_chars(items: usize) -> usize {
    items * 32
}

/// 逗号分隔的树项：去空白、去重、截断。`allow_feeds` 为假时拒绝 `feed:`。
fn normalize_scope_list(
    raw: &str,
    allow_feeds: bool,
    max: usize,
    message: &str,
) -> rrss_core::Result<String> {
    let invalid = || rrss_core::Error::InvalidSetting(message.into());
    let mut out = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let item = rrss_core::tree_order::TreeItem::parse(part).map_err(|_| invalid())?;
        if !allow_feeds && matches!(item, rrss_core::tree_order::TreeItem::Feed(_)) {
            return Err(invalid());
        }
        let key = item.key();
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out.truncate(max);
    Ok(out.join(","))
}

/// 库里的值读不懂时按默认值给出，不报错。
fn read(conn: &Connection, def: &Def) -> rrss_core::Result<String> {
    let raw = db::get_setting(conn, def.id)?;
    if def.id == db::HIDE_THRESHOLD_KEY {
        return Ok(articles::hide_threshold(conn)?.to_string());
    }
    Ok(raw
        .and_then(|raw| normalize(def, &raw).ok())
        .unwrap_or_else(|| def.default.to_string()))
}

fn normalize(def: &Def, raw: &str) -> rrss_core::Result<String> {
    if def.id == db::FEEDS_PINNED_KEY {
        return normalize_scope_list(raw, true, 10, "置顶项应为 folder:<正整数> 或 feed:<正整数>");
    }
    if def.id == db::FOCUS_COLUMNS_KEY {
        return normalize_scope_list(raw, false, 5, "焦点栏目的每一项都要是 folder:<正整数>");
    }
    match &def.kind {
        Kind::LegacyEnum(_, normalize) => return Ok(normalize(raw)),
        Kind::Language => return Ok(translations::normalize_lang(raw)),
        _ => {}
    }
    let raw = raw.trim();
    let invalid = |why: String| rrss_core::Error::InvalidSetting(why);
    match &def.kind {
        Kind::Bool => match raw {
            "1" | "true" | "TRUE" => Ok("1".into()),
            "0" | "false" | "FALSE" => Ok("0".into()),
            _ => Err(invalid(format!("{} 只能是开或关", def.id))),
        },
        Kind::Int(min, max) => {
            let n: i64 = raw
                .parse()
                .map_err(|_| invalid(format!("「{raw}」不是整数")))?;
            if n < *min || n > *max {
                return Err(invalid(format!("需要在 {min} 到 {max} 之间")));
            }
            if def.id == db::HIDDEN_SPOT_CHECK_RATE_KEY && !db::HIDDEN_SPOT_CHECK_RATES.contains(&n)
            {
                return Err(invalid("抽查比例只能是 0 / 1 / 3 / 5".into()));
            }
            Ok(n.to_string())
        }
        Kind::Text(max) => {
            if raw.chars().count() > *max {
                return Err(invalid(format!("不能超过 {max} 字")));
            }
            Ok(raw.to_string())
        }
        Kind::LegacyEnum(_, _) | Kind::Language => {
            unreachable!("legacy normalization handled above")
        }
        Kind::Enum(_) | Kind::InputTier => {
            let ids = enum_ids(def);
            if ids.iter().any(|id| id == raw) {
                Ok(raw.to_string())
            } else {
                Err(invalid(format!(
                    "「{raw}」不是可选值（{}）",
                    ids.join(" / ")
                )))
            }
        }
    }
}

/// 输入档位各档的说明：带着 rrss-core 的预算数值，改了预算这里自动跟着变。
fn input_tier_options() -> Vec<SettingOption> {
    [
        (InputTier::Low, "低", "尽量精简"),
        (InputTier::Medium, "中", "均衡"),
        (InputTier::High, "高", "少拆分"),
    ]
    .into_iter()
    .map(|(t, label, lead)| {
        let budget = t.summary_budget(false);
        SettingOption {
            id: t.as_str().into(),
            label: label.into(),
            hint: format!(
                "{lead}：每篇摘要最多送 {}正文（约 {} token，收藏或高分的多给一半），简报里每篇最多摘 {}。",
                chars_label(budget),
                approx_tokens(budget),
                chars_label(t.digest_item_chars()),
            ),
        }
    })
    .collect()
}

/// 中文正文粗算：约 4 字 3 个 token。只用于界面上的估算。
fn approx_tokens(chars: usize) -> String {
    let tokens = chars * 3 / 4;
    if tokens >= 1_000 {
        let tenths = (tokens + 50) / 100;
        if tenths.is_multiple_of(10) {
            format!("{}k", tenths / 10)
        } else {
            format!("{}.{}k", tenths / 10, tenths % 10)
        }
    } else {
        tokens.to_string()
    }
}

fn chars_label(chars: usize) -> String {
    if chars >= 10_000 && chars.is_multiple_of(10_000) {
        format!("{} 万字", chars / 10_000)
    } else {
        format!("{chars} 字")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("t.db")).unwrap();
        (dir, conn)
    }

    fn value(schema: &SettingsSchema, id: &str) -> String {
        schema
            .values
            .iter()
            .find(|v| v.id == id)
            .unwrap()
            .value
            .clone()
    }

    #[test]
    fn web_preferences_validate_and_reset_first_tip() {
        let (_dir, conn) = conn();
        assert_eq!(value(&schema(&conn).unwrap(), "web_open_mode"), "reader");
        assert_eq!(value(&schema(&conn).unwrap(), "web_first_tip"), "1");
        set(&conn, "web_open_mode", "browser").unwrap();
        assert!(set(&conn, "web_open_mode", "invalid").is_err());
        assert_eq!(value(&schema(&conn).unwrap(), "web_open_mode"), "browser");
        set(&conn, "web_first_tip", "0").unwrap();
        assert_eq!(value(&schema(&conn).unwrap(), "web_first_tip"), "0");
        set(&conn, "web_first_tip", "1").unwrap();
        assert_eq!(value(&schema(&conn).unwrap(), "web_first_tip"), "1");
    }

    #[test]
    fn schema_lists_every_setting_with_defaults() {
        let (_dir, conn) = conn();
        let schema = schema(&conn).unwrap();
        assert_eq!(schema.specs.len(), DEFS.len());
        for spec in &schema.specs {
            assert_eq!(value(&schema, &spec.id), spec.default, "{}", spec.id);
        }
        assert_eq!(
            value(&schema, db::FEED_FETCH_PER_HOST_KEY),
            db::FEED_FETCH_PER_HOST_DEFAULT.to_string()
        );
        let note = schema
            .specs
            .iter()
            .find(|s| s.id == "pi_assistant_note")
            .unwrap();
        assert!(note.affects_pi && note.apply == SettingApply::Draft);
        // 思考强度改为按模型设置（阶段 19-7）。
        assert!(schema.specs.iter().all(|s| s.id != "pi_thinking_level"));
    }

    #[test]
    fn ui_debug_is_persisted_independently_of_translation_and_pi_permissions() {
        let (_dir, conn) = conn();
        let revision = db::get_setting(&conn, "translation_config_revision").unwrap();
        let permissions = db::get_setting(&conn, ai_store::DEVELOPER_TOOLS_KEY).unwrap();
        assert_eq!(value(&schema(&conn).unwrap(), "ui_debug_mode"), "0");
        set(&conn, "ui_debug_mode", "1").unwrap();
        assert_eq!(value(&schema(&conn).unwrap(), "ui_debug_mode"), "1");
        assert_eq!(
            db::get_setting(&conn, ai_store::DEVELOPER_TOOLS_KEY).unwrap(),
            permissions
        );
        assert_eq!(
            db::get_setting(&conn, "translation_config_revision").unwrap(),
            revision
        );
        assert!(
            !schema(&conn)
                .unwrap()
                .specs
                .iter()
                .find(|spec| spec.id == "ui_debug_mode")
                .unwrap()
                .affects_pi
        );
    }

    #[test]
    fn input_tier_options_describe_core_numbers() {
        let options = input_tier_options();
        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["低", "中", "高"]);
        let hint = |i: usize| options[i].hint.as_str();
        assert!(
            hint(0).contains("2000 字") && hint(0).contains("1.5k token"),
            "{}",
            hint(0)
        );
        assert!(
            hint(1).contains("4000 字") && hint(1).contains("3k token"),
            "{}",
            hint(1)
        );
        assert!(
            hint(2).contains("1 万字") && hint(2).contains("7.5k token"),
            "{}",
            hint(2)
        );
    }

    #[test]
    fn migrated_single_values_keep_storage_and_legacy_reader_semantics() {
        let (_dir, conn) = conn();
        for (id, raw, expected) in [
            (db::THEME_PREFERENCE_KEY, "dark", "dark"),
            (db::THEME_PREFERENCE_KEY, " dark ", "system"),
            (db::FOCUS_FILTER_KEY, "focus", "focus"),
            (db::FOCUS_FILTER_KEY, "unknown", "hide_faded"),
            (db::LIST_SCOPE_KEY, "unread", "unread"),
            (db::LIST_SCOPE_KEY, " unread ", "all"),
            (db::FEEDS_SCOPE_KEY, " folder:4 ", "folder:4"),
            (db::FEEDS_LIST_SCOPE_KEY, "starred", "starred"),
            (db::CLUSTER_GROUP_ENABLED_KEY, "TRUE", "1"),
            (db::INTEREST_BOOST_ENABLED_KEY, "false", "0"),
            (db::FOCUS_RERANK_ENABLED_KEY, "1", "1"),
            (db::HIDE_NOISY_KEY, "0", "0"),
            (
                db::HIDE_THRESHOLD_KEY,
                "-9223372036854775808",
                "-9223372036854775808",
            ),
            (ai_store::DEVELOPER_TOOLS_KEY, "true", "1"),
            (ai_store::FETCH_URL_ENABLED_KEY, "false", "0"),
            (translations::TRANSLATE_TARGET_LANG_KEY, " EN-US ", "en-us"),
        ] {
            let saved = set(&conn, id, raw).unwrap();
            assert_eq!(saved.value, expected, "{id}");
            assert_eq!(
                db::get_setting(&conn, id).unwrap().as_deref(),
                Some(expected)
            );
            assert_eq!(value(&schema(&conn).unwrap(), id), expected);
        }
        // 旧库读取不改变数据；阈值旧函数不接受空白，语言空白沿用 zh。
        db::set_setting(&conn, db::HIDE_THRESHOLD_KEY, " 3 ").unwrap();
        db::set_setting(&conn, translations::TRANSLATE_TARGET_LANG_KEY, " ").unwrap();
        assert_eq!(
            value(&schema(&conn).unwrap(), db::HIDE_THRESHOLD_KEY),
            articles::hide_threshold(&conn).unwrap().to_string()
        );
        assert_eq!(
            value(
                &schema(&conn).unwrap(),
                translations::TRANSLATE_TARGET_LANG_KEY
            ),
            translations::target_lang(&conn).unwrap()
        );
        assert_eq!(
            db::get_setting(&conn, db::HIDE_THRESHOLD_KEY)
                .unwrap()
                .as_deref(),
            Some(" 3 ")
        );
    }

    #[test]
    fn feeds_pinned_normalizes_truncates_and_rejects() {
        let (_dir, conn) = conn();
        assert_eq!(
            set(
                &conn,
                db::FEEDS_PINNED_KEY,
                " folder:02 , feed:1 , folder:2 "
            )
            .unwrap()
            .value,
            "folder:2,feed:1"
        );
        let many = (1..=11)
            .map(|id| format!("feed:{id}"))
            .collect::<Vec<_>>()
            .join(",");
        let kept = (1..=10)
            .map(|id| format!("feed:{id}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(set(&conn, db::FEEDS_PINNED_KEY, &many).unwrap().value, kept);
        assert!(
            matches!(
                set(&conn, db::FEEDS_PINNED_KEY, "folder:0"),
                Err(rrss_core::Error::InvalidSetting(_))
            ) && db::get_setting(&conn, db::FEEDS_PINNED_KEY)
                .unwrap()
                .as_deref()
                == Some(kept.as_str())
        );
    }

    #[test]
    fn registered_language_write_keeps_configuration_revision_transaction() {
        let (_dir, conn) = conn();
        translations::set_backend(&conn, translations::ENGINE_HTTP).unwrap();
        db::set_setting(
            &conn,
            translations::TRANSLATE_HTTP_URL_KEY,
            "http://127.0.0.1:1",
        )
        .unwrap();
        set(&conn, translations::TRANSLATE_TARGET_LANG_KEY, "en").unwrap();
        assert_eq!(
            crate::configuration::translation_revision(&conn).unwrap(),
            1
        );
        set(&conn, translations::TRANSLATE_TARGET_LANG_KEY, "EN").unwrap();
        assert_eq!(
            crate::configuration::translation_revision(&conn).unwrap(),
            1
        );
        set(&conn, translations::TRANSLATE_TARGET_LANG_KEY, "zh").unwrap();
        assert_eq!(
            crate::configuration::translation_revision(&conn).unwrap(),
            2
        );
    }

    #[test]
    fn writes_are_validated_and_normalized() {
        let (_dir, conn) = conn();
        assert_eq!(set(&conn, "pi_auto_update", "true").unwrap().value, "1");
        assert_eq!(
            set(&conn, "ai_daily_budget_tokens", " 20000 ")
                .unwrap()
                .value,
            "20000"
        );
        assert_eq!(set(&conn, "ai_input_tier", "high").unwrap().value, "high");
        assert_eq!(
            set(&conn, "pi_assistant_note", " 说话简短 ").unwrap().value,
            "说话简短"
        );
        assert_eq!(pi_store::assistant_note(&conn).unwrap(), "说话简短");
        assert!(set(&conn, "pi_assistant_note", &"字".repeat(2001)).is_err());
        assert_eq!(ai_store::input_tier(&conn).unwrap(), InputTier::High);
        for (id, bad) in [
            ("ai_daily_budget_tokens", "-1"),
            ("ai_daily_budget_tokens", "lots"),
            ("digest_generate_minutes", "1440"),
            ("pi_cache_warming", "always"),
            ("ai_input_tier", "huge"),
            ("pi_auto_download", "maybe"),
            ("unknown_setting", "3"),
        ] {
            let err = set(&conn, id, bad).unwrap_err();
            assert!(
                matches!(err, rrss_core::Error::InvalidSetting(_)),
                "{id}={bad}: {err}"
            );
        }
        let schema = schema(&conn).unwrap();
        assert_eq!(
            value(&schema, "ai_daily_budget_tokens"),
            "20000",
            "rejected write kept the old value"
        );
        assert_eq!(value(&schema, "ai_input_tier"), "high");
        // 库里读不懂的旧值按默认值显示。
        db::set_setting(&conn, "pi_cache_warming", "weird").unwrap();
        assert_eq!(value(&schema_after(&conn), "pi_cache_warming"), "off");
    }

    #[test]
    fn spot_check_settings_validate_defaults_metadata_and_allowed_rates() {
        let (_dir, conn) = conn();
        let initial = schema(&conn).unwrap();
        for (key, default) in [
            (db::FOCUS_SHOW_DETAILS_KEY, "0"),
            (db::HIDDEN_SPOT_CHECK_RATE_KEY, "3"),
        ] {
            let spec = initial.specs.iter().find(|s| s.id == key).unwrap();
            assert!(matches!(spec.domain, SettingDomain::Reader));
            assert!(matches!(spec.level, SettingLevel::L0));
            assert_eq!(spec.default, default);
            assert_eq!(value(&initial, key), default);
        }
        assert_eq!(db::hidden_spot_check_rate(&conn).unwrap(), 3);
        assert!(
            !crate::queries::focus::ranking_settings(&conn)
                .unwrap()
                .show_details
        );
        set(&conn, db::FOCUS_SHOW_DETAILS_KEY, "true").unwrap();
        assert!(
            crate::queries::focus::ranking_settings(&conn)
                .unwrap()
                .show_details
        );
        assert!(set(&conn, db::FOCUS_SHOW_DETAILS_KEY, "maybe").is_err());
        for rate in db::HIDDEN_SPOT_CHECK_RATES {
            let raw = format!(" {rate} ");
            assert_eq!(
                set(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, &raw)
                    .unwrap()
                    .value,
                rate.to_string()
            );
            assert_eq!(db::hidden_spot_check_rate(&conn).unwrap(), rate);
        }
        for raw in ["-1", "2", "4", "6", "1.5", "bad"] {
            assert!(matches!(
                set(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, raw),
                Err(rrss_core::Error::InvalidSetting(_))
            ));
        }
        assert_eq!(db::hidden_spot_check_rate(&conn).unwrap(), 5);
        db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, "2").unwrap();
        assert_eq!(db::hidden_spot_check_rate(&conn).unwrap(), 3);
        assert_eq!(
            value(&schema(&conn).unwrap(), db::HIDDEN_SPOT_CHECK_RATE_KEY),
            "3"
        );
    }

    #[test]
    fn focus_columns_normalize_truncate_and_reject() {
        let (_dir, conn) = conn();
        assert_eq!(value(&schema(&conn).unwrap(), db::FOCUS_COLUMNS_KEY), "");
        assert_eq!(
            set(
                &conn,
                db::FOCUS_COLUMNS_KEY,
                " folder:2, folder:2, folder:10,folder:3,folder:4,folder:5,folder:9 ",
            )
            .unwrap()
            .value,
            "folder:2,folder:10,folder:3,folder:4,folder:5"
        );
        assert!(matches!(
            set(&conn, db::FOCUS_COLUMNS_KEY, "folder:1,feed:2"),
            Err(rrss_core::Error::InvalidSetting(_))
        ));
        assert_eq!(
            db::get_setting(&conn, db::FOCUS_COLUMNS_KEY)
                .unwrap()
                .as_deref(),
            Some("folder:2,folder:10,folder:3,folder:4,folder:5")
        );
    }

    #[test]
    fn rank_switches_default_on_and_apply_through_registry() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rank.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,published_at,created_at) VALUES(1,1,'old',0,0);",
        )
        .unwrap();
        for key in [
            db::RANK_FEATURES_ENABLED_KEY,
            db::RANK_SOURCE_ENABLED_KEY,
            db::RANK_COVERAGE_ENABLED_KEY,
            db::RANK_FRESHNESS_ENABLED_KEY,
            db::RANK_AFFINITY_ENABLED_KEY,
        ] {
            assert_eq!(value(&schema(&conn).unwrap(), key), "1");
            assert_eq!(set(&conn, key, "false").unwrap().value, "0");
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM article_rank", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        set(&conn, db::RANK_FRESHNESS_ENABLED_KEY, "true").unwrap();
        assert_eq!(
            conn.query_row("SELECT boost FROM article_rank", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            -2
        );
    }

    fn frontend_schema_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/settings/settingsSchemaData.ts")
    }

    fn frontend_schema_source() -> String {
        let specs = DEFS
            .iter()
            .map(spec)
            .map(|s| format!("  {},", serde_json::to_string(&s).unwrap()))
            .collect::<Vec<_>>()
            .join("\n");
        let ids = DEFS
            .iter()
            .map(|def| format!("{:?}", def.id))
            .collect::<Vec<_>>()
            .join(" | ");
        format!(
            "// Generated by rrss-app export_settings_schema. Do not edit.\n\n\
             import type {{ SettingSpec }} from \"../bindings\";\n\n\
             export type SettingId = {ids};\n\n\
             export const SETTING_SPECS: SettingSpec[] = [\n{specs}\n];\n"
        )
    }

    #[test]
    #[ignore = "explicit export: cargo test -p rrss-app export_settings_schema -- --ignored"]
    fn export_settings_schema() {
        std::fs::write(frontend_schema_path(), frontend_schema_source()).unwrap();
    }

    #[test]
    fn frontend_schema_is_current() {
        // Windows 检出会把换行转成 CRLF，只比内容。
        assert_eq!(
            std::fs::read_to_string(frontend_schema_path())
                .unwrap()
                .replace("\r\n", "\n"),
            frontend_schema_source(),
            "run cargo test -p rrss-app export_settings_schema -- --ignored"
        );
    }

    fn schema_after(conn: &Connection) -> SettingsSchema {
        schema(conn).unwrap()
    }
}
