use std::path::Path;

use rusqlite::Connection;

use rrss_core::ai_store;
use rrss_core::storage::{self, DbPart, DbUsage, MediaUsage};

use crate::views::*;

/// 大小的显示：1024 进制，与系统文件管理器一致。
pub fn fmt_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes.max(0) as f64;
    if b >= KB * KB * KB {
        format!("{:.2} GB", b / KB / KB / KB)
    } else if b >= 100.0 * KB * KB {
        format!("{:.0} MB", b / KB / KB)
    } else if b >= KB * KB {
        format!("{:.1} MB", b / KB / KB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else if bytes > 0 {
        format!("{bytes} B")
    } else {
        "0".into()
    }
}

pub fn fmt_count(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn item(key: &str, label: &str, hint: String, bytes: i64) -> StorageItemView {
    StorageItemView {
        key: key.into(),
        label: label.into(),
        hint,
        bytes,
        size: fmt_bytes(bytes),
        free: false,
        action: String::new(),
        action_label: String::new(),
        passive: String::new(),
        confirm_title: String::new(),
        confirm: Vec::new(),
    }
}

impl StorageItemView {
    fn action(mut self, action: &str, label: &str, title: &str, confirm: Vec<String>) -> Self {
        self.action = action.into();
        self.action_label = label.into();
        self.confirm_title = title.into();
        self.confirm = confirm;
        self
    }

    fn passive(mut self, text: &str) -> Self {
        self.passive = text.into();
        self
    }
}

fn part(usage: &DbUsage, part: DbPart) -> (i64, i64) {
    usage
        .parts
        .iter()
        .find(|p| p.part == part)
        .map(|p| (p.bytes, p.rows.unwrap_or(0)))
        .unwrap_or((0, 0))
}

fn database_group(conn: &Connection, db_path: &Path, usage: &DbUsage) -> StorageGroupView {
    let db_file = storage::file_bytes(db_path) as i64;
    let wal = storage::file_bytes(&wal_path(db_path)) as i64;
    let mut items = Vec::new();

    let (bytes, rows) = part(usage, DbPart::Articles);
    items.push(item(
        "articles",
        "文章",
        format!("{} 篇 · 标题、正文与阅读状态", fmt_count(rows)),
        bytes,
    ));

    let (bytes, _) = part(usage, DbPart::SearchIndex);
    items.push(
        item(
            "search_index",
            "全文索引",
            "供搜索使用，随文章增减".into(),
            bytes,
        )
        .passive("随文章变化"),
    );

    let (bytes, rows) = part(usage, DbPart::Vectors);
    let model = ai_store::resolve_embed(conn)
        .ok()
        .flatten()
        .map(|e| e.model);
    let old = storage::old_vector_count(conn).unwrap_or(0);
    let mut hint = format!("{} 段", fmt_count(rows));
    match &model {
        None => hint.push_str(" · 未设置向量模型"),
        Some(_) if old > 0 => hint.push_str(&format!(" · 其中 {} 段来自其他模型", fmt_count(old))),
        Some(m) => hint.push_str(&format!(" · 当前模型 {m}")),
    }
    let mut vectors = item("vectors", "向量", hint, bytes);
    if old > 0 {
        let model = model.clone().unwrap_or_default();
        vectors = vectors.action(
            "old_vectors",
            "清除旧模型向量",
            "清除旧模型向量",
            vec![
                format!("删除 {} 段不是由当前模型 {model} 生成的向量。", fmt_count(old)),
                "当前模型的向量不受影响；这些文章之后会按当前模型重新生成向量，远程模型会产生费用。".into(),
            ],
        );
    }
    items.push(vectors);

    let (bytes, rows) = part(usage, DbPart::AiCache);
    let mut ai = item(
        "ai_cache",
        "AI 缓存",
        format!("{} 条 · 30 天前的会自动清除", fmt_count(rows)),
        bytes,
    );
    if rows > 0 {
        ai = ai.action(
            "ai_cache",
            "清空",
            "清空 AI 缓存",
            vec![
                format!("删除 {} 条缓存的摘要和简报结果。", fmt_count(rows)),
                "之后遇到相同内容会重新调用模型，可能产生费用。".into(),
            ],
        );
    }
    items.push(ai);

    let (bytes, rows) = part(usage, DbPart::Translations);
    let mut tr = item(
        "translations",
        "译文缓存",
        format!("{} 篇", fmt_count(rows)),
        bytes,
    );
    if rows > 0 {
        tr = tr.action(
            "translations",
            "清空",
            "清空译文缓存",
            vec![
                format!("删除 {} 篇文章的译文。", fmt_count(rows)),
                "再次查看译文时会重新翻译，可能产生费用。".into(),
            ],
        );
    }
    items.push(tr);

    let (bytes, rows) = part(usage, DbPart::RecommendationLogs);
    let mut logs = item(
        "recommendation_logs",
        "推荐日志",
        format!(
            "{} 条 · 曝光、行为与阅读会话合计 · 保留 180 天",
            fmt_count(rows)
        ),
        bytes,
    );
    if rows > 0 {
        logs = logs.action(
            "recommendation_logs",
            "清空",
            "清空推荐日志",
            vec![
                format!("删除 {} 条曝光、行为与阅读会话日志。", fmt_count(rows)),
                "文章、赞踩、收藏与阅读状态不受影响。".into(),
            ],
        );
    }
    items.push(logs);

    let (chat, threads) = part(usage, DbPart::Conversations);
    let (other, _) = part(usage, DbPart::Other);
    let (digests, periods) = part(usage, DbPart::Digests);
    items.push(
        item(
            "other",
            "其他",
            format!(
                "对话与记忆 {}（{} 个对话）· 设置、订阅与规则 {} · 简报 {}（{} 期）",
                fmt_bytes(chat),
                fmt_count(threads),
                fmt_bytes(other),
                fmt_bytes(digests),
                fmt_count(periods)
            ),
            chat + other + digests,
        )
        .passive("在各自页面管理"),
    );

    let (bytes, _) = part(usage, DbPart::Legacy);
    let legacy = storage::legacy_tables(conn).unwrap_or_default();
    if !legacy.is_empty() {
        items.push(
            item(
                "legacy",
                "旧版遗留数据",
                format!("早期版本留下、已不再使用的表：{}", legacy.join("、")),
                bytes,
            )
            .action(
                "legacy_tables",
                "删除",
                "删除旧版遗留数据",
                vec![
                    format!("删除 {} 表。", legacy.join("、")),
                    "当前版本不再读取它们；译文已迁移到新表。".into(),
                ],
            ),
        );
    }
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes));

    let available = db_path.parent().and_then(storage::available_bytes);
    let mut free = item(
        "free",
        "空闲空间",
        format!(
            "删除数据后留下的空位，压缩后还给磁盘。需要约 {} 临时空间{}",
            fmt_bytes(db_file),
            available
                .map(|a| format!("，剩余 {}", fmt_bytes(a as i64)))
                .unwrap_or_default()
        ),
        usage.free_bytes,
    );
    free.free = true;
    free = if usage.free_bytes > 0 {
        free.action(
            "vacuum",
            "压缩数据库",
            "压缩数据库",
            vec![
                format!("把 {} 空闲空间还给磁盘。", fmt_bytes(usage.free_bytes)),
                format!(
                    "需要约 {} 临时磁盘空间{}，大库可能需要一两分钟。",
                    fmt_bytes(db_file),
                    available
                        .map(|a| format!("（剩余 {}）", fmt_bytes(a as i64)))
                        .unwrap_or_default()
                ),
                "期间刷新和后台作业会等待，可以继续阅读。".into(),
            ],
        )
    } else {
        free.passive("无")
    };
    items.push(free);

    StorageGroupView {
        key: "database".into(),
        label: "数据库".into(),
        size: fmt_bytes(db_file + wal),
        note: if wal > 0 {
            format!("含未写回的日志 {}", fmt_bytes(wal))
        } else {
            String::new()
        },
        items,
    }
}

