//! 一个网络窗的执行与输出验证，不持有队列，不重新切窗。
use super::{cache::Config, *};
use rrss_render::{Document, PatchError, TranslateUnit, TranslationPatch, TranslationPlan};
use std::{future::Future, path::Path, pin::Pin, time::Duration};

mod segment;

pub use crate::views::TranslationFailure as Failure;

impl Failure {
    pub fn error(self) -> crate::UiError {
        use crate::ui_error::reader::ReaderError;
        match self {
            Self::Timeout => ReaderError::TranslateTimeout,
            Self::Network => ReaderError::TranslateNetwork,
            Self::InvalidOutput => ReaderError::TranslateInvalidOutput,
            Self::Authentication => ReaderError::TranslateAuthentication,
            Self::Quota => ReaderError::TranslateQuota,
            Self::RateLimited => ReaderError::TranslateRateLimited,
            Self::Configuration => ReaderError::TranslateConfiguration,
            Self::Invalidated => ReaderError::TranslateInvalidated,
            Self::Storage => ReaderError::TranslateStorage,
            Self::RetryUnavailable => ReaderError::TranslateRetryUnavailable,
            Self::Capacity => ReaderError::TranslateCapacity,
        }
        .into()
    }
    /// 缩小输入可能就能成功的窗级失败：超时、输出不成形（含上下文超限）。
    pub(super) fn splittable(self) -> bool {
        matches!(self, Self::Timeout | Self::InvalidOutput)
    }
    pub(super) fn stops_job(self) -> bool {
        matches!(
            self,
            Self::Authentication
                | Self::Quota
                | Self::RateLimited
                | Self::Configuration
                | Self::Invalidated
                | Self::Storage
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("translation service HTTP {0}")]
pub(super) struct HttpStatus(pub u16);

fn status_failure(status: u16) -> Failure {
    match status {
        401 | 403 => Failure::Authentication,
        402 | 456 => Failure::Quota,
        429 => Failure::RateLimited,
        400 | 404 | 405 | 407 => Failure::Configuration,
        _ => Failure::Network,
    }
}

pub(super) fn classify(err: &anyhow::Error) -> Failure {
    for cause in err.chain() {
        if let Some(ui) = cause.downcast_ref::<crate::UiError>() {
            return classify_ui(ui);
        }
        if let Some(error) = cause.downcast_ref::<rrss_core::Error>() {
            match error {
                rrss_core::Error::Translation(
                    rrss_core::error::TranslationFailure::InputChanged,
                )
                | rrss_core::Error::ArticleNotFound => return Failure::Invalidated,
                rrss_core::Error::ProviderNotFound
                | rrss_core::Error::TaskModelUnset(_)
                | rrss_core::Error::TemplateNotFound(_)
                | rrss_core::Error::InvalidNetworkProxy(_)
                | rrss_core::Error::InvalidSetting(_)
                | rrss_core::Error::Translation(_) => return Failure::Configuration,
                _ => {}
            }
        }
        if let Some(status) = cause.downcast_ref::<HttpStatus>() {
            return status_failure(status.0);
        }
        if let Some(error) = cause.downcast_ref::<rrss_ai::complete::Error>() {
            match error {
                rrss_ai::complete::Error::Admission(_) => return Failure::Configuration,
                rrss_ai::complete::Error::Capacity => return Failure::Capacity,
                rrss_ai::complete::Error::ServiceCooldown => return Failure::RateLimited,
                // 输入超出模型上下文：与输出不成形一样可拆窗 / 细分，不当配置错误停掉整篇。
                error @ rrss_ai::complete::Error::Http { .. } if error.is_context_too_long() => {
                    return Failure::InvalidOutput;
                }
                rrss_ai::complete::Error::Http { status, .. } => return status_failure(*status),
                rrss_ai::complete::Error::Request { kind } => {
                    return if *kind == rrss_ai::complete::RequestKind::Timeout {
                        Failure::Timeout
                    } else {
                        Failure::Network
                    };
                }
                _ => {}
            }
        }
        if let Some(err) = cause.downcast_ref::<reqwest::Error>() {
            return if err.is_timeout() {
                Failure::Timeout
            } else {
                Failure::Network
            };
        }
        if cause.downcast_ref::<rusqlite::Error>().is_some() {
            return Failure::Storage;
        }
    }
    Failure::InvalidOutput
}

fn classify_ui(error: &crate::UiError) -> Failure {
    if let Some(cause) = &error.cause {
        return classify_ui(cause);
    }
    use crate::ui_error::{jobs::JobsUiError, reader::ReaderError};
    match &error.kind {
        crate::UiErrorKind::Reader(
            ReaderError::TranslateServiceHttp { status }
            | ReaderError::TranslateHttpRejected { status, .. }
            | ReaderError::TranslateDeepLRejected { status, .. }
            | ReaderError::TranslateDeepLxRejected { status, .. },
        ) => status_failure(*status),
        crate::UiErrorKind::Reader(
            ReaderError::TranslateKeyMissing
            | ReaderError::TranslateEndpointMissing
            | ReaderError::TranslateUrlInvalid
            | ReaderError::TranslateSchemeUnsupported,
        ) => Failure::Configuration,
        crate::UiErrorKind::Jobs(JobsUiError::RequestTimeout) => Failure::Timeout,
        crate::UiErrorKind::Jobs(
            JobsUiError::RequestConnect | JobsUiError::RequestDecode | JobsUiError::RequestFailed,
        ) => Failure::Network,
        crate::UiErrorKind::Jobs(
            JobsUiError::ModelHttp { status } | JobsUiError::ModelProxyAuth { status },
        ) => status_failure(*status),
        crate::UiErrorKind::Jobs(JobsUiError::EndpointBusy) => Failure::Capacity,
        crate::UiErrorKind::Jobs(JobsUiError::ModelCooldown) => Failure::RateLimited,
        crate::UiErrorKind::Jobs(
            JobsUiError::LimitRecordingInterrupted
            | JobsUiError::CooldownReadFailed
            | JobsUiError::CooldownSaveFailed
            | JobsUiError::ModelRuntimeStopped
            | JobsUiError::BudgetCheckFailed
            | JobsUiError::SpaceCheckFailed
            | JobsUiError::SpaceChanged
            | JobsUiError::EndpointInvalid,
        ) => Failure::Configuration,
        crate::UiErrorKind::Network(_) => Failure::Configuration,
        _ => Failure::InvalidOutput,
    }
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    #[test]
    fn window_summary_counts_rejected_units_without_logging_their_content() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().into(),
            },
            "",
        );
        tracing::subscriber::with_default(subscriber, || {
            let output = vec![
                (0, Ok(TranslationPatch::Title("canary-translation".into()))),
                (1, Err(Failure::InvalidOutput)),
                (2, Err(Failure::Timeout)),
            ];
            log_finished(Some(1), Some(2), 3, std::time::Duration::ZERO, &Ok(output));
            log_finished(
                Some(1),
                Some(2),
                2,
                std::time::Duration::ZERO,
                &Err(Failure::Timeout),
            );
        });
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
        assert_eq!(
            log.matches("translation window finished").count(),
            2,
            "{log}"
        );
        assert!(
            log.contains("succeeded=1 rejected=1 failed=1")
                && log.contains("succeeded=0 rejected=0 failed=2"),
            "{log}"
        );
        assert!(!log.contains("canary-translation"));
    }

    #[test]
    fn sanitized_ai_errors_keep_the_translation_failure_kind() {
        for timeout in [false, true] {
            let error = anyhow::Error::new(
                crate::ui_error::reader::ReaderError::TranslateRead.caused_by(
                    crate::UiError::from(anyhow::Error::new(rrss_ai::Error::Request {
                        kind: if timeout {
                            rrss_ai::complete::RequestKind::Timeout
                        } else {
                            rrss_ai::complete::RequestKind::Other
                        },
                    })),
                ),
            )
            .context("outer");
            assert_eq!(
                classify(&error),
                if timeout {
                    Failure::Timeout
                } else {
                    Failure::Network
                }
            );
        }
        let error = anyhow::Error::new(rrss_core::Error::InvalidNetworkProxy(
            rrss_core::network::ProxyError::ConfigUnavailable,
        ));
        assert_eq!(classify(&error), Failure::Configuration);
        let error = anyhow::Error::new(rrss_ai::Error::ServiceCooldown).context("outer");
        assert_eq!(classify(&error), Failure::RateLimited);
        let error = anyhow::Error::new(rrss_ai::Error::Http {
            status: 407,
            message: "safe".into(),
        });
        assert_eq!(classify(&error), Failure::Configuration);
        let overflow = anyhow::Error::new(rrss_ai::Error::Http {
            status: 400,
            message: "the request exceeds the available context size (exceed_context_size_error)"
                .into(),
        });
        assert_eq!(classify(&overflow), Failure::InvalidOutput);
        assert!(classify(&overflow).splittable());
        let bad = anyhow::Error::new(rrss_ai::Error::Http {
            status: 400,
            message: "invalid model".into(),
        });
        assert_eq!(classify(&bad), Failure::Configuration);
    }
}

