//! Reading activity contracts: exposures, article events and read sessions.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

macro_rules! signal_enum {
    ($name:ident => $core:ident { $($variant:ident => $rename:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
        pub enum $name {
            $(#[serde(rename = $rename)] $variant,)+
        }

        impl From<$name> for rrss_core::signals::$core {
            fn from(value: $name) -> Self {
                match value {
                    $($name::$variant => Self::$variant,)+
                }
            }
        }
    };
}

signal_enum!(SignalSurface => Surface {
    Focus => "focus",
    Drawer => "drawer",
    Search => "search",
    Digest => "digest",
    Feeds => "feeds",
    FocusColumn => "focus_column",
});

signal_enum!(SignalEventKind => EventKind {
    Open => "open",
    Star => "star",
    Unstar => "unstar",
    Up => "up",
    Down => "down",
    Cite => "cite",
    Ask => "ask",
    OpenOriginal => "open_original",
});

signal_enum!(ReadSessionEndedBy => EndedBy {
    Back => "back",
    Next => "next",
    Close => "close",
    FocusTimeout => "focus_timeout",
});

signal_enum!(ReadSessionVia => Via {
    Focus => "focus",
    Drawer => "drawer",
    Search => "search",
    Digest => "digest",
    Citation => "citation",
    Feeds => "feeds",
    FocusColumn => "focus_column",
    Related => "related",
});

// 按输入顺序返回曝光 ID；文章已删除时对应位置为 null。
pub type ExposureIds = Vec<Option<i64>>;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ExposureInput {
    pub article_id: i64,
    pub surface: SignalSurface,
    pub position: i64,
    // 产生该位置的排法，交错对比前固定为 "baseline"；分数由后端按写入时的文章表补上。
    pub variant: String,
    pub session_id: String,
    // Unix 毫秒。
    pub shown_at: i64,
}

impl From<ExposureInput> for rrss_core::signals::Exposure {
    fn from(value: ExposureInput) -> Self {
        Self {
            article_id: value.article_id,
            surface: value.surface.into(),
            position: value.position,
            variant: value.variant,
            session_id: value.session_id,
            shown_at: value.shown_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ArticleEventInput {
    pub article_id: i64,
    pub kind: SignalEventKind,
    // 未关联曝光时为 null。
    pub exposure_id: Option<i64>,
    // Unix 毫秒。
    pub at: i64,
}

impl From<ArticleEventInput> for rrss_core::signals::ArticleEvent {
    fn from(value: ArticleEventInput) -> Self {
        Self {
            article_id: value.article_id,
            kind: value.kind.into(),
            exposure_id: value.exposure_id,
            at: value.at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReadSessionInput {
    pub article_id: i64,
    // 未关联曝光时为 null。
    pub exposure_id: Option<i64>,
    // Unix 毫秒。
    pub opened_at: i64,
    pub active_ms: i64,
    pub idle_ms: i64,
    pub read_chars: i64,
    pub skimmed_chars: i64,
    pub total_chars: i64,
    // 到达的最深块序号。
    pub max_block: i64,
    pub reached_end: bool,
    pub translated: bool,
    pub ended_by: ReadSessionEndedBy,
    pub via: ReadSessionVia,
}

impl From<ReadSessionInput> for rrss_core::signals::ReadSession {
    fn from(value: ReadSessionInput) -> Self {
        Self {
            article_id: value.article_id,
            exposure_id: value.exposure_id,
            opened_at: value.opened_at,
            active_ms: value.active_ms,
            idle_ms: value.idle_ms,
            read_chars: value.read_chars,
            skimmed_chars: value.skimmed_chars,
            total_chars: value.total_chars,
            max_block: value.max_block,
            reached_end: value.reached_end,
            translated: value.translated,
            ended_by: value.ended_by.into(),
            via: value.via.into(),
        }
    }
}
