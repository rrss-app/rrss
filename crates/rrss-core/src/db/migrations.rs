use rusqlite::Connection;

use crate::error::{Error, Result};

pub const SCHEMA_VERSION: i32 = 47;

/// 低于此版本的库不能在当前程序里升级。
pub const MIN_UPGRADE_VERSION: i32 = 33;

const SCHEMA_SQL: &str = include_str!("../schema.sql");

const SETTINGS_PATCHES: &str = "
INSERT OR IGNORE INTO settings (key, value) VALUES ('pi_auto_download', '1');
INSERT OR IGNORE INTO settings (key, value) VALUES ('pi_auto_update', '0');
INSERT OR IGNORE INTO settings (key, value) VALUES ('pi_managed_version', '');
INSERT OR IGNORE INTO settings (key, value) VALUES ('translate_target_lang', 'auto');
INSERT OR IGNORE INTO settings (key, value) VALUES ('translate_backend', 'complete');
INSERT OR IGNORE INTO settings (key, value) VALUES ('translate_http_url', '');
INSERT OR IGNORE INTO settings (key, value) VALUES ('translate_http_key', '');
";

pub fn migrate(conn: &Connection) -> Result<()> {
    type Migration = fn(&Connection) -> Result<()>;
    const MIGRATIONS: &[(i32, Migration)] = &[
        (34, apply_v34),
        (35, apply_v35),
        (36, apply_v36),
        (37, apply_v37),
        (38, apply_v38),
        (39, apply_v39),
        (40, apply_v40),
        (41, apply_v41),
        (42, apply_v42),
        (43, apply_v43),
        (44, apply_v44),
        (45, apply_v45),
        (46, apply_v46),
        (47, apply_v47),
    ];
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(0..=SCHEMA_VERSION).contains(&version) {
        return Err(Error::UnsupportedSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == 0 {
        conn.execute_batch(SCHEMA_SQL)?;
        crate::ai_store::seed_defaults(conn)?;
        crate::pi_store::seed_default_tools(conn)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tracing::info!(from = version, to = SCHEMA_VERSION, "created schema");
    } else if version < MIN_UPGRADE_VERSION {
        return Err(Error::SchemaTooOld {
            found: version,
            min: MIN_UPGRADE_VERSION,
        });
    } else if version < SCHEMA_VERSION {
        for &(_, apply) in MIGRATIONS.iter().filter(|&&(next, _)| next > version) {
            apply(conn)?;
        }
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tracing::info!(from = version, to = SCHEMA_VERSION, "migrated schema");
    } else {
        tracing::debug!(version, "schema up to date");
    }
    apply_settings_patches(conn)?;
    ensure_digest_period_index(conn)?;
    // 补全新增 prompt / task_bindings（如 digest_pick_*）。
    crate::ai_store::seed_defaults(conn)?;
    Ok(())
}

fn apply_v47(conn: &Connection) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        if !column_exists(conn, "threads", "title_successful_turns")? {
            conn.execute_batch(
                "ALTER TABLE threads ADD COLUMN title_successful_turns INTEGER;
                 ALTER TABLE threads ADD COLUMN title_user_edited INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE threads ADD COLUMN title_request_token TEXT;",
            )?;
        }
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS chat_title_binding_revision AFTER UPDATE ON task_bindings
             WHEN new.task = 'chat_title' AND (old.provider_id IS NOT new.provider_id OR old.model IS NOT new.model) BEGIN
                 INSERT INTO settings (key, value) VALUES ('chat_title_binding_revision', lower(hex(randomblob(16))))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value;
             END;",
        )?;
        Ok(())
    })
}

fn apply_v46(conn: &Connection) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        conn.execute_batch(
            "CREATE VIEW IF NOT EXISTS model_memories AS
                SELECT * FROM memories
                WHERE scope = 'global' AND kind != 'episode'
                  AND COALESCE(source, '') NOT IN ('pending', 'rejected') AND strength > 0;
             CREATE TABLE IF NOT EXISTS memory_candidate_receipts (
                fingerprint TEXT PRIMARY KEY NOT NULL
             );
             CREATE TABLE IF NOT EXISTS memory_episode_progress (
                thread_id INTEGER PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
                through_message_id INTEGER NOT NULL DEFAULT 0,
                generation BLOB NOT NULL DEFAULT (randomblob(16))
             );
             CREATE TRIGGER IF NOT EXISTS memory_episode_thread_deleted AFTER DELETE ON threads BEGIN
                DELETE FROM memories
                WHERE kind = 'episode' AND scope = 'thread' AND scope_id = old.id;
             END;
             INSERT OR IGNORE INTO memory_episode_progress (thread_id, through_message_id)
                SELECT thread_id, MAX(id) FROM messages GROUP BY thread_id;",
        )?;
        crate::memories::remember_existing_facts(conn)
    })
}

// Upgrade databases with the original v44 triggers as well as fresh installs.
fn apply_v45(conn: &Connection) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        conn.execute_batch("DROP TRIGGER IF EXISTS chunk_preparation_ai;
            CREATE TRIGGER chunk_preparation_ai AFTER INSERT ON articles BEGIN
                INSERT INTO chunk_preparation(article_id) VALUES(new.id)
                ON CONFLICT(article_id) DO UPDATE SET revision=randomblob(16);
            END;
            DROP TRIGGER IF EXISTS revisions_articles_insert;
            CREATE TRIGGER revisions_articles_insert AFTER INSERT ON articles BEGIN
                UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster','rank');
                UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest'
                    AND EXISTS(SELECT 1 FROM chunks WHERE article_id=new.id AND embedding IS NOT NULL);
            END;")?;
        Ok(())
    })
}

