//! 领域层错误。不吞失败，交给调用方记日志或退出。

use std::path::PathBuf;

/// `rrss-core` 可恢复 / 应上报的失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("data directory is unavailable")]
    DataDirUnavailable,
    #[error("unsupported schema version {found} (supported: {supported})")]
    UnsupportedSchema { found: i32, supported: i32 },
    #[error(
        "数据库版本 v{found} 过旧，最低支持 v{min}：请先用旧版本打开一次完成升级，或新建数据库"
    )]
    SchemaTooOld { found: i32, min: i32 },
    #[error("failed to create data directory {}: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("unsupported image content")]
    UnsupportedImage,
    #[error("feed not found")]
    FeedNotFound,
    #[error("digest not found")]
    DigestNotFound,
    #[error("article not found")]
    ArticleNotFound,
    #[error("feed already subscribed: {0}")]
    DuplicateFeed(String),
    #[error("folder not found")]
    FolderNotFound,
    #[error("invalid folder name")]
    InvalidFolderName,
    /// 订阅树移动或 `folder:<id>` / `feed:<id>` 解析被拒绝。
    #[error("{0}")]
    InvalidTreeOrder(String),
    #[error("rule not found")]
    RuleNotFound,
    #[error("invalid rule: {0}")]
    InvalidRule(String),
    #[error("preset pack not found: {0}")]
    PresetNotFound(String),
    #[error("MCP server not found")]
    McpServerNotFound,
    #[error("Pi tool not found: {0}")]
    PiToolNotFound(String),
    #[error("AI provider not found")]
    ProviderNotFound,
    #[error("{0}")]
    InvalidProvider(String),
    /// 选的模型不在模型列表里，或不适用于这个用途（阶段 19-7）。
    #[error("{0}")]
    InvalidModelChoice(String),
    #[error("本库的向量由 {model} 生成，换模型需要重建向量库。请先打开「允许更换嵌入模型」。")]
    VectorSpaceRebuildRequired { model: String },
    #[error("向量库正在重建，请先取消当前重建。")]
    VectorSpaceRebuildInProgress,
    /// 用途还没选模型（阶段 19-7）。
    #[error("「{0}」还没选模型，去设置 → 模型里选一个")]
    TaskModelUnset(String),
    #[error("invalid Pi compat object: {0}")]
    InvalidCompat(String),
    #[error("{0}")]
    InvalidPiOverride(String),
    /// 通用设置写入被描述符拒绝；文案可以直接展示。
    #[error("{0}")]
    InvalidSetting(String),
    /// 代理配置错误只使用固定文案，禁止包含原始地址或解析错误链。
    #[error("{0}")]
    InvalidNetworkProxy(String),
    #[error("thread not found")]
    ThreadNotFound,
    #[error("prompt template not found: {0}")]
    TemplateNotFound(String),
    #[error("unknown AI task: {0}")]
    UnknownAiTask(String),
    #[error("unknown storage cleanup: {0}")]
    UnknownCleanup(String),
    #[error("{0}")]
    Backup(String),
    #[error("daily AI budget exhausted")]
    DailyBudgetExhausted,
    #[error("memory not found")]
    MemoryNotFound,
    #[error("interest not found")]
    InterestNotFound,
    #[error("invalid memory: {0}")]
    InvalidMemory(String),
    #[error("parse feed: {0}")]
    Parse(String),
    #[error("OPML: {0}")]
    Opml(String),
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    // Retain the transport category before reqwest's unsafe source is discarded.
    #[error("HTTP {status}: {message}")]
    Network {
        kind: crate::network::ErrorKind,
        status: u16,
        message: crate::network::RequestErrorMessage,
    },
    #[error("{message}")]
    FetchDeferred { until: Option<i64>, message: String },
    /// Dispatch paused locally; retain the admitted claim for a later retry.
    #[error("{0}")]
    FetchPaused(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 本 crate 的 [`Result`](std::result::Result) 别名。
pub type Result<T> = std::result::Result<T, Error>;
