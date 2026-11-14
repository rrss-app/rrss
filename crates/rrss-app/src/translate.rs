//! 翻译**后端**：只吃 [`TranslateUnit`]，不碰 Block IR。
//!
//! 富文本抽取 / 嵌回在 `rrss_render`（`extract_units` / `apply_units`）。
//! 换在线服务时只加 [`TranslateBackend`] 变体，作业与阅读器不用改 IR 逻辑。
//! 日志 target：`rrss_app::translate`。

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use rrss_ai::translate_json_schema;
use rrss_core::ai_store::{self, TASK_ARTICLE_TRANSLATE};
use rrss_core::articles;
use rrss_core::translations::{
    self, ArticleTranslation, ENGINE_COMPLETE, ENGINE_DEEPL, ENGINE_DEEPLX, ENGINE_HTTP,
};
use rrss_render::TranslateUnit;
use serde_json::Value;

use crate::jobs::{
    complete::{Prepared, execute_prepared_with_policy},
    spawn_db,
};
use crate::scheduler::model_capacity::{Priority, RequestAdmission};

pub(crate) mod cache;
pub mod ipc;
pub mod runtime;
pub mod selection;
mod types;
mod window;
use cache::Config;

const COMPLETE_CHUNK_CHARS: usize = 2800;
/// DeepL 开启 XML 占位后的缓存世代；低于此值的译文会在下次点「翻译」时重拉。
const DEEPL_XML_PROMPT_VERSION: i64 = 2;

#[cfg(test)]
mod cache_tests;

/// 纯文本翻译服务。IR 往返不走这里。
#[derive(Clone, PartialEq, Eq)]
pub enum TranslateBackend {
    /// 用户已配置的 OpenAI 兼容补全（当前默认）。
    Complete,
    /// LibreTranslate 兼容：`POST {url}/translate`，body `q` / `source` / `target`。
    Http {
        base_url: String,
        api_key: Option<String>,
    },
    /// DeepLX：`POST {url}/translate`，body `text` / `source_lang` / `target_lang`。
    DeepLx {
        base_url: String,
        api_key: Option<String>,
    },
    /// DeepL 官方 API：`POST {url}/v2/translate`，Header `Authorization: DeepL-Auth-Key {key}`。
    DeepL {
        base_url: Option<String>,
        api_key: Option<String>,
    },
}

impl std::fmt::Debug for TranslateBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranslateBackend")
            .field("kind", &self.id())
            .finish()
    }
}

impl TranslateBackend {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Complete => ENGINE_COMPLETE,
            Self::Http { .. } => ENGINE_HTTP,
            Self::DeepLx { .. } => ENGINE_DEEPLX,
            Self::DeepL { .. } => ENGINE_DEEPL,
        }
    }
}

pub fn selected_backend(conn: &rusqlite::Connection) -> rrss_core::Result<TranslateBackend> {
    Ok(match translations::backend_id(conn)?.as_str() {
        ENGINE_HTTP => {
            let (url, key) = translations::http_config(conn)?;
            TranslateBackend::Http {
                base_url: url,
                api_key: key,
            }
        }
        ENGINE_DEEPLX => {
            let (url, token) = translations::deeplx_config(conn)?;
            TranslateBackend::DeepLx {
                base_url: url,
                api_key: token,
            }
        }
        ENGINE_DEEPL => {
            let (url, key) = translations::deepl_config(conn)?;
            TranslateBackend::DeepL {
                base_url: url,
                api_key: key,
            }
        }
        _ => TranslateBackend::Complete,
    })
}

/// 把一批单元交给后端。切窗、协议、鉴权都是后端自己的事。
async fn translate_units(
    config: &Config,
    db_path: &Path,
    client: &reqwest::Client,
    units: &[TranslateUnit],
    ephemeral: bool,
) -> Result<Vec<TranslateUnit>> {
    let batches = if matches!(config.backend, TranslateBackend::DeepLx { .. }) {
        units.iter().map(|unit| vec![unit.clone()]).collect()
    } else {
        chunk_units(units, COMPLETE_CHUNK_CHARS)
    };
    let mut merged = Vec::new();
    for batch in batches {
        let result = if ephemeral && config.backend == TranslateBackend::Complete {
            complete_ephemeral(db_path, client, config, &batch).await
        } else if config.backend == TranslateBackend::Complete {
            complete_window(db_path, client, config, &batch, false, |value| {
                let translated = parse_units(value)?;
                validate_units(&batch, &translated)?;
                Ok((translated, true))
            })
            .await
        } else {
            non_complete_window(db_path, client, config, &batch).await
        };
        if ephemeral {
            merged.extend(result?);
            continue;
        }
        merged.extend(result.map_err(|e| redact_error(e, &config.backend))?);
    }
    Ok(merged)
}