fn apply_v44(conn: &Connection) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    tx.execute_batch(
        r#"-- Before dropping referenced columns or rebuilding tables, drop their dependent
-- preparation/revision triggers and recreate them with the migrated schema.
-- Durable work is invalidated with the article update, including external writers.
CREATE TABLE IF NOT EXISTS chunk_preparation (
    article_id INTEGER PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    revision BLOB NOT NULL DEFAULT (randomblob(16))
);
CREATE TRIGGER IF NOT EXISTS chunk_preparation_ai AFTER INSERT ON articles BEGIN
    INSERT INTO chunk_preparation(article_id) VALUES(new.id)
    ON CONFLICT(article_id) DO UPDATE SET revision=randomblob(16);
END;
CREATE TRIGGER IF NOT EXISTS chunk_preparation_au AFTER UPDATE OF title, summary, blocks_json, content_html,
    read, starred, embed_tier, pruned_at, expires_at ON articles
WHEN old.title IS NOT new.title OR old.summary IS NOT new.summary
    OR old.blocks_json IS NOT new.blocks_json OR old.content_html IS NOT new.content_html
    OR old.read IS NOT new.read OR old.starred IS NOT new.starred
    OR old.embed_tier IS NOT new.embed_tier OR old.pruned_at IS NOT new.pruned_at
    OR old.expires_at IS NOT new.expires_at
BEGIN
    INSERT INTO chunk_preparation(article_id) VALUES(new.id)
    ON CONFLICT(article_id) DO UPDATE SET revision=randomblob(16);
END;
INSERT OR IGNORE INTO chunk_preparation(article_id) SELECT id FROM articles;

CREATE TABLE IF NOT EXISTS media_cleanup (local_path TEXT PRIMARY KEY);
CREATE INDEX IF NOT EXISTS idx_media_local_path ON media(local_path);
CREATE TRIGGER IF NOT EXISTS media_cleanup_ad AFTER DELETE ON media
WHEN old.local_path IS NOT NULL AND old.local_path!='' BEGIN
    INSERT OR IGNORE INTO media_cleanup VALUES(old.local_path);
END;
CREATE TRIGGER IF NOT EXISTS media_cleanup_au AFTER UPDATE OF local_path ON media
WHEN old.local_path IS NOT NULL AND old.local_path!='' AND old.local_path IS NOT new.local_path BEGIN
    INSERT OR IGNORE INTO media_cleanup VALUES(old.local_path);
END;

CREATE TABLE IF NOT EXISTS computation_revisions (domain TEXT PRIMARY KEY, revision INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO computation_revisions(domain) VALUES('cluster'),('interest'),('rank');
CREATE TRIGGER IF NOT EXISTS revisions_articles_insert AFTER INSERT ON articles BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster','rank');
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest'
        AND EXISTS(SELECT 1 FROM chunks WHERE article_id=new.id AND embedding IS NOT NULL);
END;
CREATE TRIGGER IF NOT EXISTS revisions_articles_delete AFTER DELETE ON articles BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster','interest','rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_articles_cluster_update AFTER UPDATE OF feed_id,title,canonical_url,simhash,published_at,created_at,score,cluster_id,content_html,expires_at ON articles
WHEN old.feed_id IS NOT new.feed_id OR old.title IS NOT new.title OR old.canonical_url IS NOT new.canonical_url OR old.simhash IS NOT new.simhash OR old.published_at IS NOT new.published_at OR old.created_at IS NOT new.created_at OR old.score IS NOT new.score OR old.cluster_id IS NOT new.cluster_id OR old.content_html IS NOT new.content_html OR old.expires_at IS NOT new.expires_at BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='cluster';
END;
CREATE TRIGGER IF NOT EXISTS revisions_articles_interest_update AFTER UPDATE OF score,expires_at ON articles
WHEN old.score IS NOT new.score OR old.expires_at IS NOT new.expires_at BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest';
END;
CREATE TRIGGER IF NOT EXISTS revisions_articles_rank_update AFTER UPDATE OF feed_id,author,title,canonical_url,published_at,created_at,cluster_id,expires_at,read,starred ON articles
WHEN old.feed_id IS NOT new.feed_id OR old.author IS NOT new.author OR old.title IS NOT new.title OR old.canonical_url IS NOT new.canonical_url OR old.published_at IS NOT new.published_at OR old.created_at IS NOT new.created_at OR old.cluster_id IS NOT new.cluster_id OR old.expires_at IS NOT new.expires_at OR old.read IS NOT new.read OR old.starred IS NOT new.starred BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_chunks_insert AFTER INSERT ON chunks BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster','interest','rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_chunks_delete AFTER DELETE ON chunks BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster','interest','rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_chunks_cluster_update AFTER UPDATE OF article_id,ordinal,kind,embedding,space_id ON chunks
WHEN old.article_id IS NOT new.article_id OR old.ordinal IS NOT new.ordinal OR old.kind IS NOT new.kind OR old.embedding IS NOT new.embedding OR old.space_id IS NOT new.space_id BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='cluster';
END;
CREATE TRIGGER IF NOT EXISTS revisions_chunks_interest_update AFTER UPDATE OF article_id,ordinal,kind,embedding,space_id ON chunks
WHEN old.article_id IS NOT new.article_id OR old.ordinal IS NOT new.ordinal OR old.kind IS NOT new.kind OR old.embedding IS NOT new.embedding OR old.space_id IS NOT new.space_id BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest';
END;
CREATE TRIGGER IF NOT EXISTS revisions_chunks_rank_update AFTER UPDATE OF article_id,ordinal,kind,embedding,space_id ON chunks
WHEN old.article_id IS NOT new.article_id OR old.ordinal IS NOT new.ordinal OR old.kind IS NOT new.kind OR old.embedding IS NOT new.embedding OR old.space_id IS NOT new.space_id BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_clusters_insert AFTER INSERT ON clusters BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster');
END;
CREATE TRIGGER IF NOT EXISTS revisions_clusters_delete AFTER DELETE ON clusters BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster');
END;
CREATE TRIGGER IF NOT EXISTS revisions_clusters_cluster_update AFTER UPDATE OF label,representative_id,member_count,first_published_at,last_published_at,created_at ON clusters
WHEN old.label IS NOT new.label OR old.representative_id IS NOT new.representative_id OR old.member_count IS NOT new.member_count OR old.first_published_at IS NOT new.first_published_at OR old.last_published_at IS NOT new.last_published_at OR old.created_at IS NOT new.created_at BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='cluster';
END;
CREATE TRIGGER IF NOT EXISTS revisions_interests_insert AFTER INSERT ON interests BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('interest');
END;
CREATE TRIGGER IF NOT EXISTS revisions_interests_delete AFTER DELETE ON interests BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('interest');
END;
CREATE TRIGGER IF NOT EXISTS revisions_interests_interest_update AFTER UPDATE OF label,weight,enabled,embedding,space_id ON interests
WHEN old.label IS NOT new.label OR old.weight IS NOT new.weight OR old.enabled IS NOT new.enabled OR old.embedding IS NOT new.embedding OR old.space_id IS NOT new.space_id BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest';
END;
CREATE TRIGGER IF NOT EXISTS revisions_vector_spaces_insert AFTER INSERT ON vector_spaces BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster','interest','rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_vector_spaces_delete AFTER DELETE ON vector_spaces BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('cluster','interest','rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_vector_spaces_cluster_update AFTER UPDATE OF state,dims,source_dims ON vector_spaces
WHEN old.state IS NOT new.state OR old.dims IS NOT new.dims OR old.source_dims IS NOT new.source_dims BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='cluster';
END;
CREATE TRIGGER IF NOT EXISTS revisions_vector_spaces_interest_update AFTER UPDATE OF state,dims,source_dims ON vector_spaces
WHEN old.state IS NOT new.state OR old.dims IS NOT new.dims OR old.source_dims IS NOT new.source_dims BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest';
END;
CREATE TRIGGER IF NOT EXISTS revisions_vector_spaces_rank_update AFTER UPDATE OF state,dims,source_dims ON vector_spaces
WHEN old.state IS NOT new.state OR old.dims IS NOT new.dims OR old.source_dims IS NOT new.source_dims BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_feedback_insert AFTER INSERT ON feedback BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_feedback_delete AFTER DELETE ON feedback BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_feedback_rank_update AFTER UPDATE ON feedback BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_article_events_insert AFTER INSERT ON article_events BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_article_events_delete AFTER DELETE ON article_events BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_article_events_rank_update AFTER UPDATE ON article_events BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_read_sessions_insert AFTER INSERT ON read_sessions BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_read_sessions_delete AFTER DELETE ON read_sessions BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_read_sessions_rank_update AFTER UPDATE ON read_sessions BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_exposures_insert AFTER INSERT ON exposures BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_exposures_delete AFTER DELETE ON exposures BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_exposures_rank_update AFTER UPDATE ON exposures BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_article_tags_insert AFTER INSERT ON article_tags BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_article_tags_delete AFTER DELETE ON article_tags BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain IN ('rank');
END;
CREATE TRIGGER IF NOT EXISTS revisions_article_tags_rank_update AFTER UPDATE ON article_tags BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_interest_insert AFTER INSERT ON settings
WHEN new.key IN ('hide_threshold','interest_boost_enabled') BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_interest_update AFTER UPDATE ON settings
WHEN new.key IN ('hide_threshold','interest_boost_enabled') AND old.value IS NOT new.value BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_interest_delete AFTER DELETE ON settings
WHEN old.key IN ('hide_threshold','interest_boost_enabled') BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='interest';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_cluster_insert AFTER INSERT ON settings
WHEN new.key IN ('cluster_algorithm_version') BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='cluster';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_cluster_update AFTER UPDATE ON settings
WHEN new.key IN ('cluster_algorithm_version') AND old.value IS NOT new.value BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='cluster';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_cluster_delete AFTER DELETE ON settings
WHEN old.key IN ('cluster_algorithm_version') BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='cluster';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_rank_insert AFTER INSERT ON settings
WHEN new.key IN ('rank_features_enabled','rank_source_enabled','rank_coverage_enabled','rank_freshness_enabled','rank_affinity_enabled') BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_rank_update AFTER UPDATE ON settings
WHEN new.key IN ('rank_features_enabled','rank_source_enabled','rank_coverage_enabled','rank_freshness_enabled','rank_affinity_enabled') AND old.value IS NOT new.value BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
CREATE TRIGGER IF NOT EXISTS revisions_settings_rank_delete AFTER DELETE ON settings
WHEN old.key IN ('rank_features_enabled','rank_source_enabled','rank_coverage_enabled','rank_freshness_enabled','rank_affinity_enabled') BEGIN
    UPDATE computation_revisions SET revision=revision+1 WHERE domain='rank';
END;
"#,
    )?;
    tx.commit()?;
    Ok(())
}

fn apply_v38(conn: &Connection) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    if !column_exists(&tx, "mcp_servers", "auth")? {
        tx.execute_batch("ALTER TABLE mcp_servers ADD COLUMN auth TEXT NOT NULL DEFAULT 'none';
            UPDATE mcp_servers SET auth='token' WHERE headers_json IS NOT NULL AND headers_json NOT IN ('', '{}');")?;
    }
    tx.execute_batch("UPDATE mcp_servers SET enabled=0 WHERE transport='sse';")?;
    if column_exists(&tx, "mcp_servers", "lifecycle")? {
        tx.execute_batch("ALTER TABLE mcp_servers DROP COLUMN lifecycle;")?;
    }
    tx.commit()?;
    Ok(())
}

/// 相近报道综述按簇存库（AI 规格 · 相近报道综述）。
fn apply_v39(conn: &Connection) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    for (column, kind) in [
        ("summary_json", "TEXT"),
        ("summary_members", "TEXT"),
        ("summary_at", "INTEGER"),
    ] {
        if !column_exists(&tx, "clusters", column)? {
            tx.execute_batch(&format!("ALTER TABLE clusters ADD COLUMN {column} {kind};"))?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// 自动摘要与两份预算。
fn apply_v40(conn: &Connection) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    for column in ["embed_tokens", "auto_summaries", "auto_cluster_summaries"] {
        if !column_exists(&tx, "ai_usage", column)? {
            tx.execute_batch(&format!(
                "ALTER TABLE ai_usage ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0;"
            ))?;
        }
    }
    let budget_key = crate::ai_store::AI_DAILY_BUDGET_TOKENS_KEY;
    let kept = crate::db::get_setting(&tx, budget_key)?
        .and_then(|value| value.trim().parse::<i64>().ok())
        .is_some_and(|value| value > 0);
    // 旧 schema 给每个库预写 0；只有有效正数才视为需要保留的预算。
    if !kept {
        tx.execute("DELETE FROM settings WHERE key=?1", [budget_key])?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO settings(key,value) VALUES('focus_auto_summary_notice',?1)",
        [if kept { "kept" } else { "default" }],
    )?;
    tx.commit()?;
    Ok(())
}

/// 会话文件夹、归档与消息搜索。
fn apply_v41(conn: &Connection) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS thread_folders (
        id INTEGER PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL,
        collapsed INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);",
    )?;
    // 开启外键时，ALTER TABLE 新增 REFERENCES 列必须使用 NULL 默认值。
    if !column_exists(&tx, "threads", "folder_id")? {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN folder_id INTEGER
            REFERENCES thread_folders (id) ON DELETE SET NULL;",
        )?;
    }
    if !column_exists(&tx, "threads", "archived_at")? {
        tx.execute_batch(
            "ALTER TABLE threads ADD COLUMN archived_at INTEGER;
            UPDATE threads SET title=NULL WHERE anchor_kind IS NULL AND title='新对话';",
        )?;
    }
    tx.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5 (
            content,
            tokenize = 'trigram'
        );

        CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages
        WHEN new.role IN ('user', 'assistant') BEGIN
            INSERT INTO messages_fts (rowid, content) VALUES (new.id, new.content);
        END;

        CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
            DELETE FROM messages_fts WHERE rowid = old.id;
        END;

        CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE OF id, content, role ON messages BEGIN
            DELETE FROM messages_fts WHERE rowid = old.id;
            INSERT INTO messages_fts (rowid, content)
            SELECT new.id, new.content WHERE new.role IN ('user', 'assistant');
        END;
        DELETE FROM messages_fts;
        INSERT INTO messages_fts (rowid, content)
        SELECT id, content FROM messages WHERE role IN ('user', 'assistant');",
    )?;
    tx.commit()?;
    Ok(())
}

