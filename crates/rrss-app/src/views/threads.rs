//! Conversation lists, folders and persisted message contracts.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{CitationView, ContextUsageView, ModelChoice};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ThreadView {
    pub id: i64,
    pub title: String,
    // 这条对话选定的模型；为空跟随对话默认。`provider_id` 为 0 表示自有 Pi 配置下的 `provider/id`。
    pub chat_model: Option<ModelChoice>,
    #[serde(default)]
    pub chat_thinking: Option<String>,
    #[serde(default)]
    pub context: Option<ContextUsageView>,
    #[serde(default)]
    pub folder_id: Option<i64>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub archived_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ThreadFolderView {
    pub id: i64,
    pub name: String,
    pub position: i64,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ThreadListView {
    pub folders: Vec<ThreadFolderView>,
    pub threads: Vec<ThreadView>,
    pub archived_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ThreadSearchHitView {
    pub thread_id: i64,
    pub title: String,
    pub folder_id: Option<i64>,
    pub message_id: Option<i64>,
    pub role: String,
    pub snippet: String,
    // 命中消息的创建时间；仅标题命中时为会话创建时间。
    pub created_at: i64,
    pub updated_at: i64,
    pub title_match: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DigestThreadView {
    pub thread_id: i64,
    pub chat_model: Option<ModelChoice>,
    #[serde(default)]
    pub chat_thinking: Option<String>,
    #[serde(default)]
    pub context: Option<ContextUsageView>,
    // 最新一页追问记录；更早的用 `messages` 按 `before_id` 向前取。
    pub messages: Vec<ChatMessageView>,
    #[serde(default)]
    pub has_more: bool,
}

/// 一页历史消息（按 id 升序）；`has_more` 表示第一条之前还有更早的消息。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MessagesPage {
    pub messages: Vec<ChatMessageView>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChatMessageView {
    pub id: i64,
    pub role: String,
    pub text: String,
    pub streaming: bool,
    pub cite_article_id: i64,
    pub cite_title: String,
    #[serde(default)]
    pub citations: Vec<CitationView>,
    #[ts(type = "unknown | null")]
    pub document: Option<serde_json::Value>,
}
