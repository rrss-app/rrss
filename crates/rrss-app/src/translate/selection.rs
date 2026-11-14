//! 划词翻译只读配置。不写译文、不读写补全缓存、不回写协议；大模型用量计入今日用量。
use super::{Config, TranslateBackend, cache, spawn_db, translate_units, window};
use crate::views::{
    SelectionTranslation, SelectionTranslationConfig, TranslationCommandError, TranslationFailure,
};
use rrss_core::translations;
use rrss_render::TranslateUnit;
use rusqlite::Connection;
use std::path::Path;

/// Unicode scalar 数量；由 export_ts 同时导出前端常量。
pub const MAX_CHARS: usize = 2000;

pub fn configuration(conn: &Connection) -> rrss_core::Result<Option<SelectionTranslationConfig>> {
    if !translations::is_enabled(conn)? {
        return Ok(None);
    }
    let Some(config) = Config::optional(conn)? else {
        return Ok(None);
    };
    if matches!(&config.backend, TranslateBackend::DeepL { api_key, .. } if api_key.as_deref().is_none_or(|key| key.trim().is_empty()))
    {
        return Ok(None);
    }
    let service = match &config.backend {
        TranslateBackend::Complete => config.complete.as_ref().unwrap().provider.name.clone(),
        TranslateBackend::Http { .. } => "HTTP 服务".into(),
        TranslateBackend::DeepLx { .. } => "DeepLX".into(),
        TranslateBackend::DeepL { .. } => "DeepL".into(),
    };
    Ok(cache::configuration_fingerprint(conn)?
        .map(|identity| SelectionTranslationConfig { identity, service }))
}

pub async fn translate_selection(
    path: &Path,
    client: &reqwest::Client,
    text: String,
    config_identity: String,
) -> Result<SelectionTranslation, TranslationCommandError> {
    if text.chars().count() > MAX_CHARS {
        return Err(TranslationCommandError {
            kind: TranslationFailure::InvalidOutput,
            message: format!("选中文字超过 {MAX_CHARS} 字，请改用全文翻译。"),
            error: None,
        });
    }
    if text.trim().is_empty() {
        return Err(TranslationFailure::InvalidOutput.into());
    }
    let (config, identity) = spawn_db(path.to_owned(), |conn| {
        let tx = conn.unchecked_transaction()?;
        Ok((Config::optional(&tx)?, configuration(&tx)?))
    })
    .await
    .map_err(|err| TranslationCommandError::from(window::classify(&err)))?;
    let (Some(config), Some(identity)) = (config, identity) else {
        return Err(TranslationFailure::Configuration.into());
    };
    // 前端读配置与发请求之间可能发生写入，拒绝用新配置替旧缓存键翻译。
    if identity.identity != config_identity {
        return Err(TranslationFailure::Invalidated.into());
    }
    let units = [TranslateUnit { i: 0, t: text }];
    let out = tokio::time::timeout(
        super::runtime::WINDOW_TIMEOUT,
        translate_units(&config, path, client, &units, true),
    )
    .await
    .map_err(|_| TranslationCommandError::from(TranslationFailure::Timeout))?
    .map_err(|err| TranslationCommandError::from(window::classify(&err)))?;
    super::validate_units(&units, &out)
        .map_err(|_| TranslationCommandError::from(TranslationFailure::InvalidOutput))?;
    Ok(SelectionTranslation {
        text: out.into_iter().next().unwrap().t,
        config: identity,
    })
}

#[cfg(test)]
mod tests;
