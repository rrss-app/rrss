//! 模型翻译的单单元窗：超出窗字数时按句拆片；整单元超时、输出不成形或上下文超限时对半细分。
//!
//! 块与单元仍一一对应：各片还原占位编号后拼回整单元，交给 [`validate`] 统一校验嵌回。
//! 每片作为 0 号单元照常读写 `ai_cache`，失败后重试时已译好的片直接命中。
//! 日志只记字数、请求数与错误类别，不记原文或译文。

use super::{Alive, Failure, Output, classify, validate};
use crate::translate::{cache::Config, complete_window, parse_units, remap_units};
use crate::ui_error::reader::ReaderError;
use rrss_render::{
    Document, TaggedSegment, TranslateUnit, TranslationPlan, join_translated_segments,
    restore_segment_tags, split_tagged_unit,
};
use std::{future::Future, path::Path, pin::Pin, time::Duration};

/// 细分下限：不超过它的片不再对半拆。与跨作业窗字数下限一致。
const MIN_SEGMENT_CHARS: usize = rrss_core::translations::WINDOW_CHARS_RANGE.0 as usize;
/// 一个单元最多发出的模型请求（含整单元首试与全部细分），保证必终止。
const MAX_UNIT_REQUESTS: usize = rrss_render::MAX_UNIT_SEGMENTS;

type Cacheable<'c> = &'c (dyn Fn(&str) -> bool + Sync);

pub(super) struct Unit<'a> {
    pub path: &'a Path,
    pub client: &'a reqwest::Client,
    pub config: &'a Config,
    pub plan: &'a TranslationPlan,
    pub source: &'a Document,
    pub id: u32,
    pub article_id: Option<i64>,
    pub timeout: Duration,
    pub fresh: bool,
    pub alive: &'a Alive,
    pub requests: usize,
}

impl<'a> Unit<'a> {
    pub async fn run(mut self, chunk_chars: usize, bypass_cache: bool) -> Result<Output, Failure> {
        let plan: &'a TranslationPlan = self.plan;
        let text = plan.units[self.id as usize].t.as_str();
        let chars = text.chars().count();
        // 窗字数在作业内可以拆到很小，按句拆片仍不低于下限，免得把普通段落切碎。
        let limit = chunk_chars.max(MIN_SEGMENT_CHARS);
        let parts = if chars > limit {
            split_tagged_unit(text, limit)
        } else {
            Vec::new()
        };
        let presplit = parts.len();
        let result = if presplit >= 2 {
            match self.join(parts, limit).await {
                Ok(joined) => Ok(self.validate(joined)),
                Err(failure) => Err(failure),
            }
        } else {
            self.whole(text, chars, bypass_cache).await
        };
        if self.requests > 1 {
            let error = match &result {
                Ok(output) => output
                    .iter()
                    .find_map(|(_, value)| value.as_ref().err().copied()),
                Err(failure) => Some(*failure),
            };
            tracing::info!(
                article_id = self.article_id,
                unit = self.id,
                chars,
                presplit,
                requests = self.requests,
                error = ?error,
                "translation unit segmented"
            );
        }
        result
    }

    /// 整单元先试一次；超时、输出不成形或上下文超限时对半细分。
    async fn whole(
        &mut self,
        text: &str,
        chars: usize,
        bypass_cache: bool,
    ) -> Result<Output, Failure> {
        let (plan, source, id) = (self.plan, self.source, self.id);
        let first = self
            .attempt(text, bypass_cache, &|t| plan.patch(source, id, t).is_ok())
            .await;
        let (failure, fallback) = match first {
            Ok(translated) => {
                let output = self.validate(translated);
                let rejected = output
                    .iter()
                    .find_map(|(_, value)| value.as_ref().err().copied())
                    .filter(|failure| failure.splittable());
                match rejected {
                    Some(failure) => (failure, Ok(output)),
                    None => return Ok(output),
                }
            }
            Err(failure) => (failure, Err(failure)),
        };
        if !failure.splittable() || chars <= MIN_SEGMENT_CHARS {
            return fallback;
        }
        let half = (chars / 2).max(MIN_SEGMENT_CHARS);
        let parts = split_tagged_unit(text, half);
        if parts.len() < 2 {
            return fallback;
        }
        tracing::debug!(
            article_id = self.article_id,
            unit = self.id,
            chars,
            limit = half,
            error = ?failure,
            "translation unit split after failure"
        );
        match self.join(parts, half).await {
            Ok(joined) => Ok(self.validate(joined)),
            Err(failure) => Err(failure),
        }
    }

