//! 界面语言（阶段 27 · I4）：设置 `ui_language` 与系统偏好的语言一起解析成界面语言。
//! 前端在首帧前取一次；I3 的「AI 输出语言」跟随界面时也用这里的解析，后台作业不经前端也能拿到。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 设置项：`system` 跟随系统（默认）/ `zh-CN` / `en`。
pub const SETTING: &str = "ui_language";
pub const FOLLOW_SYSTEM: &str = "system";
/// 设置的取值；除 `system` 外都是界面支持的语言。
pub const OPTIONS: &[&str] = &[FOLLOW_SYSTEM, "zh-CN", "en"];
/// 系统语言一个都不认识时用的语言。
pub const FALLBACK: &str = "en";

/// 首帧要用的语言：设置原值与解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UiLanguageView {
    pub setting: String,
    pub language: String,
}

/// 显式设置优先；跟随系统时取系统列表里第一个认识的（`zh*` → `zh-CN`，`en*` → `en`），都不认识落到英文。
pub fn resolve(setting: &str, system: &[String]) -> &'static str {
    if let Some(language) = OPTIONS
        .iter()
        .copied()
        .find(|o| *o != FOLLOW_SYSTEM && *o == setting)
    {
        return language;
    }
    system
        .iter()
        .find_map(|tag| supported(tag))
        .unwrap_or(FALLBACK)
}

fn supported(tag: &str) -> Option<&'static str> {
    let primary = tag.trim().split(['-', '_']).next()?.to_ascii_lowercase();
    match primary.as_str() {
        // 繁体与港台变体先落到简体中文，等有繁体语言包再分。
        "zh" => Some("zh-CN"),
        "en" => Some("en"),
        _ => None,
    }
}

/// 翻译目标语言跟随界面时的语言代码（翻译用两位代码：`zh`、`en`）。
pub fn translation_target(language: &str) -> &'static str {
    if language.starts_with("zh") {
        "zh"
    } else {
        "en"
    }
}

/// 翻译实际用的目标语言：设置为跟随界面（`auto`）时按当前界面语言取。
pub fn translate_target(conn: &Connection) -> rrss_core::Result<String> {
    let raw = rrss_core::translations::target_lang(conn)?;
    if raw != rrss_core::translations::TARGET_FOLLOW_UI {
        return Ok(raw);
    }
    Ok(translation_target(&current(conn)?.language).into())
}

pub fn setting(conn: &Connection) -> rrss_core::Result<String> {
    let raw = rrss_core::db::get_setting(conn, SETTING)?.unwrap_or_default();
    Ok(if OPTIONS.contains(&raw.as_str()) {
        raw
    } else {
        FOLLOW_SYSTEM.into()
    })
}

/// 当前界面语言：读设置，跟随系统时问 `rrss_core::platform`。
pub fn current(conn: &Connection) -> rrss_core::Result<UiLanguageView> {
    let setting = setting(conn)?;
    let language = resolve(&setting, &rrss_core::platform::ui_languages()).to_string();
    Ok(UiLanguageView { setting, language })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn explicit_setting_wins_over_system() {
        assert_eq!(resolve("en", &tags(&["zh-CN"])), "en");
        assert_eq!(resolve("zh-CN", &tags(&["en-US"])), "zh-CN");
    }

    #[test]
    fn follow_system_takes_the_first_known_language() {
        assert_eq!(
            resolve("system", &tags(&["de-DE", "zh-TW", "en-US"])),
            "zh-CN"
        );
        assert_eq!(resolve("system", &tags(&["en-GB", "zh-CN"])), "en");
        assert_eq!(resolve("system", &tags(&["zh_Hant_HK"])), "zh-CN");
        assert_eq!(resolve("system", &tags(&["fr-FR"])), "en");
        assert_eq!(resolve("system", &[]), "en");
        // 认不出的设置值按跟随系统处理。
        assert_eq!(resolve("ja", &tags(&["zh-CN"])), "zh-CN");
    }

    #[test]
    fn translation_follows_the_interface_unless_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("t.db")).unwrap();
        crate::settings_schema::set(&conn, SETTING, "en").unwrap();
        assert_eq!(translate_target(&conn).unwrap(), "en");
        crate::settings_schema::set(&conn, SETTING, "zh-CN").unwrap();
        assert_eq!(translate_target(&conn).unwrap(), "zh");
        rrss_core::translations::set_target_lang(&conn, "ja").unwrap();
        assert_eq!(translate_target(&conn).unwrap(), "ja");
    }

    #[test]
    fn unknown_stored_values_read_as_follow_system() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("t.db")).unwrap();
        assert_eq!(setting(&conn).unwrap(), "system");
        rrss_core::db::set_setting(&conn, SETTING, "ja").unwrap();
        assert_eq!(setting(&conn).unwrap(), "system");
        crate::settings_schema::set(&conn, SETTING, "en").unwrap();
        assert_eq!(setting(&conn).unwrap(), "en");
        assert!(crate::settings_schema::set(&conn, SETTING, "ja").is_err());
    }
}