pub(super) type Output = Vec<(u32, Result<TranslationPatch, Failure>)>;

pub(super) fn log_finished(
    article_id: Option<i64>,
    generation: Option<u64>,
    count: usize,
    elapsed: std::time::Duration,
    result: &Result<Output, Failure>,
) {
    let (succeeded, rejected, failed) = match result {
        Ok(output) => {
            let succeeded = output.iter().filter(|(_, result)| result.is_ok()).count();
            let rejected = output
                .iter()
                .filter(|(_, result)| matches!(result, Err(Failure::InvalidOutput)))
                .count();
            (
                succeeded,
                rejected,
                output.len().saturating_sub(succeeded + rejected),
            )
        }
        Err(Failure::InvalidOutput) => (0, count, 0),
        Err(_) => (0, 0, count),
    };
    tracing::info!(article_id, generation, count, succeeded, rejected, failed,
        elapsed_ms = elapsed.as_millis() as u64, error = ?result.as_ref().err(), "translation window finished");
}

/// 模型在多单元 JSON 数组里漏写闭合引号时，会把 `}, {` 吞进当前译文末尾（常伴随后续单元截断）。
/// 仅在多单元窗且原文不含该片段时判定为损坏，不误伤正文里的代码或 JSON 示例。
fn has_json_boundary_leak(source: &str, translated: &str) -> bool {
    let trimmed = translated.trim_end();
    let Some(before_brace) = trimmed.strip_suffix('{') else {
        return false;
    };
    let leaked = before_brace
        .trim_end()
        .strip_suffix(',')
        .is_some_and(|before_comma| before_comma.trim_end().ends_with('}'));
    leaked && !source.contains("},{") && !source.contains("}, {")
}

