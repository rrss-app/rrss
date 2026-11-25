//! 模型与翻译配置的写入边界：统一事务、派生版本与提交后的通知。
use crate::translate::cache::configuration_fingerprint as fingerprint;
use rrss_core::error::settings::SettingProblem;
use rrss_core::{Result, db};
use rusqlite::Connection;

const KEY: &str = "translation_config_revision";

pub(crate) fn translation_revision(conn: &Connection) -> Result<u64> {
    Ok(db::get_setting(conn, KEY)?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0))
}

struct Change {
    fingerprint: Option<String>,
    concurrency: Vec<(String, i64)>,
}

impl Change {
    fn begin(conn: &Connection) -> Result<Self> {
        Ok(Self {
            fingerprint: fingerprint(conn)?,
            concurrency: rrss_core::ai_store::provider_concurrency_limits(conn)?,
        })
    }
    /// 调用者已持有写事务；失败写入的回滚也回滚版本号。
    fn finish(self, conn: &Connection) -> Result<(Option<u64>, bool)> {
        let providers_changed =
            self.concurrency != rrss_core::ai_store::provider_concurrency_limits(conn)?;
        if self.fingerprint == fingerprint(conn)? {
            return Ok((None, providers_changed));
        }
        let revision = translation_revision(conn)?.checked_add(1).ok_or_else(|| {
            rrss_core::Error::InvalidSetting(SettingProblem::TranslationRevisionExhausted)
        })?;
        db::set_setting(conn, KEY, &revision.to_string())?;
        Ok((Some(revision), providers_changed))
    }
}

fn notify(conn: &Connection, revision: Option<u64>, providers_changed: bool) {
    let Some(path) = conn.path().filter(|path| !path.is_empty()) else {
        return;
    };
    let path = std::path::Path::new(path);
    // 并发上限不进入译文身份，但仅当连接地址或并发上限实际变化时才作废调度缓存，
    // 避免普通设置或 ai_cache 写入在每次补全完成后反复重读数据库。
    if providers_changed {
        crate::scheduler::model_capacity::Capacity::providers_changed(path);
    }
    if let Some(revision) = revision {
        crate::translate::runtime::configuration_changed(path, revision);
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
    let (revision, providers_changed) = change.finish(&tx)?;
    tx.commit()?;
    notify(conn, revision, providers_changed);
    Ok(result)
}