fn redact_error(error: anyhow::Error, backend: &TranslateBackend) -> anyhow::Error {
    let mut ui = crate::views::UiError::from_anyhow_ref(&error);
    if ui.code() == "legacy_text" {
        anyhow::anyhow!(redact_backend(&format!("{error:#}"), backend))
    } else {
        ui.detail = redact_backend(&ui.detail, backend);
        anyhow::Error::new(ui)
    }
}

async fn non_complete_window(
    db_path: &Path,
    client: &reqwest::Client,
    config: &Config,
    units: &[TranslateUnit],
) -> Result<Vec<TranslateUnit>> {
    use rrss_ai::admission::Admission;
    let base = match &config.backend {
        TranslateBackend::Http { base_url, .. } | TranslateBackend::DeepLx { base_url, .. } => {
            base_url.clone()
        }
        TranslateBackend::DeepL { base_url, api_key } => {
            resolve_deepl_official_url(base_url.as_deref(), api_key.as_deref().unwrap_or_default())
        }
        TranslateBackend::Complete => unreachable!("complete has its own checked response path"),
    };
    let admission = RequestAdmission::new(db_path, &base, Priority::Interactive, false)?;
    let _permit = admission.acquire().await?;
    match &config.backend {
        TranslateBackend::Http { base_url, api_key } => {
            http_translate(client, base_url, api_key.as_deref(), &config.lang, units).await
        }
        TranslateBackend::DeepLx { base_url, api_key } => {
            deeplx_translate(client, base_url, api_key.as_deref(), &config.lang, units).await
        }
        TranslateBackend::DeepL { base_url, api_key } => {
            deepl_official_translate(
                client,
                base_url.as_deref(),
                api_key.as_deref(),
                &config.lang,
                units,
            )
            .await
        }
        TranslateBackend::Complete => unreachable!("complete has its own checked response path"),
    }
}

fn redact_backend(text: &str, backend: &TranslateBackend) -> String {
    let key = match backend {
        TranslateBackend::Complete => None,
        TranslateBackend::Http { api_key, .. }
        | TranslateBackend::DeepLx { api_key, .. }
        | TranslateBackend::DeepL { api_key, .. } => api_key.as_deref(),
    };
    rrss_core::logsafe::scrub_secrets_with(text, key)
}

