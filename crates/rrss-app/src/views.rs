//! 前端契约。字段用 i64 / String，不含 Slint 原生类型。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClientLogLevel {
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ArticleView {
    pub id: i64,
    pub title: String,
    pub feed_id: i64,
    pub feed_title: String,
    pub feed_icon_href: String,
    // 发布时间（Unix 毫秒）；本地时区与相对说法由前端按当前时间格式化，保活的列表不会停在旧的「N 分钟前」。
    pub published_at: Option<i64>,
    pub url: String,
    pub read: bool,
    #[serde(default)]
    pub starred: bool,
    pub score: i64,
    pub summary: String,
    pub row_kind: i32,
    pub cluster_id: i64,
    pub cluster_size: i64,
    pub cluster_label: String,
    pub cluster_span: String,
    pub interest_label: String,
    #[serde(default)]
    #[ts(optional)]
    pub rank_reasons: Option<Vec<RankReasonView>>,
    pub noisy: bool,
    #[serde(default)]
    pub spot_check: bool,
    #[serde(default)]
    #[ts(optional)]
    pub hide_reasons: Option<Vec<ReasonView>>,
    // 只在相近报道标题行上：这组可见成员的来源，按成员先后去重。组头收起时也能写来源数、叠放头像。
    #[serde(default)]
    #[ts(optional)]
    pub cluster_sources: Option<Vec<ClusterSourceView>>,
    // 只在相近报道标题行上：这组存过综述、之后又新来了几篇（组头的「新 N」）。
    #[serde(default)]
    #[ts(optional)]
    pub cluster_fresh: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClusterSourceView {
    pub feed_id: i64,
    pub title: String,
    pub icon_href: String,
}

/// 相近报道综述（AI 规格 · 相近报道综述）。由 `cluster_summary` 读取、`generate_cluster_summary` 生成后返回。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClusterSummaryView {
    pub cluster_id: i64,
    // none：还没生成；ready：与当前成员一致；stale：成员或各篇摘要变了，`fresh` 列出新来的篇。
    pub state: String,
    // 「相近报道综述」用途有可用的模型；没有时界面列各篇已有的一句话，不调用模型。
    pub model_ready: bool,
    // 按当前输入估算的一次用量（token，取整到 500）。
    pub estimate_tokens: i64,
    pub overview: String,
    pub additions: Vec<ClusterNoteView>,
    pub conflicts: Vec<ClusterNoteView>,
    // 生成时依据的篇数与时刻（Unix 毫秒）；没有综述时为 0 与 null。
    pub basis: i64,
    pub generated_at: Option<i64>,
    pub fresh: Vec<ClusterArticleView>,
    // 当前可见成员，按发布时间正序。
    pub members: Vec<ClusterArticleView>,
}

