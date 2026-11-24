//! Subscription management contracts and domain-to-view conversions.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::UiError;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeedView {
    pub id: i64,
    pub title: String,
    pub icon_href: String,
    pub url: String,
    pub error: Option<UiError>,
    pub noisy: bool,
    pub folder_id: i64,
    // 保留天数：null 跟随全局，0 永久保留。
    pub retention_days: Option<i64>,
    // 刷新间隔（分钟）：null 跟随全局设置。
    pub interval_minutes: Option<i64>,
    // 上次抓取与下次到期（UTC 毫秒）；手动收录源或从未排期时为 null。
    pub last_fetched_at: Option<i64>,
    pub next_fetch_at: Option<i64>,
    // 所在网站限流到何时（UTC 毫秒）；来自调度器内存，宿主返回订阅树前填写，查询里恒为 null。
    pub limited_until: Option<i64>,
    // 源健康：码与原始参数；null 表示没有提示（或已忽略）。
    pub health: Option<crate::ui_error::feeds::FeedHealthNotice>,
    // 与同级文件夹共用的顺序。
    pub position: i64,
    // 手动收录源还没有任何文章；订阅树据此隐藏它。其他源恒为 false。
    pub empty: bool,
    // 自动翻译覆盖：always 总是、never 从不；跟随全局时不带。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auto_translate: Option<String>,
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
    // 刷新间隔："follow" 跟随全局，其余为分钟数。
    #[serde(default)]
    pub interval: Option<String>,
    // 自动翻译："follow" / "always" / "never"。
    #[serde(default)]
    #[ts(optional = nullable)]
    pub auto_translate: Option<String>,
}

// OPML 导入结果的计数，提示文字由前端生成。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OpmlImportView {
    pub imported: i64,
    pub skipped_same_url: i64,
    pub folders: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AddFeedResult {
    pub id: i64,
    pub notice: AddFeedNotice,
}

// 添加订阅后的抓取安排；error 是延期或未能安排的原因。
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "params", rename_all = "snake_case")]
pub enum AddFeedNotice {
    Fetching {},
    Offline {},
    Deferred { error: UiError },
    NotScheduled { error: UiError },
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
    // 文件夹的缩进标签，按订阅树顺序；「未分类」由前端补。
    pub folder_combo: Vec<String>,
}

impl From<rrss_core::opml::ImportReport> for OpmlImportView {
    fn from(report: rrss_core::opml::ImportReport) -> Self {
        Self {
            imported: i64::from(report.imported),
            skipped_same_url: i64::from(report.skipped_same_url),
            folders: i64::from(report.folders),
        }
    }
}

impl From<rrss_core::tree_order::TreePlacement> for TreeLayout {
    fn from(slot: rrss_core::tree_order::TreePlacement) -> Self {
        Self {
            item: slot.item.key(),
            parent_folder_id: slot.parent,
            position: slot.position,
        }
    }
}

impl TryFrom<&TreeLayout> for rrss_core::tree_order::TreePlacement {
    type Error = rrss_core::Error;

    fn try_from(row: &TreeLayout) -> Result<Self, Self::Error> {
        Ok(Self {
            item: rrss_core::tree_order::TreeItem::parse(&row.item)?,
            parent: row.parent_folder_id,
            position: row.position,
        })
    }
}