/// 划词只记今日用量。`cached = None` 不读缓存；这里也不写缓存、不回写探测到的协议、不写译文。
async fn complete_ephemeral(
    db_path: &Path,
    client: &reqwest::Client,
    config: &Config,
    batch: &[TranslateUnit],
) -> Result<Vec<TranslateUnit>> {
    let task = config
        .complete
        .as_ref()
        .context("missing translation configuration")?;
    let (usage_send, mut usage_receive) = tokio::sync::mpsc::unbounded_channel::<(u32, u32)>();
    let usage_path = db_path.to_path_buf();
    let settling = crate::scheduler::model_capacity::Capacity::for_database(db_path).work();
    let usage_task = tokio::spawn(async move {
        let _settling = settling;
        while let Some((prompt, completion)) = usage_receive.recv().await {
            let day = ai_store::local_day();
            spawn_db(usage_path.clone(), move |conn| {
                ai_store::add_usage(conn, &day, i64::from(prompt), i64::from(completion))?;
                Ok(())
            })
            .await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let admission = RequestAdmission::new(
        db_path,
        &task.provider.base_url,
        Priority::Interactive,
        false,
    )?;
    let out = rrss_ai::complete::complete_admitted_until(
        client,
        &rrss_ai::CompleteRequest {
            base_url: task.provider.base_url.clone(),
            api_key: task.provider.api_key.clone(),
            model: task.model.clone(),
            system: task.template.body.clone(),
            user: format!(
                "target_lang: {}\n{}",
                config.lang,
                serde_json::to_string(batch)?
            ),
            json_schema: Some(translate_json_schema()),
            protocol: rrss_ai::Protocol::parse(&task.provider.protocol),
        },
        None,
        &|prompt, completion| {
            let _ = usage_send.send((prompt, completion));
        },
        tokio::time::Instant::now() + rrss_ai::deadline::ITEM_TIMEOUT,
        Some(&admission),
    )
    .await;
    drop(usage_send);
    usage_task.await??;
    let translated = parse_units(&out?.parsed)?;
    validate_units(batch, &translated)?;
    Ok(translated)
}

async fn complete_window<T>(
    db_path: &Path,
    client: &reqwest::Client,
    config: &Config,
    units: &[TranslateUnit],
    bypass_cache: bool,
    check: impl FnOnce(&Value) -> Result<(T, bool)>,
) -> Result<T> {
    let payload = format!(
        "target_lang: {}\n{}",
        config.lang,
        serde_json::to_string(units)?
    );
    let resolved = config
        .complete
        .clone()
        .context("missing translation configuration")?;
    let key = ai_store::cache_key(
        &format!("{TASK_ARTICLE_TRANSLATE}:{}", config.hash),
        &resolved.model,
        resolved.template.version,
        &ai_store::hash_input(&payload),
    );
    let prepared = spawn_db(db_path.to_path_buf(), move |conn| {
        let cached = if bypass_cache {
            None
        } else {
            ai_store::cache_get(conn, &key)?
        };
        Ok(Prepared {
            provider: resolved.provider.clone(),
            resolved,
            cache_key: key,
            cached,
            write_detected_protocol: false,
            auto: false,
        })
    })
    .await?;
    execute_prepared_with_policy(
        db_path,
        client,
        prepared,
        &payload,
        translate_json_schema(),
        check,
    )
    .await
}

/// 在读取正文前拒绝代理认证响应；网络错误只保留安全类别及超时标记。
async fn read_service_response(resp: reqwest::Response) -> Result<(reqwest::StatusCode, String)> {
    let status = resp.status();
    if status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return Err(anyhow::Error::new(window::HttpStatus(status.as_u16())));
    }
    let text = resp.text().await.map_err(rrss_ai::Error::from)?;
    Ok((status, text))
}

async fn http_translate(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    target_lang: &str,
    units: &[TranslateUnit],
) -> Result<Vec<TranslateUnit>> {
    let url = http_translate_url(base_url)?;
    let target = http_target_lang(target_lang);
    let mut merged = Vec::new();
    let batch = units;
    let q: Vec<String> = batch.iter().map(|u| u.t.clone()).collect();
    let mut body = serde_json::json!({
        "q": if q.len() == 1 { Value::String(q[0].clone()) } else { Value::from(q.clone()) },
        "source": "auto",
        "target": target,
        "format": "text",
    });
    if let Some(key) = api_key.filter(|s| !s.is_empty()) {
        body["api_key"] = Value::String(key.to_string());
    }
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(rrss_ai::Error::from)
        .context("请求翻译端点失败")?;
    let (status, text) = read_service_response(resp)
        .await
        .context("读取翻译响应失败")?;
    if !status.is_success() {
        return Err(anyhow::Error::new(window::HttpStatus(status.as_u16()))
            .context(format!("翻译端点：{}", truncate_err(&text))));
    }
    let parsed: Value = serde_json::from_str(&text).context("翻译响应不是 JSON")?;
    let texts = parse_libretranslate_texts(&parsed, batch.len())?;
    for (unit, t) in batch.into_iter().zip(texts) {
        merged.push(TranslateUnit { i: unit.i, t });
    }
    Ok(merged)
}

async fn deeplx_translate(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    target_lang: &str,
    units: &[TranslateUnit],
) -> Result<Vec<TranslateUnit>> {
    let url = http_translate_url(base_url)?;
    let target = deepl_target_lang(target_lang);
    let mut merged = Vec::new();
    anyhow::ensure!(units.len() == 1, "DeepLX requires one unit per window");
    let unit = &units[0];
    let mut req = client.post(&url).json(&serde_json::json!({
        "text": unit.t,
        "source_lang": "auto",
        "target_lang": target,
    }));
    if let Some(key) = api_key.filter(|s| !s.is_empty()) {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req
        .send()
        .await
        .map_err(rrss_ai::Error::from)
        .context("请求 DeepLX 失败")?;
    let (status, text) = read_service_response(resp)
        .await
        .context("读取 DeepLX 响应失败")?;
    if !status.is_success() {
        return Err(anyhow::Error::new(window::HttpStatus(status.as_u16()))
            .context(format!("DeepLX：{}", truncate_err(&text))));
    }
    let parsed: Value = serde_json::from_str(&text).context("DeepLX 响应不是 JSON")?;
    merged.push(TranslateUnit {
        i: unit.i,
        t: parse_deeplx_text(&parsed)?,
    });
    Ok(merged)
}

async fn deepl_official_translate(
    client: &reqwest::Client,
    base_url: Option<&str>,
    api_key: Option<&str>,
    target_lang: &str,
    units: &[TranslateUnit],
) -> Result<Vec<TranslateUnit>> {
    let key = api_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("请在设置 → 翻译中填写 DeepL API Key"))?;
    let url = resolve_deepl_official_url(base_url, key);
    let target = deepl_official_target_lang(target_lang);
    let mut merged = Vec::new();
    let batch = units;
    let texts: Vec<String> = batch.iter().map(|u| u.t.clone()).collect();
    let body = deepl_official_body(&texts, &target);
    let resp = client
        .post(&url)
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .json(&body)
        .send()
        .await
        .map_err(rrss_ai::Error::from)
        .context("请求 DeepL 官方端点失败")?;
    let (status, text) = read_service_response(resp)
        .await
        .context("读取 DeepL 官方响应失败")?;
    if !status.is_success() {
        let detail = parse_deepl_error(&text).unwrap_or_else(|| truncate_err(&text));
        return Err(anyhow::Error::new(window::HttpStatus(status.as_u16()))
            .context(format!("DeepL：{detail}")));
    }
    let parsed: Value = serde_json::from_str(&text).context("DeepL 官方响应不是 JSON")?;
    let translated_texts = parse_deepl_official_translations(&parsed, batch.len())?;
    for (unit, t) in batch.into_iter().zip(translated_texts) {
        merged.push(TranslateUnit { i: unit.i, t });
    }
    Ok(merged)
}

fn deepl_official_body(texts: &[String], target: &str) -> Value {
    let mut body = serde_json::json!({
        "text": texts,
        "target_lang": target,
        "tag_handling": "xml",
        "tag_handling_version": "v2",
    });
    let tags = deepl_non_splitting_tags(texts);
    if !tags.is_empty() {
        body["non_splitting_tags"] = Value::from(tags);
    }
    body
}

/// 行内占位不能当分句边界，否则「of the <x1>RPM</x1> has」会被拆开翻。
fn deepl_non_splitting_tags(texts: &[String]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for t in texts {
        let bytes = t.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'<' {
                let mut j = i + 1;
                if bytes.get(j) == Some(&b'/') {
                    j += 1;
                }
                let start = j;
                while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                    j += 1;
                }
                if start < j {
                    let name = &t[start..j];
                    if name.starts_with('x') {
                        names.insert(name.to_string());
                    }
                }
                i = j.max(i + 1);
                continue;
            }
            i += 1;
        }
    }
    names.into_iter().collect()
}