fn media_group(media: &MediaUsage) -> StorageGroupView {
    let mut items = vec![
        item(
            "images",
            "配图缓存",
            "上限 256 MB，打开文章时按需下载".into(),
            media.image_bytes,
        ),
        item(
            "videos",
            "音视频缓存",
            "上限 512 MB".into(),
            media.video_bytes,
        ),
    ];
    if media.image_bytes > 0 {
        items[0] = items[0].clone().action(
            "images",
            "清空",
            "清空配图缓存",
            vec![
                format!(
                    "删除本地保存的配图，释放约 {}。",
                    fmt_bytes(media.image_bytes)
                ),
                "再打开文章时会重新下载。".into(),
            ],
        );
    }
    if media.video_bytes > 0 {
        items[1] = items[1].clone().action(
            "videos",
            "清空",
            "清空音视频缓存",
            vec![
                format!(
                    "删除本地保存的音视频，释放约 {}。",
                    fmt_bytes(media.video_bytes)
                ),
                "再播放时会重新下载。".into(),
            ],
        );
    }
    if media.other_bytes > 0 {
        items.push(item(
            "media_other",
            "封面信息",
            "视频封面等小文件".into(),
            media.other_bytes,
        ));
    }
    if media.untracked_files > 0 {
        items.push(
            item(
                "untracked_media",
                "未登记文件",
                format!(
                    "{} 个 · 中断的下载等，可以安全删除",
                    fmt_count(media.untracked_files)
                ),
                media.untracked_bytes,
            )
            .action(
                "untracked_media",
                "清理",
                "清理未登记文件",
                vec![
                    format!(
                        "删除媒体目录里没有登记的 {} 个文件，约 {}。",
                        fmt_count(media.untracked_files),
                        fmt_bytes(media.untracked_bytes)
                    ),
                    "不影响任何文章。".into(),
                ],
            ),
        );
    }
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let total = media.image_bytes + media.video_bytes + media.other_bytes + media.untracked_bytes;
    StorageGroupView {
        key: "media".into(),
        label: "媒体缓存".into(),
        size: fmt_bytes(total),
        note: String::new(),
        items,
    }
}

