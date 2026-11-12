//! 导入 OPML 并抓取到期源，写入本机 SQLite。
//!
//! ```text
//! cargo run -p rrss-core --example import_opml
//! cargo run -p rrss-core --example import_opml -- testdata/feeds/opml/real.opml
//! cargo run -p rrss-core --example import_opml -- testdata/feeds/opml/real.opml /path/to/rrss.db
//! ```

use std::path::{Path, PathBuf};

use rrss_core::feeds::{self, now_ms};
use rrss_core::ingest::{FetchRequest, apply_fetch, fetch_http, http_client};
use rrss_core::{RefreshStatus, db, import_opml};

fn default_opml() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/feeds/opml/real.opml")
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opml_path = args.first().map(PathBuf::from).unwrap_or_else(default_opml);
    let db_path = match args.get(1) {
        Some(p) => PathBuf::from(p),
        None => match rrss_core::db_path() {
            Ok(p) => p,
            Err(err) => {
                eprintln!("db path: {err}");
                std::process::exit(1);
            }
        },
    };
    if let Err(err) = run(&opml_path, &db_path).await {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

async fn run(opml_path: &Path, db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let xml = std::fs::read_to_string(opml_path)?;
    let conn = db::open_at(db_path)?;
    let report = import_opml(&conn, &xml)?;
    println!(
        "opml {} → imported={} skipped={} folders={} db={}",
        opml_path.display(),
        report.imported,
        report.skipped_same_url,
        report.folders,
        db_path.display()
    );

    let client = http_client()?;
    let feeds = feeds::list_feeds(&conn)?;
    drop(conn);
    let mut inserted = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;
    for feed in feeds {
        let resp = match fetch_http(
            &client,
            FetchRequest {
                url: feed.url.clone(),
                etag: feed.etag.clone(),
                last_modified: feed.last_modified.clone(),
            },
        )
        .await
        {
            Ok(r) => r,
            Err(err) => {
                eprintln!("fetch {} failed: {err}", feed.url);
                failed += 1;
                continue;
            }
        };
        let conn = db::open_at(db_path)?;
        match apply_fetch(&conn, &feed, &resp, now_ms()) {
            Ok(result) => match result.status {
                RefreshStatus::Updated {
                    inserted: ins,
                    skipped: skip,
                    ..
                } => {
                    println!("{}: +{ins} skip {skip}", feed.url);
                    inserted += ins;
                    skipped += skip;
                }
                RefreshStatus::NotModified => println!("{}: not modified", feed.url),
                RefreshStatus::Deleted => println!("{}: deleted", feed.url),
                RefreshStatus::Skipped { message, .. } => {
                    println!("{}: skipped ({message})", feed.url)
                }
                RefreshStatus::Failed { message } => {
                    eprintln!("{}: {message}", feed.url);
                    failed += 1;
                }
            },
            Err(err) => {
                eprintln!("{}: {err}", feed.url);
                failed += 1;
            }
        }
    }
    println!("done inserted={inserted} skipped={skipped} failed={failed}");
    Ok(())
}