fn resolve_deepl_official_url(base_url: Option<&str>, api_key: &str) -> String {
    if let Some(base) = base_url.map(str::trim).filter(|s| !s.is_empty()) {
        let t = base.trim_end_matches('/');
        if t.ends_with("/v2/translate") || t.ends_with("/translate") {
            t.to_string()
        } else {
            format!("{t}/v2/translate")
        }
    } else if api_key.trim().to_ascii_lowercase().ends_with(":fx") {
        "https://api-free.deepl.com/v2/translate".to_string()
    } else {
        "https://api.deepl.com/v2/translate".to_string()
    }
}

fn deepl_official_target_lang(lang: &str) -> String {
    match lang.trim().to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh-hans" | "zh_cn" | "zh_hans" => "ZH-HANS".into(),
        "zh-tw" | "zh-hant" | "zh_tw" | "zh_hant" | "zh-hk" => "ZH-HANT".into(),
        "en" | "en-us" => "EN-US".into(),
        "en-gb" => "EN-GB".into(),
        "pt" | "pt-pt" => "PT-PT".into(),
        "pt-br" => "PT-BR".into(),
        "ja" => "JA".into(),
        "ko" => "KO".into(),
        "de" => "DE".into(),
        "fr" => "FR".into(),
        "es" => "ES".into(),
        "it" => "IT".into(),
        "ru" => "RU".into(),
        other => other.to_ascii_uppercase(),
    }
}

