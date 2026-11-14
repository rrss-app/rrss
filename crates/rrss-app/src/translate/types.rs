pub use super::window::Failure;
use rrss_render::Block;
use serde::{Deserialize, Serialize};

pub use crate::views::{
    TranslationPhase as Phase, TranslationSaveState as SaveState,
    TranslationStartMode as StartMode, TranslationUnitState as UnitState,
};

impl Phase {
    pub(super) fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockPatch {
    pub index: usize,
    pub ir: Block,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub article_id: i64,
    pub generation: u64,
    pub source_hash: String,
    pub config_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub identity: Identity,
    pub seq: u64,
    pub phase: Phase,
    pub save: SaveState,
    pub units: Vec<UnitState>,
    pub unit_map: Vec<Option<usize>>,
    pub title: Option<String>,
    pub blocks: Vec<BlockPatch>,
    pub error: Option<Failure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventKind {
    Start,
    Delta {
        title: Option<String>,
        blocks: Vec<BlockPatch>,
        units: Vec<(u32, UnitState)>,
    },
    Done,
    Error(Failure),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub identity: Identity,
    pub seq: u64,
    pub phase: Phase,
    pub save: SaveState,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub error: Option<Failure>,
    pub kind: EventKind,
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}
impl<F: Fn(Event) + Send + Sync> EventSink for F {
    fn emit(&self, event: Event) {
        self(event);
    }
}

#[derive(Debug, Clone)]
pub enum StartResult {
    Noop(crate::UiError),
    Cached {
        title: String,
        document: rrss_render::Document,
        units: Vec<UnitState>,
        unit_map: Vec<Option<usize>>,
        source_hash: String,
        config_hash: String,
    },
    Started(Snapshot),
    Resumed(Snapshot),
}

/// 只读恢复没有会话时直接返回缓存，不创建 generation。
#[derive(Debug, Clone)]
pub enum Recovery {
    Missing,
    Session(Snapshot),
    Cached {
        title: String,
        document: rrss_render::Document,
        units: Vec<UnitState>,
        unit_map: Vec<Option<usize>>,
        source_hash: String,
        config_hash: String,
    },
}
