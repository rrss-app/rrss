//! AI 输出语言：设置 `ai_output_language` 决定后台作业写给用户看的内容用哪种语言。
//! 提示词只有一份英文；这里在系统提示末尾追加一句语言指令，作业经 [`resolve_task`] 取到的模板正文
//! 已经带上它，所以缓存键、配置指纹与「配置是否变了」的比较都自动区分语言。

use rrss_core::ai_store::{self, OutputLanguage, ResolvedTask};
use rusqlite::Connection;

/// 设置项：`ui` 跟随界面语言（默认）/ `zh-CN` / `en`。
pub const SETTING: &str = "ai_output_language";
pub const FOLLOW_UI: &str = "ui";
pub const OPTIONS: &[&str] = &[FOLLOW_UI, "zh-CN", "en"];

/// 设置原值；认不出的按跟随界面处理。
pub fn setting(conn: &Connection) -> rrss_core::Result<String> {
    let raw = rrss_core::db::get_setting(conn, SETTING)?.unwrap_or_default();
    Ok(if OPTIONS.contains(&raw.as_str()) {
        raw
    } else {
        FOLLOW_UI.into()
    })
}

/// 当前输出语言：显式设置优先，跟随界面时取当前界面语言。
pub fn current(conn: &Connection) -> rrss_core::Result<OutputLanguage> {
    let setting = setting(conn)?;
    if setting != FOLLOW_UI {
        return Ok(OutputLanguage::from_tag(&setting));
    }
    Ok(OutputLanguage::from_tag(
        &crate::ui_language::current(conn)?.language,
    ))
}

/// 追加在系统提示末尾的语言指令。输入里的原文（标题、来源名、引文）不翻译。
pub fn directive(language: OutputLanguage) -> String {
    format!(
        "Output language: write every user-facing text field in {0}, whatever the language of the input. \
         When the input is in another language, translate what you use from it into {0}; do not copy its sentences. \
         Keep JSON keys, IDs and reference numbers unchanged, and keep product names, people's names and quoted source titles as they appear in the input; \
         when such a name is in a script a {0} reader may not read, add a short translation in parentheses the first time it appears.",
        language.english_name()
    )
}

/// 输出里有给用户看的文字的用途。选题的理由只给模型自己看，翻译按翻译的目标语言，都不加。
pub fn applies(task: &str) -> bool {
    matches!(
        task,
        ai_store::TASK_FOCUS_SUMMARY
            | ai_store::TASK_ARTICLE_SUMMARY
            | ai_store::TASK_ARTICLE_SEGMENT
            | ai_store::TASK_DIGEST_DAILY
            | ai_store::TASK_DIGEST_WEEKLY
            | ai_store::TASK_CLUSTER_SUMMARY
            | ai_store::TASK_EPISODE_COMPRESS
    )
}

/// 模板正文加上语言指令。
pub fn with_directive(body: &str, language: OutputLanguage) -> String {
    format!("{}\n\n{}", body.trim_end(), directive(language))
}

/// 后台作业解析用途一律经这里：输出给用户看的用途在模板正文末尾带上当前语言的指令。
pub fn resolve_task(conn: &Connection, task: &str) -> rrss_core::Result<ResolvedTask> {
    let mut resolved = ai_store::resolve_task(conn, task)?;
    if applies(task) {
        resolved.template.body = with_directive(&resolved.template.body, current(conn)?);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_setting_wins_and_follow_ui_reads_the_interface() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("t.db")).unwrap();
        assert_eq!(setting(&conn).unwrap(), FOLLOW_UI);
        crate::settings_schema::set(&conn, crate::ui_language::SETTING, "zh-CN").unwrap();
        assert_eq!(current(&conn).unwrap(), OutputLanguage::ZhCn);
        crate::settings_schema::set(&conn, crate::ui_language::SETTING, "en").unwrap();
        assert_eq!(current(&conn).unwrap(), OutputLanguage::En);
        crate::settings_schema::set(&conn, SETTING, "zh-CN").unwrap();
        assert_eq!(current(&conn).unwrap(), OutputLanguage::ZhCn);
        rrss_core::db::set_setting(&conn, SETTING, "ja").unwrap();
        assert_eq!(setting(&conn).unwrap(), FOLLOW_UI);
        assert!(crate::settings_schema::set(&conn, SETTING, "ja").is_err());
    }

    #[test]
    fn only_user_facing_tasks_get_the_directive() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("t.db")).unwrap();
        let provider = ai_store::insert_provider(
            &conn,
            &ai_store::NewProvider {
                name: "p".into(),
                base_url: "http://127.0.0.1:9".into(),
                api_key: None,
                protocol: "chat".into(),
            },
        )
        .unwrap();
        for task in [
            ai_store::TASK_FOCUS_SUMMARY,
            ai_store::TASK_DIGEST_PICK_DAILY,
            ai_store::TASK_ARTICLE_TRANSLATE,
        ] {
            ai_store::set_task_binding(&conn, task, Some(provider), Some("m")).unwrap();
        }
        crate::settings_schema::set(&conn, SETTING, "en").unwrap();
        let summary = resolve_task(&conn, ai_store::TASK_FOCUS_SUMMARY).unwrap();
        assert!(
            summary
                .template
                .body
                .ends_with(&directive(OutputLanguage::En))
        );
        crate::settings_schema::set(&conn, SETTING, "zh-CN").unwrap();
        let again = resolve_task(&conn, ai_store::TASK_FOCUS_SUMMARY).unwrap();
        assert!(again.template.body.contains("Simplified Chinese"));
        // 换语言就是换配置：在途作业据此作废，缓存也不会串。
        assert!(!crate::jobs::complete::same_task(&summary, &again));
        for task in [
            ai_store::TASK_DIGEST_PICK_DAILY,
            ai_store::TASK_ARTICLE_TRANSLATE,
        ] {
            let raw = ai_store::resolve_task(&conn, task).unwrap();
            assert_eq!(
                resolve_task(&conn, task).unwrap().template.body,
                raw.template.body
            );
        }
    }
}