fn engine_group(data_dir: &Path) -> StorageGroupView {
    let program = (storage::dir_bytes(&crate::pi::install::managed_bin_dir(data_dir))
        + storage::dir_bytes(&crate::pi::launch::pi_pkg_dir(data_dir))) as i64;
    let sessions = storage::dir_bytes(&data_dir.join("pi/sessions")) as i64;
    let mut items = Vec::new();
    if program > 0 {
        items.push(item(
            "pi_program",
            "对话引擎程序",
            "RRSS 托管的 Pi".into(),
            program,
        ));
    }
    items.push(item(
        "pi_sessions",
        "对话记录（引擎侧）",
        "续接对话所需，随备份保存".into(),
        sessions,
    ));
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    StorageGroupView {
        key: "engine".into(),
        label: "对话引擎".into(),
        size: fmt_bytes(program + sessions),
        note: String::new(),
        items,
    }
}

/// 自动备份目录；没有备份时不显示这一组。
fn backups_group(data_dir: &Path) -> Option<StorageGroupView> {
    let entries = rrss_core::backup::list_auto(data_dir);
    if entries.is_empty() {
        return None;
    }
    let bytes = entries.iter().map(|e| e.bytes as i64).sum();
    Some(StorageGroupView {
        key: "backups".into(),
        label: "自动备份".into(),
        size: fmt_bytes(bytes),
        note: String::new(),
        items: vec![
            item(
                "auto_backups",
                "自动备份",
                format!(
                    "{} 份 · 只保留最近 {} 份",
                    entries.len(),
                    rrss_core::backup::AUTO_KEEP
                ),
                bytes,
            )
            .passive("在「备份与恢复」管理"),
        ],
    })
}

/// 运行日志在本机日志目录（Windows 上与数据目录分开），不进备份。没有初始化文件日志时不显示。
fn logs_group(log: Option<crate::views::LogStatusView>) -> Option<StorageGroupView> {
    use crate::views::LogUnavailableReason;
    let log = log?;
    let item = item(
        "logs",
        "运行日志",
        "用于排查问题，清空后继续记录".into(),
        log.total_bytes,
    );
    let item = if !log.can_clear {
        item.passive(match log.unavailable {
            Some(LogUnavailableReason::OtherInstance) => "另一个 RRSS 窗口正在记录",
            _ => "日志目录无法写入",
        })
    } else if log.total_bytes > 0 {
        item.action(
            "logs",
            "清空",
            "清空运行日志？",
            vec![
                "删除现有的日志文件，之后继续记录新的日志。".into(),
                "不影响订阅、文章和设置。".into(),
            ],
        )
    } else {
        item
    };
    Some(StorageGroupView {
        key: "logs".into(),
        label: "运行日志".into(),
        size: log.total_size,
        note: "保存在本机日志目录，不进备份。".into(),
        items: vec![item],
    })
}

fn wal_path(db_path: &Path) -> std::path::PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-wal");
    name.into()
}

