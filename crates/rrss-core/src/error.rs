//! 领域层错误。不吞失败，交给调用方记日志或退出。

use std::path::PathBuf;

pub mod models;
pub mod settings;
pub mod rules;
pub mod backup;

/// `rrss-core` 可恢复 / 应上报的失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("data directory is unavailable")]
    DataDirUnavailable,
    #[error("unsupported schema version {found} (supported: {supported})")]
    UnsupportedSchema { found: i32, supported: i32 },
    #[error("schema version {found} is too old (minimum: {min})")]
    SchemaTooOld { found: i32, min: i32 },
    #[error("failed to create data directory {}: {source}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid URL: {0}")]
    InvalidUrl(crate::url_norm::InvalidUrlReason),
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
    InvalidTreeOrder(crate::tree_order::TreeOrderError),
    #[error("rule not found")]
    RuleNotFound,
    #[error("invalid rule: {0}")]
    InvalidRule(rules::RuleProblem),
    #[error("preset pack not found: {0}")]
    PresetNotFound(String),
    #[error("MCP server not found")]
    McpServerNotFound,
    #[error("Pi tool not found: {0}")]
    PiToolNotFound(String),
    #[error("AI provider not found")]
    ProviderNotFound,
    #[error("{0}")]
    InvalidProvider(models::ProviderProblem),
    /// 选的模型不在模型列表里，或不适用于这个用途（阶段 19-7）。
    #[error("{0}")]
    InvalidModelChoice(models::ModelChoiceProblem),
    #[error("vector space model change requires a rebuild")]
    VectorSpaceRebuildRequired { model: String },
    #[error("vector space rebuild is in progress")]
    VectorSpaceRebuildInProgress,
    /// 用途还没选模型（阶段 19-7）。
    #[error("model is unset for {0}")]
    TaskModelUnset(models::TaskModelLabel),
    #[error("invalid Pi compat object: {0}")]
    InvalidCompat(models::CompatProblem),
    #[error("{0}")]
    InvalidPiOverride(settings::PiOverrideProblem),
    /// 通用设置写入被描述符拒绝；原因由前端展示。
    #[error("{0}")]
    InvalidSetting(settings::SettingProblem),
    /// 代理配置错误只使用固定文案，禁止包含原始地址或解析错误链。
    #[error("{0}")]
    InvalidNetworkProxy(crate::network::ProxyError),
    #[error("thread not found")]
    ThreadNotFound,
    #[error("prompt template not found: {0}")]
    TemplateNotFound(String),
    #[error("unknown AI task: {0}")]
    UnknownAiTask(String),
    #[error("unknown storage cleanup: {0}")]
    UnknownCleanup(String),
    #[error("{0}")]
    Backup(backup::BackupProblem),
    #[error("daily AI budget exhausted")]
    DailyBudgetExhausted,
    #[error("memory not found")]
    MemoryNotFound,
    #[error("interest not found")]
    InterestNotFound,
    #[error("invalid memory: {0}")]
    InvalidMemory(rules::MemoryProblem),
    #[error("parse feed: {0}")]
    Parse(String),
    #[error("parse feed: {0}")]
    FeedParse(crate::parse::FeedParseError),
    #[error("OPML: {0}")]
    Opml(crate::opml::OpmlError),
    #[error("{0}")]
    FeedRequest(crate::ingest::feed_http::FeedRequestError),
    #[error("media exceeds size limit: {bytes} bytes (maximum {max_bytes})")]
    MediaTooLarge { bytes: u64, max_bytes: u64 },
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    // Retain the transport category before reqwest's unsafe source is discarded.
    #[error("HTTP {status}: {message}")]
    Network {
        kind: crate::network::ErrorKind,
        status: u16,
        message: crate::network::RequestErrorMessage,
    },
    #[error("fetch deferred: {reason} (until: {until:?})")]
    FetchDeferred {
        until: Option<i64>,
        reason: crate::ingest::FetchDeferredReason,
    },
    /// Dispatch paused locally; retain the admitted claim for a later retry.
    #[error("network unavailable; check proxy settings if configured")]
    FetchPaused,
    #[error("pi failure: {0:?}")]
    Pi(PiFailure),
    #[error("mcp failure: {0:?}")]
    Mcp(McpFailure),
    #[error("jobs failure: {0:?}")]
    Jobs(JobFailure),
    #[error("web failure: {0:?}")]
    Web(WebFailure),
    #[error("host failure: {0:?}")]
    Host(HostFailure),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 本 crate 的 [`Result`](std::result::Result) 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// Structured failures projected by the application UI error boundary.
#[derive(Debug, Clone)]
pub enum PiFailure {
    NotInstalled,
    ChatModelUnset,
    ProviderUrlMissing { name: String },
    VersionTooOld { found: String, min: String },
    OwnModelUnset,
    SessionCancelled,
    SessionPathMissing,
    InputHandled,
    NotRunning,
    Timeout,
    ProcessExit,
    InputClosed,
    PipeMissing,
    ThreadGenerating,
    SendBusy,
    ReplyPending,
    RestartBusy,
    RestartPending,
    ModelsBusy,
    ModelsPending,
    ModelErrorMissing,
    ReplyEmpty,
    ExitEmpty,
    ExitPartial,
    SkillNameLength,
    SkillNameCharacters,
    SkillDescriptionMissing,
    SkillDescriptionLength,
    SkillNotFound { name: String },
    ReleaseMissing { line: String },
    PackageUnsupported { file: String },
    PackageBinaryMissing { file: String },
    PackageNotStandalone { file: String },
    ChecksumMissing { file: String },
    PlatformUnsupported,
}

