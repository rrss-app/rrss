//! 阅读器协议投影。宿主只转接，不重新判断缓存或调度作业。
use std::{path::Path, sync::Arc};

use super::{cache::Config, runtime::Runtime, types::*};
use crate::views::*;

impl From<Failure> for TranslationCommandError {
    fn from(kind: Failure) -> Self {
        Self {
            kind,
            error: Some(kind.error()),
        }
    }
}

fn identity(value: Identity) -> TranslationIdentity {
    TranslationIdentity {
        article_id: value.article_id,
        generation: Some(value.generation),
        source_hash: value.source_hash,
        config_hash: value.config_hash,
    }
}

fn patches(values: Vec<BlockPatch>) -> Vec<TranslationBlockPatch> {
    values
        .into_iter()
        .map(|value| {
            let mut document = rrss_render::Document {
                v: 1,
                blocks: vec![value.ir],
            };
            crate::embed_fetch::present_document(&mut document);
            TranslationBlockPatch {
                index: value.index,
                ir: serde_json::to_value(&document.blocks[0]).expect("block serialization"),
            }
        })
        .collect()
}

pub fn event(value: Event) -> TranslationEvent {
    let (kind, title, blocks, units, error) = match value.kind {
        EventKind::Start => (
            TranslationEventKind::Start,
            None,
            vec![],
            vec![],
            value.error,
        ),
        EventKind::Delta {
            title,
            blocks,
            units,
        } => (
            TranslationEventKind::Delta,
            title,
            patches(blocks),
            units,
            value.error,
        ),
        EventKind::Done => (
            TranslationEventKind::Done,
            None,
            vec![],
            vec![],
            value.error,
        ),
        EventKind::Error(error) => (
            TranslationEventKind::Error,
            None,
            vec![],
            vec![],
            Some(error),
        ),
    };
    TranslationEvent {
        identity: identity(value.identity),
        seq: value.seq,
        phase: value.phase,
        save: value.save,
        total: value.total,
        done: value.done,
        failed: value.failed,
        kind,
        title,
        blocks,
        units,
        error,
    }
}

fn snapshot(value: Snapshot) -> TranslationSnapshot {
    TranslationSnapshot {
        identity: identity(value.identity),
        seq: value.seq,
        phase: value.phase,
        save: value.save,
        units: value.units,
        unit_map: value.unit_map,
        title: value.title,
        blocks: patches(value.blocks),
        error: value.error,
    }
}

fn cached(
    id: i64,
    title: String,
    document: rrss_render::Document,
    units: Vec<UnitState>,
    unit_map: Vec<Option<usize>>,
    source_hash: String,
    config_hash: String,
) -> TranslationSnapshot {
    let partial = units
        .iter()
        .any(|unit| matches!(unit, UnitState::Failed(_)));
    let mut blocks = Vec::new();
    let mut translated_title = None;
    for (unit, index) in units.iter().zip(&unit_map) {
        if *unit != UnitState::Done {
            continue;
        }
        match index {
            Some(index) => blocks.push(BlockPatch {
                index: *index,
                ir: document.blocks[*index].clone(),
            }),
            None => translated_title = Some(title.clone()),
        }
    }
    TranslationSnapshot {
        identity: TranslationIdentity {
            article_id: id,
            generation: None,
            source_hash,
            config_hash,
        },
        seq: 0,
        phase: if partial {
            Phase::Partial
        } else {
            Phase::Completed
        },
        save: SaveState::Saved,
        units,
        unit_map,
        title: translated_title,
        blocks: patches(blocks),
        error: None,
    }
}

