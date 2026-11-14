//! OPML 导入 / 导出。日志 target：`rrss_core::opml`。

use opml::{Head, OPML, Outline};
use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::feeds::{self, SubscribeRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OpmlError {
    #[error("invalid document")]
    InvalidDocument,
    #[error("document serialization failed")]
    Serialize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: u32,
    pub skipped_same_url: u32,
    pub folders: u32,
}

pub fn import_opml(conn: &Connection, xml: &str) -> Result<ImportReport> {
    let doc = OPML::from_str(xml).map_err(|_| Error::Opml(OpmlError::InvalidDocument))?;
    let mut report = ImportReport::default();
    walk_outlines(conn, &doc.body.outlines, None, &mut report)?;
    tracing::info!(
        imported = report.imported,
        skipped = report.skipped_same_url,
        folders = report.folders,
        "opml imported"
    );
    Ok(report)
}

fn walk_outlines(
    conn: &Connection,
    outlines: &[Outline],
    folder_id: Option<i64>,
    report: &mut ImportReport,
) -> Result<()> {
    for outline in outlines {
        if let Some(xml_url) = outline.xml_url.as_deref().filter(|s| !s.is_empty()) {
            match feeds::subscribe(
                conn,
                SubscribeRequest {
                    url: xml_url.to_string(),
                    folder_id,
                    as_rsshub: false,
                    rsshub_instance_id: None,
                },
            ) {
                Ok(feed_id) => {
                    // OPML 里的名称多是用户在原阅读器里起的，按自定义名称保留；
                    // 空名或 URL 形式的不算，留给刷新填入源标题。
                    let name = outline
                        .title
                        .as_deref()
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .unwrap_or(outline.text.trim());
                    if !name.is_empty() && !feeds::looks_like_url(name) {
                        let _ = conn.execute(
                            "UPDATE feeds SET title = ?1, title_custom = 1 WHERE id = ?2",
                            rusqlite::params![name, feed_id],
                        );
                    }
                    if let Some(html) = outline.html_url.as_deref() {
                        let _ = conn.execute(
                            "UPDATE feeds SET site_url = COALESCE(site_url, ?1) WHERE id = ?2",
                            rusqlite::params![html, feed_id],
                        );
                    }
                    report.imported += 1;
                }
                Err(Error::DuplicateFeed(_)) => report.skipped_same_url += 1,
                Err(err) => return Err(err),
            }
        } else if !outline.outlines.is_empty() {
            let name = match outline.text.trim() {
                "" => "Folder",
                text => text,
            };
            // 同一层已有同名文件夹就并入，重复导入不再多出空文件夹。
            let id = match feeds::find_folder(conn, name, folder_id)? {
                Some(id) => id,
                None => {
                    let id = feeds::create_folder(conn, name, folder_id)?;
                    report.folders += 1;
                    id
                }
            };
            walk_outlines(conn, &outline.outlines, Some(id), report)?;
        }
    }
    Ok(())
}

pub fn export_opml(conn: &Connection) -> Result<String> {
    let folders = feeds::list_folders(conn)?;
    let feeds: Vec<_> = feeds::list_feeds(conn)?
        .into_iter()
        .filter(|f| f.kind != crate::kind::FeedKind::Manual)
        .collect();
    let mut root = Vec::new();
    build_folder_outlines(&folders, &feeds, None, &mut root);
    for feed in feeds.iter().filter(|f| f.folder_id.is_none()) {
        root.push(feed_outline(feed));
    }
    let doc = OPML {
        version: "2.0".into(),
        head: Some(Head {
            title: Some("RRSS".into()),
            ..Head::default()
        }),
        body: opml::Body { outlines: root },
    };
    doc.to_string()
        .map_err(|_| Error::Opml(OpmlError::Serialize))
}

fn build_folder_outlines(
    folders: &[feeds::FolderRow],
    feeds: &[feeds::FeedRow],
    parent: Option<i64>,
    out: &mut Vec<Outline>,
) {
    for folder in folders.iter().filter(|f| f.parent_id == parent) {
        let mut node = Outline {
            text: folder.name.clone(),
            ..Outline::default()
        };
        build_folder_outlines(folders, feeds, Some(folder.id), &mut node.outlines);
        for feed in feeds.iter().filter(|f| f.folder_id == Some(folder.id)) {
            node.outlines.push(feed_outline(feed));
        }
        out.push(node);
    }
}

fn feed_outline(feed: &feeds::FeedRow) -> Outline {
    Outline {
        text: feed.title.clone().unwrap_or_else(|| feed.url.clone()),
        title: feed.title.clone(),
        xml_url: Some(feed.url.clone()),
        html_url: feed.site_url.clone(),
        r#type: Some("rss".into()),
        ..Outline::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    const NESTED: &str = include_str!("../../../testdata/feeds/opml/nested.opml");
    const DUPS: &str = include_str!("../../../testdata/feeds/opml/duplicates.opml");

    #[test]
    fn import_nested_and_export_roundtrip() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let report = import_opml(&conn, NESTED).unwrap();
        assert_eq!(report.imported, 3);
        assert_eq!(report.folders, 2);
        assert_eq!(report.skipped_same_url, 0);
        let xml = export_opml(&conn).unwrap();
        assert!(xml.contains("news.example.com/rss.xml"));
        assert!(xml.contains("lab.example.org/atom.xml"));
        assert!(xml.contains("json.example.net/feed.json"));
        assert!(xml.contains("News"));
        assert!(xml.contains("Local"));
    }

    #[test]
    fn imported_folders_keep_outline_order() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0"><body>
  <outline text="甲"><outline text="A" xmlUrl="https://a.example/rss.xml"/></outline>
  <outline text="乙"><outline text="B" xmlUrl="https://b.example/rss.xml"/></outline>
</body></opml>"#;
        import_opml(&conn, xml).unwrap();
        let names: Vec<String> = feeds::list_folders(&conn)
            .unwrap()
            .into_iter()
            .filter(|folder| folder.parent_id.is_none())
            .map(|folder| folder.name)
            .collect();
        assert_eq!(names, ["甲".to_string(), "乙".to_string()]);
    }

    #[test]
    fn duplicate_urls_are_skipped() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let first = import_opml(&conn, DUPS).unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.skipped_same_url, 1);
        let second = import_opml(&conn, DUPS).unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped_same_url, 2);
    }

    #[test]
    fn reimport_reuses_same_name_folders_at_the_same_level() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let news = feeds::create_folder(&conn, "News", None).unwrap();
        let first = import_opml(&conn, NESTED).unwrap();
        assert_eq!(first.folders, 1, "only News / Local is new");
        let second = import_opml(&conn, NESTED).unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped_same_url, 3);
        assert_eq!(second.folders, 0);
        assert_eq!(feeds::list_folders(&conn).unwrap().len(), 2);
        let feed = feeds::list_feeds(&conn)
            .unwrap()
            .into_iter()
            .find(|f| f.url.contains("news.example.com/rss.xml"))
            .unwrap();
        assert_eq!(feed.folder_id, Some(news));
    }

    #[test]
    fn imported_names_survive_refresh_unless_they_match_the_source() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Names</title></head>
  <body>
    <outline text="我的新闻" xmlUrl="https://a.example.com/rss.xml"/>
    <outline text="Example News" xmlUrl="https://b.example.com/rss.xml"/>
    <outline text="https://c.example.com/rss.xml" xmlUrl="https://c.example.com/rss.xml"/>
  </body>
</opml>"#;
        assert_eq!(import_opml(&conn, xml).unwrap().imported, 3);
        let rss = include_str!("../../../testdata/feeds/rss2-sample.xml");
        let refresh_all = |body: &str| {
            for feed in feeds::list_feeds(&conn).unwrap() {
                let resp = crate::ingest::FetchResponse {
                    status: 200,
                    etag: None,
                    last_modified: None,
                    retry_after: None,
                    body: body.as_bytes().to_vec(),
                };
                crate::ingest::apply_fetch(&conn, &feed, &resp, 1).unwrap();
            }
            feeds::list_feeds(&conn)
                .unwrap()
                .into_iter()
                .map(|f| f.title.unwrap_or_default())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            refresh_all(rss),
            ["我的新闻", "Example News", "Example News"]
        );
        let renamed = rss.replace(
            "<title>Example News</title>",
            "<title>Example Daily</title>",
        );
        assert_eq!(
            refresh_all(&renamed),
            ["我的新闻", "Example Daily", "Example Daily"]
        );
    }
}
