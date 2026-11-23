//! 主窗口关闭行为策略：设置 `close_action` 决定拦下关闭后是问、
//! 隐藏到托盘还是退出。宿主只按这里的结论动窗口，不自己读设置。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::settings_schema;

/// 设置项：`ask` 每次询问（默认）/ `tray` 隐藏到托盘 / `quit` 退出程序。
pub const SETTING: &str = "close_action";

/// 关闭询问里用户的选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CloseChoice {
    Tray,
    Quit,
}

impl CloseChoice {
    fn setting(self) -> &'static str {
        match self {
            Self::Tray => "tray",
            Self::Quit => "quit",
        }
    }
}

/// 拦下关闭之后宿主要做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// 交给前端弹关闭询问，窗口不动。
    Ask,
    /// 隐藏窗口，留在托盘。
    Hide,
    /// 托盘不可用：退化为最小化，免得窗口找不回来。
    Minimize,
    Quit,
}

/// Close behavior loaded at startup; the host updates its copy after committed settings writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    Ask,
    Tray,
    Quit,
}

impl CloseAction {
    pub fn parse(value: &str) -> Self {
        match value {
            "tray" => Self::Tray,
            "quit" => Self::Quit,
            _ => Self::Ask,
        }
    }

    /// 按这个设置处理一次关闭。
    pub fn decision(self, tray_available: bool) -> CloseDecision {
        match self {
            Self::Ask => CloseDecision::Ask,
            Self::Tray if tray_available => CloseDecision::Hide,
            Self::Tray => CloseDecision::Minimize,
            Self::Quit => CloseDecision::Quit,
        }
    }
}

impl From<CloseChoice> for CloseAction {
    fn from(choice: CloseChoice) -> Self {
        match choice {
            CloseChoice::Tray => Self::Tray,
            CloseChoice::Quit => Self::Quit,
        }
    }
}

/// 读当前设置。
pub fn action(conn: &Connection) -> rrss_core::Result<CloseAction> {
    Ok(CloseAction::parse(&settings_schema::current(
        conn, SETTING,
    )?))
}

/// 关闭询问里勾了「记住」：写进设置，以后不再询问。
pub fn remember(conn: &Connection, choice: CloseChoice) -> rrss_core::Result<()> {
    settings_schema::set(conn, SETTING, choice.setting())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("t.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn asks_by_default_and_follows_the_setting() {
        let (_dir, conn) = conn();
        assert_eq!(action(&conn).unwrap(), CloseAction::Ask);
        assert_eq!(action(&conn).unwrap().decision(true), CloseDecision::Ask);
        settings_schema::set(&conn, SETTING, "tray").unwrap();
        assert_eq!(action(&conn).unwrap().decision(true), CloseDecision::Hide);
        assert_eq!(
            action(&conn).unwrap().decision(false),
            CloseDecision::Minimize
        );
        settings_schema::set(&conn, SETTING, "quit").unwrap();
        assert_eq!(action(&conn).unwrap().decision(true), CloseDecision::Quit);
        assert!(settings_schema::set(&conn, SETTING, "hide").is_err());
    }

    #[test]
    fn remembers_the_choice() {
        let (_dir, conn) = conn();
        remember(&conn, CloseChoice::Tray).unwrap();
        assert_eq!(settings_schema::current(&conn, SETTING).unwrap(), "tray");
        assert_eq!(action(&conn).unwrap(), CloseAction::Tray);
        remember(&conn, CloseChoice::Quit).unwrap();
        assert_eq!(action(&conn).unwrap(), CloseAction::Quit);
    }

    #[test]
    fn parses_committed_values_and_unknown_values_ask() {
        assert_eq!(CloseAction::parse("ask"), CloseAction::Ask);
        assert_eq!(CloseAction::parse("tray"), CloseAction::Tray);
        assert_eq!(CloseAction::parse("quit"), CloseAction::Quit);
        assert_eq!(CloseAction::parse("unknown"), CloseAction::Ask);
    }
}