fn parse_deepl_official_translations(value: &Value, expected: usize) -> Result<Vec<String>> {
    let arr = value
        .get("translations")
        .and_then(Value::as_array)
        .context("DeepL 响应缺少 translations 数组")?;
    let texts: Vec<String> = arr
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if texts.len() != expected {
        anyhow::bail!(
            "DeepL 官方返回条数与原文不一致（{} vs {}）",
            texts.len(),
            expected
        );
    }
    Ok(texts)
}

fn parse_deepl_error(raw: &str) -> Option<String> {
    let val: Value = serde_json::from_str(raw).ok()?;
    val.get("message")
        .or_else(|| val.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn http_translate_url(base: &str) -> Result<String> {
    let t = base.trim();
    if t.is_empty() {
        anyhow::bail!("请在设置 → 翻译里填写 HTTP 端点");
    }
    let t = t.trim_end_matches('/');
    if t.rsplit('/').next() == Some("translate") {
        Ok(t.to_string())
    } else {
        Ok(format!("{t}/translate"))
    }
}

fn http_target_lang(lang: &str) -> &str {
    match lang {
        "zh-cn" | "zh-hans" | "zh_cn" | "zh_hans" => "zh",
        other => other,
    }
}

fn deepl_target_lang(lang: &str) -> String {
    match lang.trim().to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh-hans" | "zh_cn" | "zh_hans" => "ZH".into(),
        "zh-tw" | "zh-hant" | "zh_tw" | "zh_hant" => "ZH-HANT".into(),
        "en" | "en-us" => "EN".into(),
        "en-gb" => "EN-GB".into(),
        "ja" => "JA".into(),
        "ko" => "KO".into(),
        other => other.to_ascii_uppercase(),
    }
}

fn parse_deeplx_text(value: &Value) -> Result<String> {
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(200);
    if code != 200 {
        let msg = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("翻译失败");
        anyhow::bail!("DeepLX {code}：{msg}");
    }
    match value.get("data") {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        _ => anyhow::bail!("DeepLX 响应缺少 data"),
    }
}

fn parse_libretranslate_texts(value: &Value, expected: usize) -> Result<Vec<String>> {
    if let Some(err) = value.get("error").and_then(Value::as_str) {
        anyhow::bail!("{err}");
    }
    let raw = value
        .get("translatedText")
        .or_else(|| value.get("translated_text"))
        .context("翻译响应缺少 translatedText")?;
    let texts = match raw {
        Value::String(s) => vec![s.clone()],
        Value::Array(arr) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => anyhow::bail!("translatedText 格式无法识别"),
    };
    if texts.len() != expected {
        anyhow::bail!(
            "HTTP 翻译返回条数与原文不一致（{} vs {}）",
            texts.len(),
            expected
        );
    }
    Ok(texts)
}

fn truncate_err(s: &str) -> String {
    let t = s.trim();
    t.chars().take(240).collect()
}

fn parse_units(parsed: &Value) -> Result<Vec<TranslateUnit>> {
    serde_json::from_value(parsed.get("units").cloned().unwrap_or(Value::Null))
        .map_err(|_| anyhow::anyhow!("翻译服务返回的段落格式无效"))
}

// T1a 仍整篇返回，缺项或重复不能作为完整缓存；T1b 再按单元保留局部成功。
fn validate_units(source: &[TranslateUnit], translated: &[TranslateUnit]) -> Result<()> {
    let expected: BTreeSet<_> = source.iter().map(|unit| unit.i).collect();
    let actual: BTreeSet<_> = translated.iter().map(|unit| unit.i).collect();
    if source.len() != translated.len()
        || actual != expected
        || translated.iter().any(|unit| unit.t.trim().is_empty())
    {
        anyhow::bail!("翻译服务返回的段落缺失、重复或为空，请重试");
    }
    Ok(())
}

fn chunk_units(units: &[TranslateUnit], max_chars: usize) -> Vec<Vec<TranslateUnit>> {
    let mut batches = Vec::new();
    let mut cur = Vec::new();
    let mut n = 0usize;
    for unit in units {
        let len = unit.t.chars().count();
        if !cur.is_empty() && n + len > max_chars {
            batches.push(std::mem::take(&mut cur));
            n = 0;
        }
        n += len;
        cur.push(unit.clone());
    }
    if !cur.is_empty() {
        batches.push(cur);
    }
    batches
}

/// 测通当前后端：翻一句固定英文。设置页用，不写库。
pub async fn test_translate(db_path: &Path, client: &reqwest::Client) -> Result<String> {
    let path = db_path.to_path_buf();
    let config = spawn_db(path, Config::read).await?;
    tracing::info!(backend = config.backend.id(), target_lang = %config.lang, "testing translation backend");
    let units = vec![TranslateUnit {
        i: 0,
        t: "Hello, world.".into(),
    }];
    let out = translate_units(&config, db_path, client, &units, false)
        .await
        .map_err(|err| {
            tracing::warn!(error = %crate::log::error(err.as_ref()), "translation backend test failed");
            err
        })?;
    let translated = out
        .into_iter()
        .next()
        .map(|u| u.t)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("端点没有返回译文"))?;
    tracing::info!("translation backend test succeeded");
    let translated = redact_backend(&translated, &config.backend);
    Ok(translated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_redaction_keeps_typed_error_code_and_parameters() {
        let error = anyhow::Error::new(rrss_core::Error::UnknownAiTask("article_translate".into()));
        let ui = crate::views::UiError::from(redact_error(error, &TranslateBackend::Complete));
        assert_eq!(ui.code(), "ai_task_unknown");
        assert_eq!(
            serde_json::to_value(ui).unwrap()["params"],
            serde_json::json!({"task":"article_translate"})
        );
    }
    use rrss_render::{Block, Document, IR_VERSION, Inline, apply_units, extract_units};

    #[test]
    fn translation_errors_and_debug_never_reveal_the_stored_key() {
        let key = "private-\"custom\\key";
        for backend in [
            TranslateBackend::Http {
                base_url: "https://example.test".into(),
                api_key: Some(key.into()),
            },
            TranslateBackend::DeepLx {
                base_url: "https://example.test".into(),
                api_key: Some(key.into()),
            },
            TranslateBackend::DeepL {
                base_url: Some("https://example.test".into()),
                api_key: Some(key.into()),
            },
        ] {
            assert!(!format!("{backend:?}").contains("private-"));
            let error = redact_backend(
                &format!(
                    "HTTP 403 {key}\n{}\nproxy-authorization: bearer canary\nretry later",
                    serde_json::json!({"detail": key}),
                ),
                &backend,
            );
            assert!(!error.contains("private-") && !error.contains("canary"));
            assert!(error.starts_with("HTTP 403") && error.ends_with("\nretry later"));
        }
        assert!(!redact_backend("bearer canary", &TranslateBackend::Complete).contains("canary"));
    }

    #[test]
    fn backend_ids_are_stable() {
        assert_eq!(TranslateBackend::Complete.id(), ENGINE_COMPLETE);
        assert_eq!(
            TranslateBackend::Http {
                base_url: "http://127.0.0.1:5000".into(),
                api_key: None,
            }
            .id(),
            ENGINE_HTTP
        );
        assert_eq!(
            TranslateBackend::DeepLx {
                base_url: "http://127.0.0.1:1188".into(),
                api_key: None,
            }
            .id(),
            ENGINE_DEEPLX
        );
        assert_eq!(
            TranslateBackend::DeepL {
                base_url: None,
                api_key: Some("test-key:fx".into()),
            }
            .id(),
            ENGINE_DEEPL
        );
        assert_eq!(deepl_target_lang("zh"), "ZH");
        assert_eq!(deepl_target_lang("zh-hant"), "ZH-HANT");
        assert_eq!(deepl_official_target_lang("zh"), "ZH-HANS");
        assert_eq!(deepl_official_target_lang("zh-hans"), "ZH-HANS");
        assert_eq!(deepl_official_target_lang("zh-cn"), "ZH-HANS");
        assert_eq!(deepl_official_target_lang("zh-hant"), "ZH-HANT");
        assert_eq!(deepl_official_target_lang("zh-tw"), "ZH-HANT");
        assert_eq!(deepl_official_target_lang("en"), "EN-US");
        assert_eq!(deepl_official_target_lang("en-us"), "EN-US");
        assert_eq!(deepl_official_target_lang("en-gb"), "EN-GB");
        assert_eq!(deepl_official_target_lang("ja"), "JA");
        assert_eq!(
            resolve_deepl_official_url(None, "abc:fx"),
            "https://api-free.deepl.com/v2/translate"
        );
        assert_eq!(
            resolve_deepl_official_url(None, "abc-pro-key"),
            "https://api.deepl.com/v2/translate"
        );
        assert_eq!(
            resolve_deepl_official_url(Some("https://custom.deepl.proxy"), "abc"),
            "https://custom.deepl.proxy/v2/translate"
        );
        assert_eq!(
            resolve_deepl_official_url(Some("https://custom.deepl.proxy/v2/translate"), "abc"),
            "https://custom.deepl.proxy/v2/translate"
        );
        let parsed_deepl = parse_deepl_official_translations(
            &serde_json::json!({
                "translations": [
                    {"detected_source_language": "EN", "text": "你好，世界。"}
                ]
            }),
            1,
        )
        .unwrap();
        assert_eq!(parsed_deepl, ["你好，世界。"]);
        assert_eq!(
            parse_deepl_error(r#"{"message": "Quota exceeded"}"#).as_deref(),
            Some("Quota exceeded")
        );
        let xml_body = deepl_official_body(
            &[
                "<x0>Version 6.1.0</x0> of the <x1>RPM Package Manager</x1>.".into(),
                "plain".into(),
            ],
            "ZH-HANS",
        );
        assert_eq!(xml_body["tag_handling"], "xml");
        assert_eq!(xml_body["tag_handling_version"], "v2");
        assert_eq!(
            xml_body["non_splitting_tags"],
            serde_json::json!(["x0", "x1"])
        );
        assert!(
            deepl_official_body(&["hello".into()], "DE")
                .get("non_splitting_tags")
                .is_none()
        );
        assert_eq!(
            parse_deeplx_text(&serde_json::json!({
                "code": 200,
                "data": "你好，世界。"
            }))
            .unwrap(),
            "你好，世界。"
        );
        assert!(parse_deeplx_text(&serde_json::json!({"code": 400, "message": "bad"})).is_err());
        assert_eq!(
            http_translate_url("http://127.0.0.1:5000").unwrap(),
            "http://127.0.0.1:5000/translate"
        );
        assert_eq!(
            http_translate_url("http://127.0.0.1:5000/translate/").unwrap(),
            "http://127.0.0.1:5000/translate"
        );
        assert!(http_translate_url("").is_err());
        let parsed =
            parse_libretranslate_texts(&serde_json::json!({"translatedText": ["你好，世界。"]}), 1)
                .unwrap();
        assert_eq!(parsed, ["你好，世界。"]);
    }

    #[test]
    fn engine_roundtrip_does_not_call_backend() {
        let doc = Document {
            v: IR_VERSION,
            blocks: vec![Block::Paragraph {
                inlines: vec![
                    Inline::text("Hello "),
                    Inline::Link {
                        href: "https://ex.test".into(),
                        children: vec![Inline::text("there")],
                    },
                ],
            }],
        };
        let units = extract_units("Title", &doc);
        let fake: Vec<TranslateUnit> = units
            .iter()
            .map(|u| TranslateUnit {
                i: u.i,
                t: u.t
                    .replace("Hello ", "你好 ")
                    .replace("there", "那边")
                    .replace("Title", "标题"),
            })
            .collect();
        let (title, out) = apply_units("Title", &doc, &fake);
        assert_eq!(title, "标题");
        match &out.blocks[0] {
            Block::Paragraph { inlines } => {
                let href = inlines.iter().find_map(|n| match n {
                    Inline::Link { href, .. } => Some(href.as_str()),
                    _ => None,
                });
                assert_eq!(href, Some("https://ex.test"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn complete_backend_chunks_by_itself() {
        let units: Vec<TranslateUnit> = (0..5)
            .map(|i| TranslateUnit {
                i,
                t: "a".repeat(1000),
            })
            .collect();
        let batches = chunk_units(&units, 2500);
        assert!(batches.len() >= 2);
        assert_eq!(batches.iter().map(|b| b.len()).sum::<usize>(), 5);
    }
}