/// 「各家补充」一条（一个来源）或「说法不一」一条（一到多个来源）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClusterNoteView {
    pub text: String,
    pub sources: Vec<ClusterArticleView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClusterArticleView {
    pub article_id: i64,
    pub feed_id: i64,
    pub feed_title: String,
    pub icon_href: String,
    pub title: String,
    pub published_at: Option<i64>,
    // 这篇已有的一句话；没有摘要时为空。
    pub one_liner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RelatedArticleView {
    pub article_id: i64,
    pub feed_id: i64,
    pub feed_title: String,
    pub icon_href: String,
    pub title: String,
    pub published_at: Option<i64>,
    pub one_liner: String,
    pub read: bool,
    pub similarity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RankReasonView {
    pub text: String,
    // 这条原因折合的整数分，正为加分。
    pub points: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeedView {
    pub id: i64,
    pub title: String,
    pub icon_href: String,
    pub url: String,
    pub error: String,
    pub noisy: bool,
    pub folder_id: i64,
    // 保留天数：null 跟随全局，0 永久保留。
    pub retention_days: Option<i64>,
    // 源健康：failing / stale / noisy，空串表示没有提示（或已忽略）。
    pub health: String,
    pub health_label: String,
    // 展开后显示的详情，多行。
    pub health_detail: String,
    // 与同级文件夹共用的顺序。
    pub position: i64,
    // 手动收录源还没有任何文章；订阅树据此隐藏它。其他源恒为 false。
    pub empty: bool,
}

/// 订阅行内编辑的一次保存；`None` 字段表示未改。`folder_id` 为 `Some(0)` 表示移出文件夹。
/// 三项写入同一事务，成功返回最新 `FeedTree`。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SaveFeedArgs {
    pub id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub folder_id: Option<i64>,
    // 保留："follow" 跟随全局，"keep" 永久保留，其余为天数。
    #[serde(default)]
    pub retention: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AddFeedResult {
    pub id: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeedUpdated {
    pub run_id: String,
    pub generation: Option<u64>,
    pub status: String,
    pub derived_degraded: bool,
    // None invalidates shared source / derived data (icons or background jobs).
    pub feed_id: Option<i64>,
    pub inserted: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FolderView {
    pub id: i64,
    pub name: String,
    pub label: String,
    pub indent: i64,
    pub position: i64,
    pub feeds: Vec<FeedView>,
}

/// 一次树移动前，某个被重编号父级里的一项。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TreeLayout {
    pub item: String,
    pub parent_folder_id: Option<i64>,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeedTree {
    pub folders: Vec<FolderView>,
    pub uncategorized: Vec<FeedView>,
    pub folder_combo: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReasonView {
    pub rule_id: i64,
    pub dimension: String,
    pub snippet: String,
    pub weight: i64,
    pub label: String,
}

impl From<rrss_core::score::Reason> for ReasonView {
    fn from(reason: rrss_core::score::Reason) -> Self {
        Self {
            rule_id: reason.rule_id,
            dimension: reason.dimension.as_str().into(),
            label: format!("{}：{}", reason.dimension.label_zh(), reason.snippet),
            snippet: reason.snippet,
            weight: reason.weight,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct HiddenArticleView {
    pub spot_liked: bool,
    pub id: i64,
    pub title: String,
    pub feed_id: i64,
    pub feed_title: String,
    pub published_at: Option<i64>,
    pub score: i64,
    pub reasons: Vec<ReasonView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct HiddenRuleCount {
    pub rule_id: i64,
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct HiddenPage {
    pub spot_liked_count: i64,
    pub articles: Vec<HiddenArticleView>,
    pub rule_counts: Vec<HiddenRuleCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuleStatsView {
    pub rule_id: i64,
    pub hits_30d: i64,
    pub last_hit_at: Option<i64>,
    pub hidden_count: i64,
    pub advice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SpotCheckStatsView {
    pub exposed: i64,
    pub liked: i64,
    pub disliked: i64,
    pub insufficient: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuleStatsPage {
    pub rules: Vec<RuleStatsView>,
    pub spot_check: SpotCheckStatsView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeatureView {
    pub dimension: String,
    pub pattern: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuleView {
    pub id: i64,
    pub polarity: String,
    pub dimension: String,
    pub dimension_id: String,
    pub pattern: String,
    pub summary: String,
    pub scope_label: String,
    pub scope_kind: String,
    pub scope_id: i64,
    pub weight: i64,
    pub source: String,
    pub enabled: bool,
    pub pending: bool,
    pub pack_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PresetView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub rule_count: i64,
    pub enabled_count: i64,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScopeOption {
    pub id: i64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct InterestView {
    pub id: i64,
    pub label: String,
    pub weight: i64,
    pub enabled: bool,
    pub has_embedding: bool,
    pub hit_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct InterestHitView {
    pub id: i64,
    pub title: String,
    pub feed_title: String,
    pub weight: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct CitationView {
    pub kind: String,
    pub article_id: i64,
    pub title: String,
    pub feed_title: String,
    pub url: String,
    pub excerpt: String,
    pub block_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DigestSelection {
    pub block_id: i64,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MediaRef {
    pub src: String,
    pub href: String,
    // 本地缓存图片的像素宽高（按显示方向），阅读器据此在解码前占位；视频与读不出时为空。
    pub width: Option<u32>,
    pub height: Option<u32>,
    // 最近一次缓存失败的说明；没有失败时为空。只活在本进程内存里，不写入 media 表。
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MediaReadyEvent {
    pub article_id: i64,
    pub media: MediaRef,
}

/// 官方视频封面。`duration_secs` 只在哔哩哔哩有值；YouTube oEmbed 没有时长。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EmbedCoverView {
    pub url: String,
    pub open_url: String,
    pub site: String,
    pub title: String,
    pub author: String,
    pub duration_secs: Option<u32>,
    pub cover_href: String,
    pub failed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct EmbedCoverEvent {
    pub article_id: i64,
    pub cover: EmbedCoverView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReadingSpeedsView {
    // 每毫秒字数；没有足够的个人样本时为 null。
    pub chinese_original: Option<f64>,
    pub chinese_translated: Option<f64>,
    pub english_original: Option<f64>,
    pub english_translated: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ReaderView {
    pub id: i64,
    // 实际标题与正文的版本；不使用馈源去重 hash。
    pub source_hash: String,
    pub title: String,
    pub meta: String,
    /// 本地缓存的网站图标（`rrss://`）；没有或已关闭时为空。
    pub feed_icon_href: String,
    pub url: String,
    pub summary: String,
    pub score: i64,
    pub feedback: i32,
    #[ts(type = "unknown")]
    pub document: serde_json::Value,
    pub reasons: Vec<ReasonView>,
    pub media: Vec<MediaRef>,
    #[serde(default)]
    pub embed_covers: Vec<EmbedCoverView>,
    #[serde(default)]
    pub translated_title: String,
    #[serde(default)]
    #[ts(type = "unknown | null")]
    pub translated_document: Option<serde_json::Value>,
    #[serde(default)]
    pub translation_ready: bool,
    #[serde(default)]
    pub translation_note: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub starred: bool,
    // 正文已按保留策略清理：阅读器显示提示和重新抓取入口。
    #[serde(default)]
    pub pruned: bool,
    // 摘要来自旧正文（例如抓回全文之前生成），建议重新生成。旧摘要无法判断时为 false。
    #[serde(default)]
    pub summary_stale: bool,
    // 有值即表示当前正文来自原网页窗口；同时保留最近抽取时间。
    #[serde(default)]
    #[ts(optional = nullable)]
    pub web_extracted_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranslationFailure {
    Timeout,
    Network,
    InvalidOutput,
    Authentication,
    Quota,
    RateLimited,
    Configuration,
    Invalidated,
    Storage,
    Capacity,
    RetryUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranslationUnitState {
    Pending,
    Inflight,
    Done,
    Failed(TranslationFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranslationPhase {
    Queued,
    Running,
    Completed,
    Partial,
    Failed,
    Invalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranslationSaveState {
    Unsaved,
    Saving,
    Saved,
    SaveFailed,
    // 未全部译完且库里已有完整译文：本次结果只留在内存，可继续重试未译部分。
    Kept,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationIdentity {
    pub article_id: i64,
    // 缓存没有运行中的代际，不伪造 generation = 0。
    pub generation: Option<u64>,
    pub source_hash: String,
    pub config_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TranslationViewport {
    pub visible: Vec<usize>,
    pub nearby: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationInitialViewport {
    pub source_hash: String,
    pub ticket: u64,
    pub intent: u64,
    pub viewport: TranslationViewport,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationHint {
    pub identity: TranslationIdentity,
    pub ticket: u64,
    pub intent: u64,
    pub viewport: TranslationViewport,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationBlockPatch {
    pub index: usize,
    #[ts(type = "unknown")]
    pub ir: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationSnapshot {
    pub identity: TranslationIdentity,
    pub seq: u64,
    pub phase: TranslationPhase,
    pub save: TranslationSaveState,
    pub units: Vec<TranslationUnitState>,
    pub unit_map: Vec<Option<usize>>,
    pub title: Option<String>,
    pub blocks: Vec<TranslationBlockPatch>,
    pub error: Option<TranslationFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranslationEventKind {
    Start,
    Delta,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationEvent {
    pub identity: TranslationIdentity,
    pub seq: u64,
    pub phase: TranslationPhase,
    pub save: TranslationSaveState,
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub kind: TranslationEventKind,
    pub title: Option<String>,
    pub blocks: Vec<TranslationBlockPatch>,
    pub units: Vec<(u32, TranslationUnitState)>,
    pub error: Option<TranslationFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranslationStartMode {
    Default,
    RetryFailed,
    Regenerate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranslationReplyKind {
    Noop,
    Missing,
    Cached,
    Started,
    Resumed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationReply {
    pub kind: TranslationReplyKind,
    pub reader: ReaderView,
    pub snapshot: Option<TranslationSnapshot>,
    pub note: String,
    // 无事件时核对状态的周期来自后端窗期限。
    pub reconcile_after_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslationCommandError {
    pub kind: TranslationFailure,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ContextUsageView {
    pub used: i64,
    pub window: Option<i64>,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChatContextEvent {
    pub thread_id: i64,
    pub context: ContextUsageView,
}

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
#[serde(rename_all = "camelCase")]
pub struct SendMessageArgs {
    pub thread_id: i64,
    pub text: String,
    pub citations: Vec<CitationView>,
    #[serde(default)]
    pub digest_id: Option<i64>,
    // 这条对话此后用的模型（对话里的选择）；为空跟随对话默认。随发送记到线程上。
    #[serde(default)]
    pub model: Option<ModelChoice>,
    // None 保持线程设置；空字符串清除，跟随模型。
    #[serde(default)]
    #[ts(optional)]
    pub thinking: Option<String>,
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

/// chat:delta 是指定线程的完整快照，不是可直接累加的增量。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChatStreamEvent {
    pub thread_id: i64,
    pub text: String,
    #[ts(type = "unknown | null")]
    pub document: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChatErrorEvent {
    pub thread_id: i64,
    pub message: String,
    // not_installed / version / config / model / process_exit / busy / internal，文案见阶段 19。
    #[serde(default)]
    pub kind: String,
}

/// chat:status：一行状态（压缩、重试、提示），`text` 为空表示清除；或者一步过程（`step`），只给回复位置的等待过程用。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChatStatusEvent {
    // 冷启动时「启动对话引擎」可能早于线程建立，此时为发送时的线程 id（新对话为 0）。
    pub thread_id: i64,
    pub text: String,
    // 仅「未验证版本」提示带版本号：界面实际展示后用它调用 acknowledge_pi_version_notice。
    #[serde(default)]
    pub notice_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub step: Option<ChatStepView>,
}

/// 等待过程里的一步：启动对话引擎（仅冷启动）或一次工具调用，分开始与结束两条，用 `id` 对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ChatStepView {
    // engine / tool
    pub kind: String,
    pub id: String,
    // true 开始，false 结束。
    pub start: bool,
    // 工具名；engine 为空。
    pub tool: String,
    pub server: String,
    pub server_tool: String,
    // 一行文案，如「查阅本地文章」「启动对话引擎」。
    pub label: String,
    // 参数摘要：检索词加引号、文章标题加书名号、网址取站点与路径；没有为空。
    pub detail: String,
    // 这一步读的是本地文章时的 id。
    pub article_id: Option<i64>,
    // 结束时工具报错。
    pub error: bool,
}

/// chat:confirm：扩展请求用户确认（例如读取网页）。用 `confirm_fetch(id, ok, remember)` 回应；超时由对话引擎按拒绝处理。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChatConfirmEvent {
    pub thread_id: i64,
    pub id: String,
    pub title: String,
    pub message: String,
    pub timeout_ms: Option<i64>,
    pub server: String,
    pub tool: String,
    pub args: String,
    pub can_remember: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MemoryView {
    pub id: i64,
    pub kind: String,
    pub content: String,
    pub pinned: bool,
    pub pending: bool,
    pub scope: String,
    pub strength: String,
}

/// 连接（阶段 19-7 / 19-9）：不再有模型，也不带完整密钥。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderView {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub base_url: String,
    pub protocol: String,
    pub key: KeyState,
    // 已保存密钥的首尾各 4 位（短密钥只给末 4 位或不给）；没有密钥时为空。
    pub key_hint: String,
    #[serde(default)]
    #[ts(optional)]
    pub last_test: Option<ProviderTestView>,
    #[serde(default)]
    #[ts(optional)]
    pub compat_json: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    None,
    Set,
    // 从不含密钥的备份恢复，需要重新填写。
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SecretView {
    pub state: KeyState,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderTestView {
    pub at: i64,
    pub ok: bool,
    pub note: String,
}

/// 用途当前选的模型；`provider_id` 为 0 表示还没选。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TaskBindView {
    pub task: String,
    pub title: String,
    pub provider_id: i64,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DigestSourceView {
    pub article_id: i64,
    pub title: String,
    pub feed_title: String,
    pub number: Option<usize>,
    pub summary: Option<String>,
    pub published_at: Option<i64>,
    // available / removed / unavailable；查询失败不能冒充文章已移除。
    pub status: String,
}

/// 本地文章的悬停预览（对话里的 `rrss://article/<id>` 链接与引用小签）；字段同简报来源，另带已读与原文地址。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ArticlePreviewView {
    pub article_id: i64,
    pub title: String,
    pub feed_title: String,
    pub summary: Option<String>,
    pub published_at: Option<i64>,
    pub read: bool,
    pub url: String,
    // available / removed（库里已没有这篇）。
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DigestLineView {
    pub block_id: i64,
    pub sources: Vec<DigestSourceView>,
}

/// 简报列表只带翻期需要的身份；正文和来源由 `digest` 按当前期单独取。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DigestSummary {
    pub id: i64,
    pub period: String,
    pub period_label: String,
    pub period_start: i64,
    pub date_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DigestView {
    pub id: i64,
    pub period: String,
    pub period_label: String,
    pub period_start: i64,
    pub date_label: String,
    pub body: String,
    pub revision: String,
    // verified / legacy / mismatch / unavailable
    pub source_state: String,
    pub source_note: String,
    pub source_index: Vec<DigestSourceView>,
    pub unlocated_sources: Vec<DigestSourceView>,
    pub items: Vec<DigestLineView>,
    #[ts(type = "unknown | null")]
    pub document: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PiToolView {
    pub name: String,
    pub label: String,
    pub description: String,
    // 随应用提供的描述；与 `description` 不同时界面给出「恢复默认」。
    pub default_description: String,
    pub enabled: bool,
    pub confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SkillView {
    pub name: String,
    pub description: String,
    pub bundled: bool,
    pub enabled: bool,
    pub loaded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McpView {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub env: Vec<McpSecretView>,
    pub headers: Vec<McpSecretView>,
    pub auth: String,
    pub include_tools: Vec<String>,
    pub enabled: bool,
    pub logged_in: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McpSecretView {
    pub name: String,
    pub has_value: bool,
}

#[derive(Clone, Serialize, Deserialize, TS)]
pub struct McpSecretInput {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Serialize, Deserialize, TS)]
pub struct McpInput {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub env: Vec<McpSecretInput>,
    pub headers: Vec<McpSecretInput>,
    pub auth: String,
    pub include_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McpToolView {
    pub name: String,
    pub description: String,
    pub writes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McpProbeView {
    pub ok: bool,
    pub elapsed_ms: i64,
    pub tools: Vec<McpToolView>,
    pub error: String,
    pub hint: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McpLoginResult {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McpImportEntry {
    pub name: String,
    pub transport: String,
    pub secret_count: usize,
    pub skipped: bool,
}

/// 用户技能的完整内容，编辑弹层按需读取；随应用提供的技能不可编辑。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SkillDraft {
    pub name: String,
    pub description: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RankingSettings {
    #[serde(default)]
    pub show_details: bool,
    pub interest_boost: bool,
    pub rerank: bool,
    pub cluster_group: bool,
    #[serde(default)]
    pub hide_noisy: bool,
    #[serde(default)]
    pub interest_ready: bool,
    #[serde(default)]
    pub ranking_note: String,
    pub hide_threshold: i64,
    pub theme: String,
    // B1 / B2 学来的排序分量（阶段 23「共同部分」），默认开启。
    pub rank_features: bool,
    pub rank_source: bool,
    pub rank_coverage: bool,
    pub rank_freshness: bool,
    pub rank_affinity: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FocusPage {
    pub articles: Vec<ArticleView>,
    pub digest_banner: String,
    pub digest_lede: String,
    pub filter: String,
    pub status: String,
    pub ranking: RankingSettings,
    pub has_more: bool,
    #[serde(default = "default_list_scope")]
    pub list_scope: String,
}

fn default_list_scope() -> String {
    "all".into()
}

// 时间线游标。at 是发布时间，没有发布时间时用入库时间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct FeedsCursor {
    pub at: i64,
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeedsPage {
    pub articles: Vec<ArticleView>,
    pub has_more: bool,
    pub next_cursor: Option<FeedsCursor>,
    pub hidden_count: i64,
    // 解析并确认文件夹或源仍在之后的范围；已删除的回成 all。
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ScopeCount {
    pub id: i64,
    pub unread: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeedCountsView {
    pub total: i64,
    pub feeds: Vec<ScopeCount>,
    pub folders: Vec<ScopeCount>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SearchScope {
    #[default]
    All,
    Unread,
    Starred,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct SearchQuery {
    pub query: String,
    pub scope: SearchScope,
    pub feed_ids: Vec<i64>,
    pub published_since: Option<i64>,
    pub limit: i64,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            scope: SearchScope::All,
            feed_ids: vec![],
            published_since: None,
            limit: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SearchHitBlocksView {
    pub source_hash: String,
    pub total: usize,
    pub title_match: bool,
    pub hits: Vec<SearchBlockHitView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SearchBlockHitView {
    pub block_id: usize,
    pub ordinal: usize,
    pub section: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SearchHitView {
    pub id: i64,
    pub title: String,
    pub feed_title: String,
    /// 本地缓存的网站图标（`rrss://`）；没有或已关闭时为空。
    pub feed_icon_href: String,
    // 发布时间（Unix 毫秒），前端格式化。
    pub published_at: Option<i64>,
    pub score: i64,
    pub snippet: String,
    pub read: bool,
    pub starred: bool,
    pub title_match: bool,
    pub body_match: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RefreshOperation {
    pub id: u64,
    pub target: usize,
    pub completed: usize,
    pub inserted: i64,
    pub failed: usize,
    // 前几个失败源的 ID，界面用订阅列表换成名字；总数看 failed。
    pub failed_feeds: Vec<i64>,
    pub deleted: usize,
    pub skipped: usize,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RefreshStarted {
    pub run_id: String,
    pub operation_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SchedulerSnapshot {
    pub run_id: String,
    pub version: u64,
    pub operations: Vec<RefreshOperation>,
    pub background_requested: u64,
    pub background_completed: u64,
    pub stopped: bool,
    pub background: Vec<BackgroundJobView>,
    // 订阅网络暂停或启动诊断；只在现有提示位置展示。
    pub feed_pause: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BackgroundJobView {
    pub kind: String,
    pub generation: u64,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct TimingView {
    pub count: u64,
    pub total_ms: f64,
    pub max_ms: f64,
    // Fixed histogram upper bounds, not exact sampled percentiles.
    pub p50_ms: f64,
    pub p95_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SchedulerTiming {
    pub stage: String,
    pub timing: TimingView,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SchedulerCounter {
    pub name: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeedDiagnostics {
    pub cooldowns: CooldownDiagnostics,
    pub network_paused: bool,
    pub admitted: usize,
    pub admission_limit: usize,
    pub peak_admitted: usize,
    pub queued: usize,
    pub parked: usize,
    // Sum of targets not yet admitted across operations, not distinct sources.
    pub operation_waiting: usize,
    pub oldest_wait_ms: u64,
    pub oldest_eligible_wait_ms: u64,
    pub downloads: usize,
    pub http_active: usize,
    pub http_peak: usize,
    pub http_limit: usize,
    pub host_peak: usize,
    pub host_limit: usize,
    pub parsing: usize,
    pub parsing_peak: usize,
    pub parsing_limit: usize,
    pub buffered_bodies: usize,
    pub buffered_bodies_peak: usize,
    pub buffered_bytes: usize,
    pub buffered_bytes_peak: usize,
    pub buffer_high_water: usize,
    pub buffer_envelope: usize,
    pub result_waiting: usize,
    pub network_waiting: usize,
    pub deferral_waiting: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelDiagnostics {
    // None while a snapshot is being restored or saved; never pretend zero.
    pub cooldowns: Option<CooldownDiagnostics>,
    pub active: usize,
    pub peak_active: usize,
    pub waiting_interactive: usize,
    pub waiting_background: usize,
    pub active_endpoints: usize,
    pub busiest_endpoint: usize,
    pub background_active: usize,
    pub content_active: usize,
    pub content_waiting: usize,
    pub wait: TimingView,
    pub held: TimingView,
    pub http_429: u64,
    pub http_503: u64,
    pub cooldown_rejections: u64,
    pub budget_rejections: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct CooldownDiagnostics {
    pub loaded: bool,
    pub active_services: usize,
    pub global: bool,
    pub recovery: bool,
    pub storage_blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BackgroundDiagnostics {
    pub kind: String,
    pub state: String,
    pub reason: Option<String>,
    // Coalesced batch counts, not a count of all eligible database objects.
    pub pending_batches: usize,
    pub running_ms: u64,
    pub oldest_eligible_wait_ms: u64,
    pub next_attempt_in_ms: Option<u64>,
    pub attempts: u64,
    pub retries: u64,
    pub failures: u64,
    pub pauses: u64,
    pub duration: TimingView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SchedulerDiagnostics {
    pub run_id: String,
    pub elapsed_ms: u64,
    pub stopped: bool,
    pub feeds: FeedDiagnostics,
    pub models: ModelDiagnostics,
    pub background: Vec<BackgroundDiagnostics>,
    pub timings: Vec<SchedulerTiming>,
    pub counters: Vec<SchedulerCounter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RulesPage {
    pub rules: Vec<RuleView>,
    pub presets: Vec<PresetView>,
    pub folders: Vec<ScopeOption>,
    pub feeds: Vec<ScopeOption>,
    pub hide_threshold: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AiPage {
    pub providers: Vec<ProviderView>,
    pub selected_id: i64,
    pub usage: AiUsageView,
    pub auto_summary: AutoSummaryView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AiUsageView {
    pub generation: UsageLine,
    pub embedding: UsageLine,
    pub chat: UsageLine,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct UsageLine {
    pub today: i64,
    pub month: i64,
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AutoSummaryView {
    pub today: i64,
    pub daily_limit: i64,
    pub window_days: i64,
    pub batch_size: i64,
    pub batch_minutes: i64,
    pub model_ready: bool,
    pub per_article_tokens: i64,
    pub digest_daily_tokens: i64,
    pub notice: Option<AutoSummaryNotice>,
    pub cluster_today: i64,
    pub cluster_daily_limit: i64,
    pub per_cluster_tokens: i64,
    /// 「相近报道综述」用途已绑定可用模型；自动汇总没有它时不运行。
    pub cluster_model_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AutoSummaryNotice {
    pub budget_defaulted: bool,
    pub generation_budget: i64,
}

/// 只保存一个连接，成功返回连接页快照。

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SaveAiSettingsArgs {
    pub provider: ProviderInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderInput {
    // 0 创建，正数更新；不存在的正数不自动创建。
    pub id: i64,
    // 缺省时保留已有 kind；创建时使用自定义兼容预设。
    #[serde(default)]
    #[ts(optional)]
    pub kind: Option<String>,
    pub name: String,
    pub base_url: String,
    // 空或缺省保留已有密钥。
    #[serde(default)]
    pub api_key: Option<String>,
    // 为真时清除已有密钥（本机服务不需要）。
    #[serde(default)]
    #[ts(optional)]
    pub clear_key: Option<bool>,
    pub protocol: String,
    // 省略则保留已有值（创建时使用预设）；空白表示恢复默认空对象。
    #[serde(default)]
    #[ts(optional)]
    pub compat_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub protocol: String,
    pub compat_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    Supported,
    Unsupported,
    Unknown,
}

impl From<rrss_core::model_pool::Support> for CapabilitySupport {
    fn from(value: rrss_core::model_pool::Support) -> Self {
        use rrss_core::model_pool::Support;
        match value {
            Support::Supported => Self::Supported,
            Support::Unsupported => Self::Unsupported,
            Support::Unknown => Self::Unknown,
        }
    }
}

/// 模型的四项能力（阶段 19-7）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct CapsView {
    pub tool_calling: CapabilitySupport,
    pub structured_output: CapabilitySupport,
    pub image_input: CapabilitySupport,
    pub reasoning: CapabilitySupport,
}

impl From<rrss_core::model_pool::Caps> for CapsView {
    fn from(c: rrss_core::model_pool::Caps) -> Self {
        Self {
            tool_calling: c.tool_calling.into(),
            structured_output: c.structured_output.into(),
            image_input: c.image_input.into(),
            reasoning: c.reasoning.into(),
        }
    }
}

/// 手动设置：缺省即按测试结果。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
pub struct CapsOverrideView {
    #[serde(default)]
    #[ts(optional)]
    pub tool_calling: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub structured_output: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub image_input: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub reasoning: Option<bool>,
}

impl From<rrss_core::model_pool::CapsOverride> for CapsOverrideView {
    fn from(o: rrss_core::model_pool::CapsOverride) -> Self {
        Self {
            tool_calling: o.tool_calling,
            structured_output: o.structured_output,
            image_input: o.image_input,
            reasoning: o.reasoning,
        }
    }
}

impl From<CapsOverrideView> for rrss_core::model_pool::CapsOverride {
    fn from(o: CapsOverrideView) -> Self {
        Self {
            tool_calling: o.tool_calling,
            structured_output: o.structured_output,
            image_input: o.image_input,
            reasoning: o.reasoning,
        }
    }
}

/// 「测试」连接：只请求一次模型列表。已保存的连接地址与密钥没改时按 id 取库里的值，结果记到连接上。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConnectionTestArgs {
    // 0 是还没保存的草稿。
    pub provider_id: i64,
    pub base_url: String,
    // 缺省或为空时用库里已保存的密钥。
    #[serde(default)]
    #[ts(optional)]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ConnectionTestView {
    pub ok: bool,
    pub latency_ms: u64,
    // 返回的模型数；失败时为 0。
    pub models: i64,
    pub error: String,
}

// 复杂的 Pi 配置编辑；与只改路径的旧 SavePiSettingsArgs 分开。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PiConfigView {
    pub settings_override: String,
    pub models_override: String,
    pub use_own_config: bool,
    pub own_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelChoice {
    pub provider_id: i64,
    pub model: String,
}

// 连接上获取到的模型 id（阶段 19-6）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelCatalogView {
    pub provider_id: i64,
    pub models: Vec<String>,
    pub fetched_at: i64,
}

/// 加进来的一个模型与它的设置（阶段 19-7）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PoolEntryView {
    pub provider_id: i64,
    pub model: String,
    // 出现在对话的模型列表里。
    pub chat: bool,
    // 留空显示模型 id。
    pub display_name: String,
    // 对话里的思考强度：off / low / medium / high。
    pub thinking: String,
    #[serde(default)]
    #[ts(optional)]
    pub context_window: Option<i64>,
    // 最近一次握手报告的实际窗口；查询不启动引擎。
    #[serde(default)]
    #[ts(optional)]
    pub engine_context_window: Option<i64>,
    #[serde(default)]
    #[ts(optional)]
    pub max_output: Option<i64>,
    // 单独设置的兼容 JSON；缺省用连接的。
    #[serde(default)]
    #[ts(optional)]
    pub compat_json: Option<String>,
    // 「测试能力」的结果；没测过全是 unknown。
    pub detected: CapsView,
    pub overrides: CapsOverrideView,
    // 手动设置 > 测试结果 > 未知。
    pub effective: CapsView,
    #[serde(default)]
    #[ts(optional)]
    pub tested_at: Option<i64>,
    #[serde(default)]
    #[ts(optional)]
    pub recommended_compat_json: Option<String>,
    // 选了它的用途。
    pub uses: Vec<String>,
}

// 加进来的模型与各连接的模型目录。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelPoolView {
    pub pool: Vec<PoolEntryView>,
    pub catalogs: Vec<ModelCatalogView>,
    // 这次移出后改用替换模型的用途；查询时为空。
    pub replaced: Vec<String>,
    // 这次移出后变为未选择的用途；查询时为空。
    pub unset: Vec<String>,
}

/// 加入与移出一次提交。被移出的模型正被用途使用时，这些用途改用 `replace`；缺省则变为未选择。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SaveModelPoolArgs {
    pub add: Vec<ModelChoice>,
    pub remove: Vec<ModelChoice>,
    #[serde(default)]
    #[ts(optional)]
    pub replace: Option<ModelChoice>,
}

/// 改一个模型的设置；缺省的字段不改。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct UpdatePoolModelArgs {
    pub provider_id: i64,
    pub model: String,
    #[serde(default)]
    #[ts(optional)]
    pub chat: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub display_name: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub thinking: Option<String>,
    // 0 清空。
    #[serde(default)]
    #[ts(optional)]
    pub context_window: Option<i64>,
    // 0 清空。
    #[serde(default)]
    #[ts(optional)]
    pub max_output: Option<i64>,
    // 空白改回用连接的兼容设置。
    #[serde(default)]
    #[ts(optional)]
    pub compat_json: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub overrides: Option<CapsOverrideView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelBindingsView {
    pub tasks: Vec<TaskBindView>,
    pub chat_uses_own_config: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TaskModelInput {
    pub task: String,
    // 0 表示不选（这个用途暂停）；此时 model 应为空。
    pub provider_id: i64,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SaveModelBindingsArgs {
    #[serde(default)]
    pub bindings: Vec<TaskModelInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SavePiConfigArgs {
    #[serde(default)]
    #[ts(optional)]
    pub settings_override: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub models_override: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub use_own_config: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub own_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TranslatePage {
    pub selection_config: Option<SelectionTranslationConfig>,
    pub enabled: bool,
    pub backend: String,
    pub target_lang: String,
    pub deepl_key: SecretView,
    pub deepl_url: String,
    pub deeplx_url: String,
    pub deeplx_token: SecretView,
    pub http_url: String,
    pub http_key: SecretView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SelectionTranslationConfig {
    pub identity: String,
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SelectionTranslation {
    pub text: String,
    pub config: SelectionTranslationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MemoryPage {
    pub memories: Vec<MemoryView>,
    pub interests: Vec<InterestView>,
    pub inject_preview: String,
}

// AI 设置页「输入档位」：当前档位与各档说明，数值只在 rrss-core 定义。
// 设置选项注册表（阶段 19-1）。简单选项的描述符与当前值；复杂对象（连接、绑定、工具、技能、MCP）不在这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SettingDomain {
    Model,
    Assistant,
    Reader,
}

// L0 常用、L1 次常用、L2 高级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SettingLevel {
    L0,
    L1,
    L2,
}

// 开关、枚举、时刻立即生效；数字、文本走草稿和保存栏。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SettingApply {
    Immediate,
    Draft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    Bool,
    Int { min: i64, max: i64 },
    Enum { options: Vec<SettingOption> },
    Text { max: i64 },
    Json,
}

// 枚举项的标签与说明由后端给出（输入档位的说明带着 rrss-core 的数值）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SettingOption {
    pub id: String,
    pub label: String,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SettingSpec {
    // 即 settings 表的键。
    pub id: String,
    pub domain: SettingDomain,
    pub level: SettingLevel,
    pub kind: SettingKind,
    pub default: String,
    pub apply: SettingApply,
    // 是否计入 Pi 配置指纹：改了之后下次发送时生效。
    pub affects_pi: bool,
}

// 值一律是规范化后的字符串：开关为 "1" / "0"，数字为十进制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SettingValue {
    pub id: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SettingsSchema {
    pub specs: Vec<SettingSpec>,
    pub values: Vec<SettingValue>,
}

// 存储页。大小、条数和确认文案都在这里生成；`bytes` 只给前端画比例条。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StoragePage {
    pub data_dir: String,
    pub total: String,
    pub summary: String,
    pub groups: Vec<StorageGroupView>,
    // 统计时间（Unix 毫秒）。
    pub scanned_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StorageGroupView {
    pub key: String,
    pub label: String,
    pub size: String,
    pub note: String,
    pub items: Vec<StorageItemView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StorageItemView {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub bytes: i64,
    pub size: String,
    // 空闲空间：比例条用斜纹表示可回收。
    pub free: bool,
    // 传给 `storage_clean` 的动作名；为空表示没有操作，显示 `passive`。
    pub action: String,
    pub action_label: String,
    pub passive: String,
    pub confirm_title: String,
    pub confirm: Vec<String>,
}

// 存储页「保留策略」分组。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RetentionPage {
    // 全局保留天数，0 表示关闭。
    pub days: i64,
    // 开启时预设的天数。
    pub default_days: i64,
    // 单独设过保留的源个数。
    pub custom_feeds: i64,
    pub preview: String,
    pub preview_hint: String,
    pub can_run: bool,
    pub confirm_title: String,
    pub confirm: Vec<String>,
    pub confirm_ok: String,
}

// 存储页「备份与恢复」分组。与占用统计分开读取，打开页面不必等扫库。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BackupPage {
    // 「上次备份：…」或还没有手动备份时的说明。
    pub last_manual: String,
    // off / daily / weekly
    pub schedule: String,
    pub auto_summary: String,
    pub entries: Vec<BackupEntryView>,
    // 上次启动时执行恢复的结果；用户关掉后清除。
    pub restore_notice: Option<RestoreNoticeView>,
    pub rules: i64,
    pub interests: i64,
    pub memories: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BackupEntryView {
    pub path: String,
    pub when: String,
    pub note: String,
    // 读不出清单的文件只列出，不能恢复。
    pub restorable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RestoreNoticeView {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BackupDoneView {
    pub path: String,
    pub size: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RestoreCheckView {
    pub path: String,
    pub file_name: String,
    // 「备份于 9 月 20 日 22:40 · 版本 v14」
    pub taken: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PrefsImportView {
    pub summary: String,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProxyMode {
    Direct,
    System,
    Custom,
    Inherit,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkProxyRuleView {
    pub mode: NetworkProxyMode,
    // 脱敏后的代理地址；认证只用 has_auth 表示。
    pub url: String,
    pub no_proxy: String,
    pub has_auth: bool,
}

// 测活按用途：全局本身不发请求，跟随全局的用途测的就是全局。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProxyScope {
    Content,
    Api,
    Pi,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkProxyTestArgs {
    pub scope: NetworkProxyScope,
    // 整份未保存草稿，与保存同一套认证合并规则。
    pub draft: SaveNetworkProxyArgs,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkProxyTestView {
    pub ok: bool,
    pub latency_ms: u64,
    pub status: u16,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct SystemProxyView {
    pub http: Option<String>,
    pub https: Option<String>,
    pub all: Option<String>,
    pub no_proxy: Option<String>,
    pub source: String,
    pub pac: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NetworkProxyView {
    pub global: NetworkProxyRuleView,
    pub content: NetworkProxyRuleView,
    pub api: NetworkProxyRuleView,
    pub pi: NetworkProxyRuleView,
    pub system: SystemProxyView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SaveNetworkProxyRuleArgs {
    pub mode: NetworkProxyMode,
    pub url: String,
    pub no_proxy: String,
    // 缺省表示保留已保存值；空串表示明确写入空值。
    #[serde(default)]
    #[ts(optional)]
    pub username: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub password: Option<String>,
    #[serde(default)]
    pub clear_auth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SaveNetworkProxyArgs {
    pub global: SaveNetworkProxyRuleArgs,
    pub content: SaveNetworkProxyRuleArgs,
    pub api: SaveNetworkProxyRuleArgs,
    pub pi: SaveNetworkProxyRuleArgs,
}

// 自绘标题栏的窗口三键按哪个平台的样子画，平台判断见 `rrss_core::platform::Os::window_controls`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WindowControlsStyle {
    Windows,
    Gnome,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct WindowChromeView {
    pub controls: WindowControlsStyle,
    // 系统托盘能否显示；不能时「隐藏到托盘」退化为最小化，设置页据此提示。
    pub tray: bool,
}

// 托盘菜单的文字：前端按当前界面语言交给宿主（展示文案不进后端）。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TrayLabels {
    pub open: String,
    pub refresh: String,
    pub quit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DiagnosticsPage {
    pub scheduler: Option<SchedulerDiagnostics>,
    pub vec: String,
    pub stats: String,
    pub db: String,
    pub ai: String,
    // 没有初始化文件日志（测试、工具进程）时为空。
    pub log: Option<LogStatusView>,
}

// 文件日志写不进去的原因；界面按原因给出处理办法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LogUnavailableReason {
    // 另一个 RRSS 进程持有日志锁。
    OtherInstance,
    NotWritable,
    DiskFull,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LogStatusView {
    pub directory: String,
    // 当前文件；「在文件夹中显示」选中它。
    pub file: String,
    // 为空表示正在写入。
    pub unavailable: Option<LogUnavailableReason>,
    pub current_size: String,
    pub total_size: String,
    pub total_bytes: i64,
    // 本次运行是否已切到详细级别。
    pub detailed: bool,
    // 上次没有正常退出时，最后一条记录的时间（RFC 3339，带时区）；时间不可读时为 "unknown"。
    pub previous_unclean: Option<String>,
    // 本进程持有日志锁，可以清空。
    pub can_clear: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
// 空闲超时、思考强度、自动下载 / 更新走选项注册表；版本由 `PiProbe` 返回。
pub struct PiPage {
    pub developer_tools: bool,
    pub fetch_url: bool,
    #[serde(default)]
    pub pi_path: String,
    pub binary_source: String,
    pub session_dir: String,
    pub manage_status: String,
    // 固定系统提示模板原文，只读展示；助手说明追加在它之后。
    pub system_prompt: String,
    // 最近一次握手得到的可用思考档位；从未启动过时为空。
    pub thinking_levels: Vec<String>,
    pub tools: Vec<PiToolView>,
    pub skills: Vec<SkillView>,
    pub mcp_servers: Vec<McpView>,
}

/// Pi 页「状态」区一次保存的输入：路径 / 空闲超时 / 思考等级同一事务，成功返回 `PiPage`。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SavePiSettingsArgs {
    // 空闲超时与思考强度已改由选项注册表写入（`set_setting`）。
    pub pi_path: String,
}

/// `pi --version` / `pi list` 探测结果；与 `PiPage` 分开返回，页面切换不等子进程。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PiProbe {
    pub binary_source: String,
    pub version: String,
    // 版本闸结论：missing / too_old / unverified / ok（阶段 18 版本闸）。
    pub version_state: String,
    // 按当前配置启动会遇到的配置错误（如没有对话模型）；为空表示可以启动。
    pub config_error: String,
}

/// 托管 Pi 的下载 / 安装进度。下载在后台进行，离开设置页不中断；完成或失败后保留最后一次结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct PiInstallView {
    /// idle / downloading / installing / done / failed。
    pub state: String,
    /// 已下载字节。
    pub received: i64,
    /// 安装包总字节；服务器没给时为空。
    pub total: Option<i64>,
    /// 完成时的结果说明或失败原因。
    pub message: String,
    /// 托管 Pi 的安装目录，只供展示。
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NewRuleSpec {
    pub polarity: String,
    pub dimension: String,
    pub pattern: String,
    pub weight: i64,
    pub scope_kind: String,
    pub scope_id: i64,
}

/// 本地时区的绝对时间，供阅读器元信息等不需要相对说法的地方。
pub fn format_time(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return String::new();
    };
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn article_from_card(card: rrss_core::ArticleCard) -> ArticleView {
    ArticleView {
        id: card.id,
        title: card.title,
        feed_id: card.feed_id,
        feed_title: card.feed_title,
        feed_icon_href: String::new(),
        published_at: card.published_at,
        url: card.canonical_url.unwrap_or_default(),
        read: card.read,
        starred: card.starred,
        score: card.score,
        summary: card.summary,
        row_kind: card.row_kind,
        cluster_id: card.cluster_id.unwrap_or(0),
        cluster_size: card.cluster_size,
        cluster_label: card.cluster_label,
        cluster_span: card.cluster_span,
        interest_label: card.interest_label,
        rank_reasons: Some(
            card.rank_reasons
                .into_iter()
                .map(|reason| RankReasonView {
                    text: reason.text,
                    points: reason.points,
                })
                .collect(),
        ),
        noisy: card.noisy,
        spot_check: card.spot_check,
        hide_reasons: None,
        cluster_fresh: None,
        cluster_sources: (card.row_kind == rrss_core::articles::ROW_HEADER).then(|| {
            card.cluster_feeds
                .into_iter()
                .map(|(feed_id, title)| ClusterSourceView {
                    feed_id,
                    title,
                    icon_href: String::new(),
                })
                .collect()
        }),
    }
}

pub fn excerpt_utf8(text: &str, a: i32, b: i32) -> String {
    if a == b {
        return String::new();
    }
    let a = clamp_utf8(text, a.max(0) as usize);
    let b = clamp_utf8(text, b.max(0) as usize);
    let (start, end) = if a <= b { (a, b) } else { (b, a) };
    text[start..end]
        .chars()
        .take(500)
        .collect::<String>()
        .trim()
        .to_string()
}

fn clamp_utf8(text: &str, mut i: usize) -> usize {
    if i > text.len() {
        i = text.len();
    }
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::excerpt_utf8;

    #[test]
    fn empty_when_offsets_equal() {
        assert_eq!(excerpt_utf8("hello world", 3, 3), "");
    }

    #[test]
    fn slices_and_trims() {
        assert_eq!(excerpt_utf8("  hello world  ", 0, 8), "hello");
    }

    #[test]
    fn handles_reversed_offsets_and_cjk() {
        assert_eq!(excerpt_utf8("新图书馆开放", 3, 12), "图书馆");
    }
}

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

// 阶段 23 · V1：只包含前端展示和预估所需数据，不暴露接口地址或密钥。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct VectorSpaceView {
    pub id: i64,
    pub model: String,
    pub provider_name: String,
    pub dims: i64,
    pub source_dims: i64,
    pub state: String,
    pub chunks: i64,
    pub stale_reason: Option<String>,
    pub checked_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct VectorRebuildView {
    pub space: VectorSpaceView,
    pub written: i64,
    pub total: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct VectorSpaceStatusView {
    pub effective: Option<VectorSpaceView>,
    pub building: Option<VectorRebuildView>,
    pub allow_model_change: bool,
    pub dims_setting: String,
    pub estimate_chunks: i64,
    pub estimate_tokens: i64,
    pub local: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ModelBindingError {
    pub kind: String,
    pub message: String,
}
impl From<rrss_core::Error> for ModelBindingError {
    fn from(error: rrss_core::Error) -> Self {
        let kind = match &error {
            rrss_core::Error::VectorSpaceRebuildRequired { .. } => "vector_space_rebuild_required",
            rrss_core::Error::VectorSpaceRebuildInProgress => "vector_space_rebuild_in_progress",
            _ => "other",
        };
        Self {
            kind: kind.into(),
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
pub enum WebLinkLayer {
    UserRule,
    BuiltinRule,
    General,
    Fallback,
    Container,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OpenWebLinkView {
    pub article_id: i64,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub web_extracted_at: Option<i64>,
    pub temporary: bool,
    pub layer: WebLinkLayer,
    #[serde(skip)]
    #[ts(skip)]
    pub suspicious: Vec<String>,
    pub text_chars: i64,
    pub image_count: i64,
    pub actual_host: String,
    pub actual_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum LinkWarning {
    MixedScript { unicode: String, punycode: String },
    HostMismatch { text_host: String },
    NonHttps,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LinkSafety {
    pub actual_host: String,
    pub warnings: Vec<LinkWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SiteRuleView {
    #[serde(default)]
    #[ts(optional)]
    pub javascript_disabled: Option<bool>,
    pub domain: String,
    pub body: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub strip: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StoredSiteRuleView {
    pub rule: SiteRuleView,
    pub enabled: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SiteRulesPage {
    pub users: Vec<StoredSiteRuleView>,
    pub builtin_count: i64,
    pub source: String,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SiteRuleTestView {
    pub layer: WebLinkLayer,
    pub title: String,
    pub author: Option<String>,
    pub date: Option<String>,
    pub text_chars: i64,
    pub image_count: i64,
    pub paragraphs: Vec<String>,
    pub stripped_elements: i64,
    pub elapsed_ms: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WebNavigated {
    pub label: String,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WebClosed {
    pub label: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WebEscape {
    pub label: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WebDownloadBlocked {
    pub label: String,
    pub url: String,
}

/// 阅读区里原网页子 webview 的位置与尺寸，单位是 CSS 逻辑像素。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct WebViewBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum WebVerifyState {
    WaitingVerification,
    PreExtract,
    Extracted,
    NotExtracted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct WebVerifyStateEvent {
    pub label: String,
    pub state: WebVerifyState,
    pub url: String,
    pub title: Option<String>,
    pub article_id: Option<i64>,
    pub text_chars: Option<i64>,
    pub image_count: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoverFeedsRequest {
    Start { request_id: String, url: String },
    Cancel { request_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoverFeedsResult {
    Candidates { candidates: Vec<DiscoveredFeed> },
    RsshubAvailable,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscoverFeedsError {
    PageUnavailable,
    NoFeed,
    SiteProtection,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DiscoveredFeed {
    pub url: String,
    pub title: Option<String>,
    #[ts(type = "'rss' | 'atom' | 'json'")]
    pub feed_type: String,
    // UTC milliseconds; absent when the feed supplies no dates.
    pub updated_at: Option<i64>,
    pub entry_count: usize,
    pub sample_titles: Vec<String>,
    pub subscribed: bool,
}