pub(super) fn validate(
    plan: &TranslationPlan,
    source: &Document,
    ids: &[u32],
    raw: Vec<TranslateUnit>,
    article_id: Option<i64>,
) -> Output {
    // 未请求的编号直接丢弃，不连累本窗其它可验证单元；重复编号则两份都不采信。
    let expected: BTreeSet<_> = ids.iter().copied().collect();
    let mut values = std::collections::HashMap::<u32, (String, bool)>::new();
    for unit in raw.into_iter().filter(|unit| expected.contains(&unit.i)) {
        values
            .entry(unit.i)
            .and_modify(|value| value.1 = true)
            .or_insert((unit.t, false));
    }
    let multi = ids.len() > 1;
    ids.iter()
        .map(|id| {
            let source_text = plan
                .units
                .get(*id as usize)
                .map(|unit| unit.t.as_str())
                .unwrap_or("");
            let (patch, output) = match values.remove(id) {
                None => (Err(PatchError::MissingUnit), String::new()),
                Some((text, true)) => (Err(PatchError::DuplicateUnit), text),
                Some((text, false)) if multi && has_json_boundary_leak(source_text, &text) => {
                    (Err(PatchError::Remainder), text)
                }
                Some((text, false)) => (plan.patch(source, *id, &text), text),
            };
            if let Err(error) = &patch {
                log_rejected(article_id, *id, error.reason(), source_text, &output);
            }
            (*id, patch.map_err(|_| Failure::InvalidOutput))
        })
        .collect()
}

