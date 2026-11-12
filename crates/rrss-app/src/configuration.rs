//! 模型与翻译配置的写入边界：统一事务、派生版本与提交后的通知。
use crate::translate::cache::configuration_fingerprint as fingerprint;
use rrss_core::{Result, db};
use rusqlite::Connection;

const KEY: &str = "translation_config_revision";

pub(crate) fn translation_revision(conn: &Connection) -> Result<u64> {
    Ok(db::get_setting(conn, KEY)?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0))
}

struct Change(Option<String>);
impl Change {
    fn begin(conn: &Connection) -> Result<Self> {
        Ok(Self(fingerprint(conn)?))
    }
    /// 调用者已持有写事务；失败写入的回滚也回滚版本号。
    fn finish(self, conn: &Connection) -> Result<Option<u64>> {
        if self.0 == fingerprint(conn)? {
            return Ok(None);
        }
        let revision = translation_revision(conn)?
            .checked_add(1)
            .ok_or_else(|| rrss_core::Error::InvalidSetting("翻译配置版本已耗尽".into()))?;
        db::set_setting(conn, KEY, &revision.to_string())?;
        Ok(Some(revision))
    }
}

fn notify(conn: &Connection, revision: Option<u64>) {
    if let (Some(path), Some(revision)) = (conn.path(), revision) {
        crate::translate::runtime::configuration_changed(std::path::Path::new(path), revision);
    }
}

/// 有效配置与代际计数原子提交，通知不在数据库事务内调用。
pub(crate) fn write<T>(
    conn: &Connection,
    action: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let tx = db::write_transaction(conn)?;
    let change = Change::begin(&tx)?;
    let result = action(&tx)?;
    let revision = change.finish(&tx)?;
    tx.commit()?;
    notify(conn, revision);
    Ok(result)
}
