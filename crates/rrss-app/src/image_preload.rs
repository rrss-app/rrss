//! 焦点 / 收藏文章配图预载到磁盘缓存，打开阅读器时直接读盘。

use std::path::Path;
use std::sync::Arc;
use tracing::{Instrument, instrument::WithSubscriber};

use rrss_core::articles::{self, ArticleRow};
use rrss_core::{db, media};
use rrss_render::Block;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const IMAGES_PER_ARTICLE: usize = 4;
const PRELOAD_CONCURRENCY: usize = 2;

struct Job {
    article_id: i64,
    url: String,
    referer: String,
}

pub async fn run(db_path: &Path, data_dir: &Path, network: &crate::network::NetworkClients) {
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let _owner = capacity.images.lock().await;

    let db_path = db_path.to_path_buf();
    let data_dir = data_dir.to_path_buf();
    let jobs = match crate::execution::blocking({
        let db_path = db_path.clone();
        move || collect_jobs(&db_path)
    })
    .await
    {
        Ok(jobs) => jobs,
        Err(err) => {
            tracing::warn!(error = %err, "image preload collect join");
            return;
        }
    };
    if jobs.is_empty() {
        return;
    }
    tracing::info!(count = jobs.len(), "preloading article images");
    let client = match network
        .client(rrss_core::network::NetworkScope::Content)
        .await
    {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "image preload network configuration");
            return;
        }
    };
    let limit = Arc::new(Semaphore::new(PRELOAD_CONCURRENCY));
    let mut set = JoinSet::new();
    for job in jobs {
        let client = client.clone();
        let db_path = db_path.clone();
        let data_dir = data_dir.clone();
        let limit = limit.clone();
        set.spawn(
            async move {
                let Ok(_permit) = limit.acquire().await else {
                    return;
                };
                let _ = crate::image_fetch::fetch_and_store_with_priority(
                    &client,
                    &db_path,
                    &data_dir,
                    job.article_id,
                    &job.url,
                    &job.referer,
                    crate::scheduler::model_capacity::Priority::Background,
                )
                .await;
            }
            .in_current_span()
            .with_current_subscriber(),
        );
    }
    while set.join_next().await.is_some() {}
}

fn collect_jobs(db_path: &Path) -> Vec<Job> {
    let Ok(conn) = db::open_at(db_path) else {
        return Vec::new();
    };
    let Ok(ids) = articles::list_preload_article_ids(&conn) else {
        return Vec::new();
    };
    let mut jobs = Vec::new();
    for id in ids {
        let Ok(article) = articles::get_article(&conn, id) else {
            continue;
        };
        let referer = article.canonical_url.clone().unwrap_or_default();
        let mut n = 0;
        for src in image_srcs(&article) {
            if n >= IMAGES_PER_ARTICLE {
                break;
            }
            if !media::allowed_url(&src) {
                continue;
            }
            if media::cached_path(&conn, article.id, &src)
                .ok()
                .flatten()
                .is_some()
            {
                continue;
            }
            jobs.push(Job {
                article_id: article.id,
                url: src,
                referer: referer.clone(),
            });
            n += 1;
        }
    }
    jobs
}

fn image_srcs(article: &ArticleRow) -> Vec<String> {
    crate::document::load_document(article)
        .blocks
        .into_iter()
        .filter_map(|block| match block {
            Block::Image { src, .. } => Some(src),
            _ => None,
        })
        .collect()
}
