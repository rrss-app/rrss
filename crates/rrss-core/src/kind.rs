//! `feeds.kind`：rsshub 是来源，不是线格式。

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedKind {
    Rss,
    Atom,
    Json,
    Rsshub,
    Manual,
}

impl FeedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::Atom => "atom",
            Self::Json => "json",
            Self::Rsshub => "rsshub",
            Self::Manual => "manual",
        }
    }

    pub fn parse_str(value: &str) -> Result<Self> {
        match value {
            "rss" => Ok(Self::Rss),
            "atom" => Ok(Self::Atom),
            "json" => Ok(Self::Json),
            "rsshub" => Ok(Self::Rsshub),
            "manual" => Ok(Self::Manual),
            other => Err(Error::Parse(format!("unknown feed kind {other}"))),
        }
    }

    /// 由 feed-rs 的线格式决定；永不返回 [`FeedKind::Rsshub`]。
    pub fn from_wire(feed_type: &feed_rs::model::FeedType) -> Self {
        match feed_type {
            feed_rs::model::FeedType::Atom => Self::Atom,
            feed_rs::model::FeedType::JSON => Self::Json,
            feed_rs::model::FeedType::RSS0
            | feed_rs::model::FeedType::RSS1
            | feed_rs::model::FeedType::RSS2 => Self::Rss,
        }
    }
}
