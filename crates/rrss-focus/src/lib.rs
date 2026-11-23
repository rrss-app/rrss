//! 焦点推荐：排序分量、拟合、排序缓存、探索与抽查，以及焦点页的组装。
//! 对应规格：推荐系统「接入方式」与「可维护性约定」。依赖方向 `rrss-app` → `rrss-focus` → `rrss-core`；
//! 表结构与迁移仍在 core，这里只经 core 的连接与查询读写 `article_rank`、曝光等表。

pub(crate) mod evidence;
pub mod labels;

pub mod affinity;
pub(crate) mod cache;
pub mod combine;
pub mod coverage;
pub(crate) mod explore;
pub mod features;
pub mod freshness;
pub mod model;
pub mod page;
pub mod read_model;
pub mod source;
pub(crate) mod spot_check;
pub mod store;
pub(crate) mod topics;

pub use page::{
    FOCUS_PAGE_SIZE, list_focus, list_focus_page, list_focus_stream, list_preload_article_ids,
};