fn apply_v42(conn: &Connection) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    tx.execute_batch(
        "DELETE FROM task_bindings WHERE task = 'cite_summarize';
         DELETE FROM prompt_templates WHERE slug = 'cite-summarize';",
    )?;
    // Cache keys are hashes and usage is aggregated by day, so neither identifies a task.
    tx.commit()?;
    Ok(())
}

/// 翻译目标语言改为默认跟随界面语言。旧库都存着设置补丁写入的 `zh`，分不出是不是用户选的；
/// 在这之前界面只有中文，`zh` 与跟随界面效果相同，所以一并改成 `auto`，其他语言保留。
fn apply_v43(conn: &Connection) -> Result<()> {
    let tx = crate::db::write_transaction(conn)?;
    tx.execute(
        "UPDATE settings SET value = ?1 WHERE key = ?2 AND trim(lower(value)) IN ('zh', '')",
        [
            crate::translations::TARGET_FOLLOW_UI,
            crate::translations::TRANSLATE_TARGET_LANG_KEY,
        ],
    )?;
    tx.commit()?;
    Ok(())
}

fn apply_settings_patches(conn: &Connection) -> Result<()> {
    conn.execute_batch(SETTINGS_PATCHES)?;
    Ok(())
}

/// D1 UPSERT 依赖 `(period, period_start)` 唯一索引；v0 直建 schema 或早期库可能缺索引。
fn ensure_digest_period_index(conn: &Connection) -> Result<()> {
    let has_table: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'digests'",
        [],
        |row| row.get(0),
    )?;
    if has_table == 0 {
        return Ok(());
    }
    conn.execute_batch(
        "DELETE FROM digests
         WHERE id NOT IN (
             SELECT MAX(id) FROM digests GROUP BY period, period_start
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_digests_period_start
           ON digests (period, period_start);",
    )?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn apply_v35(conn: &Connection) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        if !column_exists(conn, "vector_spaces", "source_dims")? {
            conn.execute_batch(
                "ALTER TABLE vector_spaces ADD COLUMN source_dims INTEGER NOT NULL DEFAULT 0;
                UPDATE vector_spaces SET source_dims=dims;",
            )?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO settings(key,value)
             SELECT 'embedding_dims',CASE WHEN EXISTS(
                 SELECT 1 FROM vector_spaces WHERE rebuild_from IS NULL
                 AND state IN ('active','stale') AND dims>1024
             ) THEN 'native' ELSE 'auto' END",
            [],
        )?;
        Ok(())
    })
}