    fn validate(&self, text: String) -> Output {
        validate(
            self.plan,
            self.source,
            &[self.id],
            vec![TranslateUnit {
                i: self.id,
                t: text,
            }],
            self.article_id,
        )
    }

    /// 逐片翻译（必要时继续细分）后拼回；结果沿用 `parts` 所属文本的占位编号。任一片彻底失败即停止。
    async fn join(&mut self, parts: Vec<TaggedSegment>, limit: usize) -> Result<String, Failure> {
        let mut translated = Vec::with_capacity(parts.len());
        for part in &parts {
            let text = self.segment(part, limit).await?;
            translated.push(restore_segment_tags(&text, &part.tag_map));
        }
        let spaces: Vec<bool> = parts.iter().map(|part| part.trailing_space).collect();
        Ok(join_translated_segments(
            &translated,
            &spaces,
            &self.config.lang,
        ))
    }

    /// 装箱以断开 join → segment → join 的递归 Future 类型。
    fn segment<'s>(
        &'s mut self,
        part: &'s TaggedSegment,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, Failure>> + Send + 's>> {
        Box::pin(async move {
            let chars = part.text.chars().count();
            let fresh = self.fresh;
            // 占位严格匹配才直接放行；占位损坏（如丢了行内代码）优先对半细分重试，
            // 实在拆不动时只接受标签闭合且有可见文字的兜底，绝不让未闭合标签跨片吞并后续段落。
            let (failure, fallback) =
                match self.attempt(&part.text, fresh, &|t| part.accepts(t)).await {
                    Ok(text) if part.accepts(&text) => return Ok(text),
                    Ok(text) => (Failure::InvalidOutput, part.joinable(&text).then_some(text)),
                    Err(failure) => (failure, None),
                };
            if !failure.splittable()
                || chars <= MIN_SEGMENT_CHARS
                || self.requests >= MAX_UNIT_REQUESTS
            {
                return fallback.ok_or(failure);
            }
            let half = (chars / 2).max(MIN_SEGMENT_CHARS).min(limit);
            let parts = split_tagged_unit(&part.text, half);
            if parts.len() < 2 {
                return fallback.ok_or(failure);
            }
            match self.join(parts, half).await {
                Ok(text) => Ok(text),
                Err(err) if err.stops_job() => Err(err),
                Err(err) => fallback.ok_or(err),
            }
        })
    }

    /// 一次模型请求，自带期限。第二次起先核对作业仍有效；请求数到上限即按超限失败。
    async fn attempt(
        &mut self,
        text: &str,
        bypass: bool,
        cacheable: Cacheable<'_>,
    ) -> Result<String, Failure> {
        if self.requests >= MAX_UNIT_REQUESTS {
            return Err(Failure::InvalidOutput);
        }
        if self.requests > 0 {
            (self.alive)().await?;
        }
        self.requests += 1;
        let unit = [TranslateUnit {
            i: 0,
            t: text.to_string(),
        }];
        let request = complete_window(
            self.path,
            self.client,
            self.config,
            &unit,
            bypass,
            |value| {
                let (mut units, stray) = remap_units(&unit, parse_units(value)?);
                anyhow::ensure!(
                    units.len() == 1 && !units[0].t.trim().is_empty(),
                    ReaderError::TranslateUnitsMissing.into_ui()
                );
                let text = units.pop().map(|unit| unit.t).unwrap_or_default();
                let keep = !stray && cacheable(&text);
                Ok((text, keep))
            },
        );
        match tokio::time::timeout(self.timeout, request).await {
            Ok(result) => result.map_err(|error| classify(&error)),
            Err(_) => Err(Failure::Timeout),
        }
    }
}