async fn reply(
    path: &Path,
    id: i64,
    mut kind: TranslationReplyKind,
    mut state: Option<TranslationSnapshot>,
    mut notice: Option<UiError>,
) -> Result<TranslationReply, Failure> {
    let retain_cache = state
        .as_ref()
        .is_some_and(|state| state.phase != Phase::Invalidated);
    let (reader, config) = super::spawn_db(path.to_owned(), move |conn| {
        let tx = conn.unchecked_transaction()?;
        let mut reader = crate::queries::reader_view(&tx, id)?;
        let config = Config::optional(&tx)?;
        // 重译失败后，旧的部分成功缓存同样是可阅读结果；普通 ReaderView 仅挂完整缓存。
        if retain_cache && !reader.translation_ready && rrss_core::translations::is_enabled(&tx)? {
            if let Some(config) = &config {
                let article = rrss_core::articles::get_article(&tx, id)?;
                let source = crate::document::load_document(&article);
                if let Some(mut cached) =
                    super::cache::read_progress(&tx, &article, &source, config)?
                {
                    crate::embed_fetch::present_document(&mut cached.document);
                    reader.translated_title = cached.title;
                    reader.translated_document = Some(
                        serde_json::to_value(cached.document).expect("document serialization"),
                    );
                    reader.translation_ready = true;
                }
            }
        }
        tx.commit()?;
        Ok((reader, config.map(|config| config.hash)))
    })
    .await
    .map_err(|_| Failure::Storage)?;
    // 读作业与读正文之间可能发生重抓 / 改设置；绝不把旧覆盖包装成新正文。
    if state.as_ref().is_some_and(|state| {
        state.identity.source_hash != reader.source_hash
            || Some(&state.identity.config_hash) != config.as_ref()
    }) {
        kind = TranslationReplyKind::Missing;
        state = None;
        notice = Some(Failure::Invalidated.error());
    }
    Ok(TranslationReply {
        kind,
        reader,
        snapshot: state,
        notice,
        reconcile_after_ms: super::runtime::WINDOW_TIMEOUT.as_millis() as u64,
    })
}

pub async fn start(
    runtime: &Arc<Runtime>,
    path: &Path,
    id: i64,
    mode: StartMode,
    initial: Option<TranslationInitialViewport>,
) -> Result<TranslationReply, Failure> {
    let (kind, state, note) = match runtime.start(id, mode, initial).await? {
        StartResult::Noop(note) => (TranslationReplyKind::Noop, None, Some(note)),
        StartResult::Started(state) => (TranslationReplyKind::Started, Some(snapshot(state)), None),
        StartResult::Resumed(state) => (TranslationReplyKind::Resumed, Some(snapshot(state)), None),
        StartResult::Cached {
            title,
            document,
            units,
            unit_map,
            source_hash,
            config_hash,
        } => (
            TranslationReplyKind::Cached,
            Some(cached(
                id,
                title,
                document,
                units,
                unit_map,
                source_hash,
                config_hash,
            )),
            None,
        ),
    };
    reply(path, id, kind, state, note).await
}

pub async fn recover(
    runtime: &Arc<Runtime>,
    path: &Path,
    id: i64,
    generation: Option<u64>,
) -> Result<TranslationReply, Failure> {
    let recovery = match generation {
        Some(generation) => runtime
            .snapshot(id, Some(generation))
            .await?
            .map(Recovery::Session)
            .unwrap_or(Recovery::Missing),
        None => runtime.recover(id).await?,
    };
    let (kind, state) = match recovery {
        Recovery::Missing => (TranslationReplyKind::Missing, None),
        Recovery::Session(state) => (TranslationReplyKind::Resumed, Some(snapshot(state))),
        Recovery::Cached {
            title,
            document,
            units,
            unit_map,
            source_hash,
            config_hash,
        } => (
            TranslationReplyKind::Cached,
            Some(cached(
                id,
                title,
                document,
                units,
                unit_map,
                source_hash,
                config_hash,
            )),
        ),
    };
    reply(path, id, kind, state, None).await
}

pub async fn retry_save(
    runtime: &Arc<Runtime>,
    path: &Path,
    id: i64,
    generation: u64,
) -> Result<TranslationReply, Failure> {
    let state = snapshot(runtime.retry_save(id, generation).await?);
    reply(path, id, TranslationReplyKind::Resumed, Some(state), None).await
}