fn apply_v36(conn: &Connection) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        if !column_exists(conn, "feeds", "position")? {
            conn.execute_batch(
                "ALTER TABLE feeds ADD COLUMN position INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        // 升级前的显示顺序：同一父级先子文件夹（position, id），再源（id）。
        let folders: Vec<(i64, Option<i64>)> = conn
            .prepare("SELECT id, parent_id FROM folders ORDER BY position, id")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let feeds: Vec<(i64, Option<i64>)> = conn
            .prepare("SELECT id, folder_id FROM feeds ORDER BY id")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut groups: std::collections::BTreeMap<Option<i64>, Vec<(bool, i64)>> =
            std::collections::BTreeMap::new();
        for (id, parent) in folders {
            groups.entry(parent).or_default().push((true, id));
        }
        for (id, parent) in feeds {
            groups.entry(parent).or_default().push((false, id));
        }
        for items in groups.values() {
            for (position, (is_folder, id)) in items.iter().enumerate() {
                let sql = if *is_folder {
                    "UPDATE folders SET position = ?1 WHERE id = ?2"
                } else {
                    "UPDATE feeds SET position = ?1 WHERE id = ?2"
                };
                conn.execute(sql, rusqlite::params![position as i64, id])?;
            }
        }
        Ok(())
    })
}

