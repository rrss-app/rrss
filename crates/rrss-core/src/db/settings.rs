use rusqlite::Connection;

use crate::error::Result;

pub const HIDE_THRESHOLD_KEY: &str = "hide_threshold";
pub const FOCUS_FILTER_KEY: &str = "focus_filter";
pub const INTEREST_BOOST_ENABLED_KEY: &str = "interest_boost_enabled";
pub const FOCUS_RERANK_ENABLED_KEY: &str = "focus_rerank_enabled";
pub const CLUSTER_GROUP_ENABLED_KEY: &str = "cluster_group_enabled";
pub const HIDE_NOISY_KEY: &str = "focus_hide_noisy";
pub const FOCUS_SUMMARY_NEUTRAL_LIMIT_KEY: &str = "focus_summary_neutral_limit";
pub const PI_IDLE_TIMEOUT_SECS_KEY: &str = "pi_idle_timeout_secs";
/// [`settings`] 里主题偏好的键。取值 `system` / `light` / `dark`。
pub const THEME_PREFERENCE_KEY: &str = "theme_preference";
pub const RANK_FEATURES_ENABLED_KEY: &str = "rank_features_enabled";
pub const RANK_SOURCE_ENABLED_KEY: &str = "rank_source_enabled";
pub const RANK_COVERAGE_ENABLED_KEY: &str = "rank_coverage_enabled";
pub const RANK_FRESHNESS_ENABLED_KEY: &str = "rank_freshness_enabled";
pub const RANK_AFFINITY_ENABLED_KEY: &str = "rank_affinity_enabled";
pub const FOCUS_SHOW_DETAILS_KEY: &str = "focus_show_details";
pub const FOCUS_COLUMNS_KEY: &str = "focus_columns";
pub const HIDDEN_SPOT_CHECK_RATE_KEY: &str = "hidden_spot_check_rate";
pub const HIDDEN_SPOT_CHECK_RATES: [i64; 4] = [0, 1, 3, 5];
pub const LIST_SCOPE_KEY: &str = "list_scope";
pub const FEEDS_SCOPE_KEY: &str = "feeds_scope";
pub const FEEDS_LIST_SCOPE_KEY: &str = "feeds_list_scope";
pub const FEEDS_PINNED_KEY: &str = "feeds_pinned";
/// 每个站点（主机 + 端口）同时下载几个订阅源。
pub const FEED_FETCH_PER_HOST_KEY: &str = "feed_fetch_per_host";
pub const FEED_FETCH_PER_HOST_DEFAULT: usize = 4;
pub const FEED_FETCH_PER_HOST_RANGE: (usize, usize) = (1, 8);

/// 读每站同时下载数；缺省或无法解析时用默认值，越界时夹到范围内。
pub fn feed_fetch_per_host(conn: &Connection) -> Result<usize> {
    let (low, high) = FEED_FETCH_PER_HOST_RANGE;
    Ok(get_setting(conn, FEED_FETCH_PER_HOST_KEY)?
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(FEED_FETCH_PER_HOST_DEFAULT)
        .clamp(low, high))
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare_cached("SELECT value FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    if let Some(row) = rows.next()? {
        let val: String = row.get(0)?;
        Ok(Some(val))
    } else {
        Ok(None)
    }
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

pub fn setting_enabled(conn: &Connection, key: &str, default: bool) -> Result<bool> {
    match get_setting(conn, key)? {
        Some(v) => match v.trim() {
            "1" | "true" | "TRUE" => Ok(true),
            "0" | "false" | "FALSE" => Ok(false),
            _ => Ok(default),
        },
        None => Ok(default),
    }
}

pub fn set_setting_enabled(conn: &Connection, key: &str, enabled: bool) -> Result<()> {
    set_setting(conn, key, if enabled { "1" } else { "0" })
}

pub fn hidden_spot_check_rate(conn: &Connection) -> Result<i64> {
    Ok(get_setting(conn, HIDDEN_SPOT_CHECK_RATE_KEY)?
        .and_then(|s| s.trim().parse().ok())
        .filter(|rate| HIDDEN_SPOT_CHECK_RATES.contains(rate))
        .unwrap_or(3))
}

pub fn theme_preference(conn: &Connection) -> Result<String> {
    let raw = get_setting(conn, THEME_PREFERENCE_KEY)?.unwrap_or_else(|| "system".into());
    match raw.as_str() {
        "light" | "dark" => Ok(raw),
        _ => Ok("system".into()),
    }
}

pub fn set_theme_preference(conn: &Connection, value: &str) -> Result<()> {
    let norm = match value {
        "light" | "dark" => value,
        _ => "system",
    };
    set_setting(conn, THEME_PREFERENCE_KEY, norm)
}

/// B1 / B2 五个分量默认开启；未写过的新旧库都使用相同默认值。
pub fn rank_switches(conn: &Connection) -> Result<crate::rank::combine::Switches> {
    Ok(crate::rank::combine::Switches {
        features: setting_enabled(conn, RANK_FEATURES_ENABLED_KEY, true)?,
        source: setting_enabled(conn, RANK_SOURCE_ENABLED_KEY, true)?,
        coverage: setting_enabled(conn, RANK_COVERAGE_ENABLED_KEY, true)?,
        freshness: setting_enabled(conn, RANK_FRESHNESS_ENABLED_KEY, true)?,
        affinity: setting_enabled(conn, RANK_AFFINITY_ENABLED_KEY, true)?,
    })
}
