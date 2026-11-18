//! Storage cleanup, retention, backup and restore contracts.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// Storage labels use ids. Messages carry raw counts and byte sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StorageGroupKind {
    Database,
    Media,
    Engine,
    Backups,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StorageItemKind {
    Articles,
    SearchIndex,
    Vectors,
    AiCache,
    Translations,
    RecommendationLogs,
    Other,
    Legacy,
    Free,
    Images,
    Videos,
    MediaOther,
    UntrackedMedia,
    PiProgram,
    PiSessions,
    AutoBackups,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StorageAction {
    OldVectors,
    AiCache,
    Translations,
    RecommendationLogs,
    LegacyTables,
    Vacuum,
    Images,
    Videos,
    UntrackedMedia,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StoragePassive {
    FollowsArticles,
    ManageElsewhere,
    ManageBackups,
    None,
    OtherLogInstance,
    LogUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "params", rename_all = "snake_case")]
pub enum StorageMessage {
    // Compatibility for an explicitly preserved display projection.
    LegacyText {
        text: String,
    },
    StorageSummary {
        database_bytes: i64,
        media_bytes: i64,
        engine_bytes: i64,
        backup_bytes: i64,
        log_bytes: i64,
    },
    StorageArticles {
        count: i64,
    },
    StorageSearchIndex {},
    StorageVectorsUnset {
        count: i64,
    },
    StorageVectorsOther {
        count: i64,
        old_count: i64,
    },
    StorageVectorsCurrent {
        count: i64,
    },
    StorageOldVectorsConfirm {
        count: i64,
    },
    StorageOldVectorsWarning {},
    StorageAiCache {
        count: i64,
    },
    StorageAiCacheConfirm {
        count: i64,
    },
    StorageAiCacheWarning {},
    StorageTranslations {
        count: i64,
    },
    StorageTranslationsConfirm {
        count: i64,
    },
    StorageTranslationsWarning {},
    StorageRecommendationLogs {
        count: i64,
    },
    StorageRecommendationLogsConfirm {
        count: i64,
    },
    StorageRecommendationLogsWarning {},
    StorageOther {
        chat_bytes: i64,
        threads: i64,
        other_bytes: i64,
        digest_bytes: i64,
        periods: i64,
    },
    StorageLegacy {},
    StorageLegacyConfirm {},
    StorageLegacyWarning {},
    StorageFree {
        required_bytes: i64,
        available_bytes: Option<i64>,
    },
    StorageVacuumConfirm {
        bytes: i64,
    },
    StorageVacuumSpace {
        required_bytes: i64,
        available_bytes: Option<i64>,
    },
    StorageVacuumWait {},
    StorageWal {
        bytes: i64,
    },
    StorageImages {},
    StorageImagesConfirm {
        bytes: i64,
    },
    StorageImagesWarning {},
    StorageVideos {},
    StorageVideosConfirm {
        bytes: i64,
    },
    StorageVideosWarning {},
    StorageMediaOther {},
    StorageUntracked {
        count: i64,
    },
    StorageUntrackedConfirm {
        count: i64,
        bytes: i64,
    },
    StorageUntrackedWarning {},
    StoragePiProgram {},
    StoragePiSessions {},
    StorageAutoBackups {
        count: i64,
        keep: i64,
    },
    StorageLogs {},
    StorageLogsConfirm {},
    StorageLogsWarning {},
    StorageLogsLocation {},
    StorageCleaned {
        action: StorageAction,
        count: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StoragePage {
    pub data_dir: String,
    pub total_bytes: Option<i64>,
    pub summary: Option<StorageMessage>,
    pub groups: Vec<StorageGroupView>,
    // 统计时间（Unix 毫秒）。
    pub scanned_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StorageGroupView {
    pub kind: StorageGroupKind,
    pub bytes: i64,
    pub note: Option<StorageMessage>,
    pub items: Vec<StorageItemView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StorageItemView {
    pub kind: StorageItemKind,
    pub hint: StorageMessage,
    pub bytes: i64,
    // Content names and internal table ids, never diagnostic parameters.
    pub model: String,
    pub tables: Vec<String>,
    // 空闲空间：比例条用斜纹表示可回收。
    pub free: bool,
    // None means no cleanup action; the frontend can display a passive label.
    pub action: Option<StorageAction>,
    pub passive: Option<StoragePassive>,
    pub confirm: Vec<StorageMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "params", rename_all = "snake_case")]
pub enum RetentionMessage {
    RetentionPreview { count: i64 },
    RetentionPreviewEmpty {},
    RetentionPreviewHint { bytes: i64, first: bool },
    RetentionDaily {},
    RetentionConfirm { count: i64, days: i64 },
    RetentionPreserve {},
    RetentionBackup {},
    RetentionCleaned { count: i64, backed_up: bool },
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
    pub preview: Option<RetentionMessage>,
    pub preview_hint: Option<RetentionMessage>,
    pub can_run: bool,
    pub first_run: bool,
    pub confirm: Vec<RetentionMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum BackupEntryKind {
    Manual,
    Upgrade,
    Scheduled,
    BeforeRestore,
    BeforePrune,
    Unreadable,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "params", rename_all = "snake_case")]
pub enum BackupMessage {
    BackupCreated {
        count: i64,
        feeds: i64,
        rules: i64,
        threads: i64,
        secrets_included: bool,
    },
    BackupRestoreSummary {
        count: i64,
        feeds: i64,
        rules: i64,
        threads: i64,
        secrets_included: bool,
    },
    PrefsRules {
        count: u32,
        duplicates: u32,
    },
    PrefsPacks {
        count: u32,
    },
    PrefsInterests {
        count: u32,
        duplicates: u32,
    },
    PrefsMemories {
        count: u32,
        duplicates: u32,
    },
    PrefsEmpty {},
    // 上次启动时的恢复结果；name 是备份文件名，读不出时为空。
    RestoreDone {
        name: Option<String>,
    },
    RestoreExitTimeout {
        name: Option<String>,
        #[ts(type = "number")]
        seconds: u64,
    },
    RestoreStagingIncomplete {
        name: Option<String>,
    },
    RestoreRolledBack {
        name: Option<String>,
    },
    // old_dir / kept_dir 是数据目录下的文件夹名。
    RestorePartial {
        name: Option<String>,
        old_dir: String,
        kept_dir: Option<String>,
    },
    // 旧版本写下的整句结果，原样显示。
    RestoreLegacy {
        text: String,
    },
}

// 存储页「备份与恢复」分组。与占用统计分开读取，打开页面不必等扫库。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BackupPage {
    pub last_manual_at: Option<i64>,
    // off / daily / weekly
    pub schedule: String,
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
    pub created_at: i64,
    pub kind: BackupEntryKind,
    pub schema_version: i64,
    pub bytes: i64,
    pub secrets_included: Option<bool>,
    // 读不出清单的文件只列出，不能恢复。
    pub restorable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RestoreNoticeView {
    pub ok: bool,
    pub message: BackupMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct BackupDoneView {
    pub path: String,
    pub bytes: i64,
    pub summary: BackupMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RestoreCheckView {
    pub path: String,
    pub file_name: String,
    pub created_at: i64,
    pub schema_version: i64,
    pub summary: BackupMessage,
}