fn apply_v37(conn: &Connection) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        if !column_exists(conn, "feeds", "next_fetch_at")? {
            // SQLite promotes overflowing integer arithmetic to REAL; casting
            // back to INTEGER saturates instead of wrapping the UTC deadline.
            conn.execute_batch(
                "ALTER TABLE feeds ADD COLUMN next_fetch_at INTEGER NOT NULL DEFAULT 0;
                 UPDATE feeds SET next_fetch_at = CASE WHEN last_fetched_at IS NULL THEN 0
                     ELSE CAST(last_fetched_at + max(interval_secs, 0) * 1000 AS INTEGER) END;",
            )?;
        }
        if !column_exists(conn, "feeds", "slow_fail_count")? {
            conn.execute_batch(
                "ALTER TABLE feeds ADD COLUMN slow_fail_count INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_feeds_due ON feeds(next_fetch_at, id) WHERE kind!='manual';",
        )?;
        Ok(())
    })
}

fn apply_v34(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "articles", "embed_tier")? {
        let tx = crate::db::write_transaction(conn)?;
        tx.execute_batch("ALTER TABLE articles ADD COLUMN embed_tier INTEGER NOT NULL DEFAULT 0;")?;
        tx.execute_batch(
            "UPDATE articles SET embed_tier = CASE
                WHEN EXISTS (SELECT 1 FROM chunks WHERE chunks.article_id = articles.id AND chunks.ordinal > 0) THEN 2
                WHEN EXISTS (SELECT 1 FROM chunks WHERE chunks.article_id = articles.id AND chunks.ordinal = 0) THEN 1
                ELSE 0
             END;",
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v47_keeps_existing_titles_and_only_counts_new_threads() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE task_bindings (task TEXT PRIMARY KEY, provider_id INTEGER, model TEXT);
             CREATE TABLE threads (id INTEGER PRIMARY KEY, title TEXT);
             INSERT INTO threads(id, title) VALUES (1, 'Local title'), (2, NULL);",
        )
        .unwrap();
        apply_v47(&conn).unwrap();
        let old: (String, Option<i64>) = conn
            .query_row(
                "SELECT title, title_successful_turns FROM threads WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(old, ("Local title".into(), None));
        let (_dir, fresh) = {
            let dir = tempfile::tempdir().unwrap();
            let fresh = crate::db::open_at(&dir.path().join("fresh.db")).unwrap();
            (dir, fresh)
        };
        let thread = crate::threads::create_thread(&fresh, None).unwrap();
        assert_eq!(
            fresh
                .query_row(
                    "SELECT title_successful_turns FROM threads WHERE id=?1",
                    [thread.id],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let binding =
            crate::ai_store::get_task_binding(&fresh, crate::ai_store::TASK_CHAT_TITLE).unwrap();
        assert_eq!(binding.provider_id, None);
        assert_eq!(binding.model, None);
    }

    #[test]
    fn v46_keeps_unattributed_episodes_but_never_replays_old_messages() {
        use crate::{memories, threads};

        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("legacy-memory.db")).unwrap();
        conn.execute_batch(
            "DROP VIEW model_memories;
             DROP TABLE memory_candidate_receipts;
             DROP TABLE memory_episode_progress;
             DROP TRIGGER memory_episode_thread_deleted;
             INSERT INTO threads(id,pi_session_file,created_at,updated_at) VALUES(7,'old.jsonl',0,0);
             WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM ids WHERE id<30)
                 INSERT INTO messages(id,thread_id,role,content,created_at)
                 SELECT id,7,CASE WHEN id%2=0 THEN 'assistant' ELSE 'user' END,'legacy',0 FROM ids;
             INSERT INTO memories(kind,content,pinned,source,strength,created_at) VALUES
                 ('episode','Unattributed private episode',1,'compress',1,0),
                 ('semantic','I prefer offline reading.',0,'pending',0,0);
             PRAGMA user_version=45;",
        ).unwrap();
        migrate(&conn).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(memories::list_memories(&conn, "").unwrap().len(), 2);
        assert!(memories::inject_prefix(&conn).unwrap().is_empty());
        assert!(
            memories::episode_context(&conn, "old.jsonl")
                .unwrap()
                .is_empty()
        );
        assert!(
            memories::search_memory(&conn, "Unattributed", 20)
                .unwrap()
                .is_empty()
        );
        assert!(memories::episode_batch(&conn, 7, 30).unwrap().is_none());
        let pending = memories::list_memories(&conn, "offline").unwrap()[0].id;
        memories::delete_memory(&conn, pending).unwrap();
        assert!(
            memories::insert_new_candidates(&conn, vec!["I prefer offline reading".into()])
                .unwrap()
                .is_empty()
        );

        let mut latest = 30;
        for _ in 0..10 {
            for role in [threads::MessageRole::User, threads::MessageRole::Assistant] {
                latest = threads::insert_message(&conn, 7, role, "since upgrade", None)
                    .unwrap()
                    .id;
            }
        }
        let batch = memories::episode_batch(&conn, 7, latest).unwrap().unwrap();
        assert!(batch.lines.iter().all(|(_, text)| text == "since upgrade"));
    }

    #[test]
    fn v43_moves_the_default_chinese_target_to_follow_the_interface() {
        use crate::translations::{TARGET_FOLLOW_UI, TRANSLATE_TARGET_LANG_KEY, target_lang};
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("target.db")).unwrap();
        assert_eq!(
            target_lang(&conn).unwrap(),
            TARGET_FOLLOW_UI,
            "new databases follow the interface"
        );
        for (stored, expected) in [("zh", TARGET_FOLLOW_UI), ("ja", "ja"), ("en", "en")] {
            crate::db::set_setting(&conn, TRANSLATE_TARGET_LANG_KEY, stored).unwrap();
            conn.execute_batch("PRAGMA user_version=42;").unwrap();
            migrate(&conn).unwrap();
            assert_eq!(target_lang(&conn).unwrap(), expected, "stored {stored}");
        }
        // Choosing Chinese after the upgrade is kept.
        crate::db::set_setting(&conn, TRANSLATE_TARGET_LANG_KEY, "zh").unwrap();
        migrate(&conn).unwrap();
        assert_eq!(target_lang(&conn).unwrap(), "zh");
    }

    #[test]
    fn v42_removes_retired_citation_task_without_disturbing_active_tasks_or_usage() {
        use crate::{ai_store, model_pool};

        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("retired-task.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO ai_providers(id,name,base_url,created_at)
                 VALUES(99,'Fixture','https://model.example/v1',0);
             INSERT INTO ai_model_pool(provider_id,model,created_at)
                 VALUES(99,'retired-model',0),(99,'active-model',0);
             INSERT INTO prompt_templates(slug,title,body)
                 VALUES('cite-summarize','Retired template','Retired prompt');
             INSERT INTO task_bindings(task,provider_id,template_id,model)
                 SELECT 'cite_summarize',99,id,'retired-model'
                 FROM prompt_templates WHERE slug='cite-summarize';
             UPDATE task_bindings SET provider_id=99,model='active-model'
                 WHERE task='article_summary';
             INSERT INTO ai_usage(day,tokens,prompt_tokens,completion_tokens)
                 VALUES('2026-11-01',30,20,10);
             PRAGMA user_version=41;",
        )
        .unwrap();
        let cache_key = ai_store::cache_key("cite_summarize", "retired-model", 1, "fixture");
        ai_store::cache_put(&conn, &cache_key, "cached summary").unwrap();
        let active_template = ai_store::get_template_by_slug(&conn, "article-summary").unwrap();
        migrate(&conn).unwrap();
        // Reopening must not seed the retired task again.
        migrate(&conn).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        let bindings = ai_store::list_task_bindings(&conn).unwrap();
        assert_eq!(bindings.len(), ai_store::ALL_TASKS.len());
        assert!(
            bindings
                .iter()
                .all(|b| ai_store::ALL_TASKS.contains(&b.task.as_str()))
        );
        assert!(matches!(
            ai_store::get_template_by_slug(&conn, "cite-summarize"),
            Err(Error::TemplateNotFound(_))
        ));
        let active = ai_store::resolve_task(&conn, ai_store::TASK_ARTICLE_SUMMARY).unwrap();
        assert_eq!(active.provider.id, 99);
        assert_eq!(active.model, "active-model");
        assert_eq!(active.template.id, active_template.id);
        assert_eq!(active.template.body, active_template.body);
        assert_eq!(active.template.version, active_template.version);
        assert_eq!(
            ai_store::cache_get(&conn, &cache_key).unwrap().as_deref(),
            Some("cached summary")
        );
        assert_eq!(ai_store::usage_on(&conn, "2026-11-01").unwrap().tokens, 30);
        // The retired binding used to make model removal fail with UnknownAiTask.
        let removed = model_pool::update_pool(
            &conn,
            &[],
            &[model_pool::ModelRef::new(99, "retired-model")],
            None,
        )
        .unwrap();
        assert!(removed.unset.is_empty());
        assert_eq!(model_pool::pool(&conn).unwrap().len(), 1);
    }

    #[test]
    fn v40_preserves_usage_and_positive_budgets_and_marks_only_upgrades() {
        for (budget, kept) in [
            (None, None),
            (Some("0"), None),
            (Some(""), None),
            (Some("   "), None),
            (Some("42000invalid"), None),
            (Some("-1"), None),
            (Some("9223372036854775808"), None),
            (Some("42000"), Some("42000")),
            (Some(" 42000 "), Some(" 42000 ")),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let conn = crate::db::open_at(&dir.path().join("budget.db")).unwrap();
            assert_eq!(
                crate::db::get_setting(&conn, "focus_auto_summary_notice").unwrap(),
                None
            );
            if let Some(value) = budget {
                crate::db::set_setting(&conn, "ai_daily_budget_tokens", value).unwrap();
            }
            conn.execute_batch(
                "ALTER TABLE ai_usage DROP COLUMN embed_tokens;
                ALTER TABLE ai_usage DROP COLUMN auto_summaries;
                ALTER TABLE ai_usage DROP COLUMN auto_cluster_summaries;
                INSERT INTO ai_usage(day,tokens,prompt_tokens,completion_tokens)
                    VALUES('2026-10-29',30,20,10);
                PRAGMA user_version=39;",
            )
            .unwrap();
            migrate(&conn).unwrap();
            for column in ["embed_tokens", "auto_summaries", "auto_cluster_summaries"] {
                assert!(column_exists(&conn, "ai_usage", column).unwrap());
                let count: i64 = conn
                    .query_row(&format!("SELECT {column} FROM ai_usage"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 0);
            }
            assert_eq!(
                crate::ai_store::usage_on(&conn, "2026-10-29")
                    .unwrap()
                    .tokens,
                30
            );
            assert_eq!(
                crate::db::get_setting(&conn, "ai_daily_budget_tokens")
                    .unwrap()
                    .as_deref(),
                kept
            );
            assert_eq!(
                crate::ai_store::daily_budget_tokens(&conn).unwrap(),
                kept.map_or(200000, |v| v.trim().parse().unwrap())
            );
            assert_eq!(
                crate::db::get_setting(&conn, "focus_auto_summary_notice")
                    .unwrap()
                    .as_deref(),
                Some(if kept.is_some() { "kept" } else { "default" })
            );
            crate::db::set_setting(&conn, "focus_auto_summary_notice", "done").unwrap();
            migrate(&conn).unwrap();
            assert_eq!(
                crate::db::get_setting(&conn, "focus_auto_summary_notice")
                    .unwrap()
                    .as_deref(),
                Some("done")
            );
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                    .unwrap(),
                SCHEMA_VERSION
            );
        }
    }

    #[test]
    fn v41_backfills_message_search_and_tracks_content_role_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("search.db")).unwrap();
        conn.execute_batch(
            "DROP TRIGGER messages_ai; DROP TRIGGER messages_ad; DROP TRIGGER messages_au;
            DROP TABLE messages_fts;
            INSERT INTO threads(id,created_at,updated_at) VALUES(1,0,0);
            INSERT INTO messages(id,thread_id,role,content,created_at) VALUES
                (1,1,'user','Old library question',0),
                (2,1,'assistant','旧的图书馆回答',0),
                (3,1,'system','system library instruction',0);
            PRAGMA user_version=39;",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let matches = |query: &str| -> Vec<i64> {
            conn.prepare(
                "SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1 ORDER BY rowid",
            )
            .unwrap()
            .query_map([query], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
        };
        assert_eq!(matches("library"), [1]);
        assert_eq!(matches("图书馆"), [2]);
        conn.execute(
            "UPDATE messages SET content='Updated archive reply' WHERE id=2",
            [],
        )
        .unwrap();
        assert!(matches("图书馆").is_empty());
        assert_eq!(matches("archive"), [2]);
        conn.execute("UPDATE messages SET role='system' WHERE id=1", [])
            .unwrap();
        assert!(matches("library").is_empty());
        conn.execute("UPDATE messages SET role='assistant' WHERE id=3", [])
            .unwrap();
        assert_eq!(matches("library"), [3]);
        conn.execute("DELETE FROM messages WHERE id=2", []).unwrap();
        assert!(matches("archive").is_empty());
        conn.execute("DELETE FROM threads WHERE id=1", []).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages_fts", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn v41_preserves_threads_and_adds_nullable_folder_foreign_key() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("v39.db")).unwrap();
        conn.execute_batch(
            "ALTER TABLE threads DROP COLUMN folder_id;
            ALTER TABLE threads DROP COLUMN archived_at;
            DROP TABLE thread_folders;
            INSERT INTO threads(id,title,created_at,updated_at) VALUES(7,'保留',1,2),(8,'新对话',1,2);
            INSERT INTO messages(thread_id,role,content,created_at) VALUES(7,'user','消息',1);
            PRAGMA user_version=39;",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let thread = crate::threads::get_thread(&conn, 7).unwrap();
        assert_eq!(thread.title.as_deref(), Some("保留"));
        assert_eq!((thread.folder_id, thread.archived_at), (None, None));
        assert_eq!(crate::threads::get_thread(&conn, 8).unwrap().title, None);
        assert_eq!(crate::threads::list_messages(&conn, 7).unwrap().len(), 1);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            conn.pragma_query_value(None, "foreign_keys", |r| r.get::<_, i32>(0))
                .unwrap(),
            1
        );
        assert!(
            conn.execute("UPDATE threads SET folder_id=999 WHERE id=7", [])
                .is_err()
        );
        conn.execute_batch(
            "INSERT INTO thread_folders(id,name,position,created_at) VALUES(1,'目录',0,0);
            UPDATE threads SET folder_id=1 WHERE id=7;
            DELETE FROM thread_folders WHERE id=1;",
        )
        .unwrap();
        assert_eq!(
            crate::threads::get_thread(&conn, 7).unwrap().folder_id,
            None
        );
        migrate(&conn).unwrap();
    }

    #[test]
    fn v39_adds_cluster_summary_columns_and_keeps_clusters() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("cluster-summary.db")).unwrap();
        conn.execute_batch(
            "ALTER TABLE clusters DROP COLUMN summary_json;
             ALTER TABLE clusters DROP COLUMN summary_members;
             ALTER TABLE clusters DROP COLUMN summary_at;
             INSERT INTO clusters(id,label,member_count,created_at) VALUES(7,'同题',2,0);
             PRAGMA user_version=38;",
        )
        .unwrap();
        migrate(&conn).unwrap();
        for column in ["summary_json", "summary_members", "summary_at"] {
            assert!(column_exists(&conn, "clusters", column).unwrap());
        }
        let label: String = conn
            .query_row("SELECT label FROM clusters WHERE id=7", [], |r| r.get(0))
            .unwrap();
        assert_eq!(label, "同题");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn v38_preserves_legacy_services_disables_sse_and_removes_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("mcp.db")).unwrap();
        conn.execute_batch(
            r#"ALTER TABLE mcp_servers DROP COLUMN auth;
            ALTER TABLE mcp_servers ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'lazy';
            INSERT INTO mcp_servers(name,transport,enabled,headers_json,created_at) VALUES
                ('old','sse',1,'{}',0),('remote','http',1,'{"Authorization":"test-value"}',0);
            PRAGMA user_version=37;"#,
        )
        .unwrap();
        migrate(&conn).unwrap();
        let servers = crate::pi_store::list_mcp_servers(&conn).unwrap();
        assert!(!servers[0].enabled);
        assert_eq!(servers[0].auth, "none");
        assert!(servers[1].enabled);
        assert_eq!(servers[1].auth, "token");
        assert!(!column_exists(&conn, "mcp_servers", "lifecycle").unwrap());
        assert!(crate::pi_store::set_mcp_server_enabled(&conn, servers[0].id, true).is_err());
    }

    #[test]
    fn v37_backfills_due_dates_and_keeps_never_fetched_immediately_due() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("due-upgrade.db")).unwrap();
        conn.execute_batch(
            "DROP INDEX idx_feeds_due;
             ALTER TABLE feeds DROP COLUMN next_fetch_at;
             ALTER TABLE feeds DROP COLUMN slow_fail_count;
             PRAGMA user_version=36;
             INSERT INTO feeds(id,url,created_at,last_fetched_at,interval_secs)
             VALUES(1,'https://new.test/feed',0,NULL,3600),
                   (2,'https://old.test/feed',0,12345,3600),
                   (3,'https://edge.test/feed',0,9223372036854775800,3600);",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let due: Vec<(i64, i64)> = conn
            .prepare("SELECT next_fetch_at,slow_fail_count FROM feeds ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(due, [(0, 0), (3_612_345, 0), (i64::MAX, 0)]);
        assert_eq!(crate::ingest::due_feeds(&conn, 3_612_344).unwrap().len(), 1);
        assert_eq!(crate::ingest::due_feeds(&conn, 3_612_345).unwrap().len(), 2);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn v35_source_dims_backfill_preserves_stored_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("dims-upgrade.db")).unwrap();
        conn.execute_batch("INSERT INTO vector_spaces(base_url,model,dims,prefix_version,normalized,state,created_at)
            VALUES('','a',3072,1,0,'active',0),('','b',768,1,0,'retired',0);
            DROP TRIGGER revisions_vector_spaces_cluster_update;
            DROP TRIGGER revisions_vector_spaces_interest_update;
            DROP TRIGGER revisions_vector_spaces_rank_update;
            ALTER TABLE vector_spaces DROP COLUMN source_dims;
            DELETE FROM settings WHERE key='embedding_dims';
            PRAGMA user_version=34;").unwrap();
        migrate(&conn).unwrap();
        let rows: Vec<(i64, i64)> = conn
            .prepare("SELECT dims,source_dims FROM vector_spaces ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, [(3072, 3072), (768, 768)]);
        assert_eq!(crate::vector_space::dims_setting(&conn).unwrap(), "native");
        conn.execute(
            "UPDATE vector_spaces SET dims=1024 WHERE state='active'",
            [],
        )
        .unwrap();
        apply_v35(&conn).unwrap();
        assert_eq!(crate::vector_space::dims_setting(&conn).unwrap(), "native");
        assert_eq!(
            conn.query_row(
                "SELECT source_dims FROM vector_spaces WHERE state='active'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            3072
        );
    }

    #[test]
    fn v34_backfills_unsynced_title_and_body_tiers_without_changing_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("v33.db")).unwrap();
        conn.execute_batch(
            "DROP TRIGGER chunk_preparation_au;
             ALTER TABLE articles DROP COLUMN embed_tier;
             INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid/feed',0);
             INSERT INTO articles(id,feed_id,title,created_at) VALUES
                (1,1,'no chunks',0),(2,1,'title only',0),(3,1,'body too',0);
             INSERT INTO chunks(article_id,ordinal,text,embedding) VALUES
                (2,0,'title',X'0000803f'),(3,0,'title',X'0000803f'),(3,1,'body',X'00000040');
             PRAGMA user_version=33;",
        )
        .unwrap();
        migrate(&conn).unwrap();
        let tiers: Vec<i64> = conn
            .prepare("SELECT embed_tier FROM articles ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(tiers, [0, 1, 2]);
        assert_eq!(
            conn.query_row(
                "SELECT group_concat(hex(embedding)) FROM
                 (SELECT embedding FROM chunks ORDER BY article_id, ordinal)",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "0000803F,0000803F,00000040"
        );
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }
}