/// Structured failures projected by the application UI error boundary.
#[derive(Debug, Clone)]
pub enum McpFailure {
    MessageTooLarge,
    CommandPathInvalid,
    SpawnFailed,
    ProcessExit { elapsed_ms: u64, status: i32 },
    ProcessKilled { elapsed_ms: u64 },
    OutputClosed,
    HttpStatus { status: u16 },
    ConnectFailed,
    ResponseTooLarge,
    StreamClosed,
    ResponseInvalid,
    ProtocolMissing,
    ProtocolUnsupported,
    ToolsMissing,
    ToolNameMissing,
    CursorStalled,
    PageLimit,
    SseUnsupported,
    OwnConfigProbe,
    ProbeTimeout,
    PiMissing,
    PiPathInvalid,
    LoginCancelled,
    LoginSpawnFailed,
    LoginProcessFailed,
    LoginTimeout,
    LoginBusy,
    OwnConfigLogin,
    LoginTransport,
    LoginFailed,
    LoginConfigChanged,
    LoginNotSaved,
    Deleting,
    LoginRequired,
    CredentialsUnavailable,
    LoginExpired,
}

/// Structured failures projected by the application UI error boundary.
#[derive(Debug, Clone)]
pub enum JobFailure {
    RuntimeStopped,
    ModelBusy,
    ModelStopped,
    ModelStoppedRetry,
    WaitLimit,
    InputChanged,
    DigestStopped,
    DigestPeriodInvalid,
    DigestWaitLimit,
    DigestBusy,
    EndpointInvalid,
    LimitRecordingInterrupted,
    ModelRuntimeStopped,
    BudgetCheckFailed,
    SpaceCheckFailed,
    SpaceChanged,
    SpaceUnreadable,
    SpaceVerifyFailed,
    SpaceProbeFailed,
    DigestNoArticles,
    DigestEmpty,
    DigestChanged,
    ClusterTooSmall,
    ClusterEmpty,
    SummaryChanged,
    SummaryDiscarded,
    ConfigChanged,
    EmbeddingStopped,
    EmbeddingChanged,
    ModelHttp { status: u16 },
    ModelBudget,
    ModelCooldown,
    EndpointBusy,
    RequestTimeout,
    RequestConnect,
    RequestDecode,
    RequestFailed,
    DigestMissing,
    DigestUpdated,
    DigestSelectSources,
    DigestDuplicate,
    DigestBlockInvalid,
    DigestBlockNoSources,
    DigestItemChanged,
    DigestSourcesMissing,
    DigestContentChanged,
    DigestSourceRemoved,
    DigestSourceUnavailable,
    DigestDraftNotObject,
    DigestDraftSectionsInvalid,
    DigestDraftEmpty,
    DigestDraftSourcesMismatch,
    DigestDraftBodyMissing,
    DigestDraftSerialize,
}

/// Structured failures projected by the application UI error boundary.
#[derive(Debug, Clone)]
pub enum WebFailure {
    OperationFailed,
    BoundsInvalid,
    DataClearFailed,
    VerifyUnavailable,
    Closed,
    NavigationCancelled,
    UrlChanged,
    PageChanged,
    SnapshotProcessing,
    UrlUnavailable,
    NativeFailed,
    ActionUnknown,
    SnapshotUnsupported,
    SnapshotTimeout,
    SnapshotCancelled,
    SnapshotTooLarge,
    WebkitContextMissing,
    WebkitDataMissing,
    WebkitSettingsMissing,
    ParentMissing,
    ParentInvalid,
    MainMissing,
    WebkitSnapshotFailed,
    WebkitSnapshotRead,
    Webview2Unavailable,
    Webview2Title,
    Webview2Snapshot,
    Webview2SnapshotInvalid,
    MimeLimit,
    MimeHeadersInvalid,
    MimeTypeMissing,
    MimeBoundaryMissing,
    MimeBoundaryInvalid,
    MimeTruncated,
    Base64Invalid,
    QuotedPrintableInvalid,
    EncodingUnsupported,
    HtmlMissing,
    SnapshotUrlInvalid,
    SnapshotSchemeUnsupported,
    FulltextWaf,
    FulltextNetwork,
    FulltextEmpty,
    FulltextNotArticle,
    FulltextProxyAuth,
    FulltextTooLarge,
    ContentBusy,
    DirectTooLarge,
    DirectNotSupported,
    DirectHttp { status: u16 },
    DirectStore,
}

/// Structured failures projected by the application UI error boundary.
#[derive(Debug, Clone)]
pub enum HostFailure {
    CommandForbidden,
    LogNotInitialized,
    LogLevelUnavailable,
    LogUnavailable,
}
