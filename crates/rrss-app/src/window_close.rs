//! 关闭主窗口时做什么（阶段 14 · 后台驻留）：设置 `close_action` 决定拦下关闭后是问、
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

fn resolve(choice: CloseChoice, tray_available: bool) -> CloseDecision {
    match choice {
        CloseChoice::Tray if tray_available => CloseDecision::Hide,
        CloseChoice::Tray => CloseDecision::Minimize,
        CloseChoice::Quit => CloseDecision::Quit,
    }
}

/// 按设置决定这次关闭怎么处理。
pub fn decide(conn: &Connection, tray_available: bool) -> rrss_core::Result<CloseDecision> {
    Ok(match settings_schema::current(conn, SETTING)?.as_str() {
        "tray" => resolve(CloseChoice::Tray, tray_available),
        "quit" => CloseDecision::Quit,
        _ => CloseDecision::Ask,
    })
}

/// 关闭询问的结果：勾了「记住」就写进设置，以后不再询问。
pub fn choose(
    conn: &Connection,
    choice: CloseChoice,
    remember: bool,
    tray_available: bool,
) -> rrss_core::Result<CloseDecision> {
    if remember {
        settings_schema::set(conn, SETTING, choice.setting())?;
    }
    Ok(resolve(choice, tray_available))
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
        assert_eq!(decide(&conn, true).unwrap(), CloseDecision::Ask);
        settings_schema::set(&conn, SETTING, "tray").unwrap();
        assert_eq!(decide(&conn, true).unwrap(), CloseDecision::Hide);
        assert_eq!(decide(&conn, false).unwrap(), CloseDecision::Minimize);
        settings_schema::set(&conn, SETTING, "quit").unwrap();
        assert_eq!(decide(&conn, true).unwrap(), CloseDecision::Quit);
        assert!(settings_schema::set(&conn, SETTING, "hide").is_err());
    }

    #[test]
    fn remembers_only_when_asked() {
        let (_dir, conn) = conn();
        assert_eq!(
            choose(&conn, CloseChoice::Quit, false, true).unwrap(),
            CloseDecision::Quit
        );
        assert_eq!(decide(&conn, true).unwrap(), CloseDecision::Ask);
        assert_eq!(
            choose(&conn, CloseChoice::Tray, true, false).unwrap(),
            CloseDecision::Minimize
        );
        assert_eq!(settings_schema::current(&conn, SETTING).unwrap(), "tray");
        assert_eq!(decide(&conn, true).unwrap(), CloseDecision::Hide);
    }
}