/// 扫一遍库和数据目录，大库上需要几秒；只在打开存储页或点重新统计时调用。
pub fn storage_page(conn: &Connection, db_path: &Path) -> rrss_core::Result<StoragePage> {
    let data_dir = db_path.parent().unwrap_or(Path::new("."));
    let usage = storage::db_usage(conn)?;
    let media = storage::media_usage(conn, data_dir)?;
    let mut groups = vec![
        database_group(conn, db_path, &usage),
        media_group(&media),
        engine_group(data_dir),
    ];
    groups.extend(backups_group(data_dir));
    groups.extend(logs_group(
        crate::log::current().and_then(crate::log::Handle::status_view),
    ));
    let group_total = |key: &str| {
        groups
            .iter()
            .find(|g| g.key == key)
            .map(|g| g.items.iter().map(|i| i.bytes).sum::<i64>())
            .unwrap_or(0)
    };
    let db_total =
        storage::file_bytes(db_path) as i64 + storage::file_bytes(&wal_path(db_path)) as i64;
    let media_total = group_total("media");
    let engine_total = group_total("engine");
    let backups_total = group_total("backups");
    let logs_total = group_total("logs");
    Ok(StoragePage {
        data_dir: data_dir.display().to_string(),
        total: fmt_bytes(db_total + media_total + engine_total + backups_total + logs_total),
        summary: format!(
            "数据库 {} · 媒体缓存 {} · 对话引擎 {}{}{}",
            fmt_bytes(db_total),
            fmt_bytes(media_total),
            fmt_bytes(engine_total),
            if backups_total > 0 {
                format!(" · 自动备份 {}", fmt_bytes(backups_total))
            } else {
                String::new()
            },
            if logs_total > 0 {
                format!(" · 运行日志 {}", fmt_bytes(logs_total))
            } else {
                String::new()
            }
        ),
        groups,
        scanned_at: rrss_core::feeds::now_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_and_counts() {
        assert_eq!(fmt_bytes(0), "0");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(420 * 1024), "420 KB");
        assert_eq!(fmt_bytes(31 * 1024 * 1024 + 200_000), "31.2 MB");
        assert_eq!(fmt_bytes(612 * 1024 * 1024), "612 MB");
        assert_eq!(fmt_bytes(1_514_000_000), "1.41 GB");
        assert_eq!(fmt_count(38210), "38,210");
        assert_eq!(fmt_count(999), "999");
    }

    #[test]
    fn logs_group_offers_clearing_only_to_the_lock_owner() {
        use crate::views::{LogStatusView, LogUnavailableReason};
        let status = |can_clear, unavailable, total_bytes| LogStatusView {
            directory: "logs".into(),
            file: "logs/rrss.log".into(),
            unavailable,
            current_size: "1 KB".into(),
            total_size: "2 KB".into(),
            total_bytes,
            detailed: false,
            previous_unclean: None,
            can_clear,
        };
        assert!(logs_group(None).is_none());
        let owned = logs_group(Some(status(true, None, 2048))).unwrap();
        assert_eq!(
            (owned.key.as_str(), owned.items[0].action.as_str()),
            ("logs", "logs")
        );
        let other = logs_group(Some(status(
            false,
            Some(LogUnavailableReason::OtherInstance),
            2048,
        )))
        .unwrap();
        assert!(other.items[0].action.is_empty());
        assert!(other.items[0].passive.contains("另一个"));
        let empty = logs_group(Some(status(true, None, 0))).unwrap();
        assert!(empty.items[0].action.is_empty() && empty.items[0].passive.is_empty());
    }

    #[test]
    fn page_lists_groups_and_offers_only_available_actions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rrss.db");
        let conn = rrss_core::db::open_at(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO ai_cache (cache_key, value, created_at) VALUES ('k', 'v', 1);
             INSERT INTO feeds(id,url,created_at) VALUES(1,'https://test.invalid/rss',0);
             INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'article',0);
             INSERT INTO article_events(article_id,kind,at) VALUES(1,'open',0);
             CREATE TABLE feed_sub_records (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        let page = storage_page(&conn, &path).unwrap();
        let keys: Vec<&str> = page.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, ["database", "media", "engine"]);
        let db = &page.groups[0];
        let find = |key: &str| db.items.iter().find(|i| i.key == key).unwrap();
        assert_eq!(find("ai_cache").action, "ai_cache");
        assert!(find("ai_cache").confirm[0].contains("1 条"));
        assert_eq!(find("translations").action, "", "nothing to clear");
        assert_eq!(find("recommendation_logs").action, "recommendation_logs");
        assert!(find("recommendation_logs").hint.starts_with("1 条"));
        assert_eq!(find("vectors").action, "", "no embed model configured");
        assert_eq!(find("legacy").action, "legacy_tables");
        assert_eq!(find("search_index").passive, "随文章变化");
        assert!(db.items.last().unwrap().free, "free space stays last");
        let media = &page.groups[1];
        assert!(
            media.items.iter().all(|i| i.action.is_empty()),
            "empty caches have nothing to clear"
        );

        rrss_core::backup::create_auto(
            &conn,
            dir.path(),
            rrss_core::backup::BackupKind::Scheduled,
            "t",
        )
        .unwrap();
        let page = storage_page(&conn, &path).unwrap();
        let backups = page.groups.last().unwrap();
        assert_eq!(backups.key, "backups");
        assert!(backups.items[0].hint.starts_with("1 份"));
        assert!(page.summary.contains("自动备份"));
    }
}