fn log_rejected(article_id: Option<i64>, unit: u32, reason: &str, source: &str, output: &str) {
    let source_len = source.chars().count();
    let output_len = output.chars().count();
    tracing::debug!(
        article_id,
        unit,
        error_kind = reason,
        source_len,
        output_len,
        "translation unit rejected"
    );
}

/// 拆片之间核对作业仍有效（未失效、未停止）；失败时返回应记给该单元的错误。
pub(super) type Alive =
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<(), Failure>> + Send>> + Send + Sync;

pub(super) struct Request<'a> {
    pub path: &'a Path,
    pub config: &'a Config,
    pub plan: &'a TranslationPlan,
    pub source: &'a Document,
    pub ids: &'a [u32],
    pub bypass_cache: bool,
    pub article_id: Option<i64>,
    /// 本窗的有效窗字数；模型翻译的单单元超出时按句拆片。
    pub chunk_chars: usize,
    /// 单次模型请求的期限。模型单单元窗由后端逐次计时，其余窗由调用方整体计时。
    pub timeout: Duration,
    /// 显式重新生成：拆片也绕过片缓存。其余重试复用已译好的片。
    pub fresh: bool,
    pub alive: &'a Alive,
}

pub(super) trait Backend: Send + Sync {
    fn execute<'a>(
        &'a self,
        request: Request<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<Output, Failure>> + Send + 'a>>;
}

/// 工厂只持有库路径与连接池；每个执行窗领取最新 Api 配置。
/// 同窗内的分支、格式重试与响应读取共用一个客户端快照。
pub(super) struct HttpBackend(pub crate::network::NetworkClients);

impl Backend for HttpBackend {
    fn execute<'a>(
        &'a self,
        request: Request<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<Output, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let Request {
                path,
                config,
                plan,
                source,
                ids,
                bypass_cache,
                article_id,
                chunk_chars,
                timeout,
                fresh,
                alive,
            } = request;
            let units: Vec<_> = ids
                .iter()
                .map(|id| plan.units[*id as usize].clone())
                .collect();
            if matches!(&config.backend, TranslateBackend::DeepL { api_key, .. } if api_key.as_deref().is_none_or(|s| s.trim().is_empty()))
            {
                return Err(Failure::Configuration);
            }
            // 执行槽与组窗之后才读取网络设置；不把网络路由加入译文配置身份。
            // 领取失败即停止，绝不回退为直连或继续用上一次的客户端。
            let client = self
                .0
                .client(rrss_core::network::NetworkScope::Api)
                .await
                .map_err(|err| classify(&anyhow::Error::new(err)))?;
            if config.backend == TranslateBackend::Complete && ids.len() == 1 {
                return segment::Unit {
                    path,
                    client: &client,
                    config,
                    plan,
                    source,
                    id: ids[0],
                    article_id,
                    timeout,
                    fresh,
                    alive,
                    requests: 0,
                }
                .run(chunk_chars, bypass_cache)
                .await;
            }
            if config.backend == TranslateBackend::Complete {
                return complete_window(path, &client, config, &units, bypass_cache, |value| {
                    let (raw, stray) = remap_units(&units, parse_units(value)?);
                    let out = validate(plan, source, ids, raw, article_id);
                    let cacheable = !stray && out.iter().all(|(_, value)| value.is_ok());
                    Ok((out, cacheable))
                })
                .await
                .map_err(|err| classify(&err));
            }
            let raw = non_complete_window(path, &client, config, &units)
                .await
                .map_err(|err| classify(&err))?;
            Ok(validate(plan, source, ids, raw, article_id))
        })
    }
}
