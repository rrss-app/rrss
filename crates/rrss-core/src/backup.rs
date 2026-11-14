//! 完整备份与恢复：一个 zip 装库副本、Pi 会话、用户技能和清单。
//!
//! 备份在运行中用 `VACUUM INTO` 取一致副本。恢复分两步：运行中只解压到暂存目录并检查，
//! 确认后写待办文件并重启；下次启动、打开数据库之前再换文件，失败时把原文件移回。
//! 自动备份放在 `<data_dir>/backups/`，只留最近 [`AUTO_KEEP`] 份；手动备份由用户选位置，不在此管理。

use crate::error::backup::BackupProblem;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::db::{MIN_UPGRADE_VERSION, SCHEMA_VERSION};
use crate::error::{Error, Result};

/// 清单格式版本；读到更高的版本拒绝恢复。
pub const FORMAT: u32 = 2;
/// 自动备份保留份数。
pub const AUTO_KEEP: usize = 5;

const DB_ENTRY: &str = "rrss.db";
const MANIFEST_ENTRY: &str = "manifest.json";
const SESSIONS: &str = "pi/sessions";
const SKILLS: &str = "pi/user-skills";
/// 阶段 18 P2a 之前的用户技能位置；旧备份与未迁移的数据目录仍可能用它。
const LEGACY_SKILLS: &str = "pi-project/.pi/skills";
const STAGING: &str = "restore-staging";
const PENDING: &str = "restore-pending.json";
const RESULT: &str = "restore-result.json";
const OLD: &str = "restore-old";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    Manual,
    /// 启动时发现库版本低于程序，迁移前备份。
    Upgrade,
    Scheduled,
    /// 恢复前对当前数据的备份，可以再恢复回去。
    BeforeRestore,
    /// 第一次执行保留清理前。
    BeforePrune,
}

impl BackupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Upgrade => "upgrade",
            Self::Scheduled => "scheduled",
            Self::BeforeRestore => "before-restore",
            Self::BeforePrune => "before-prune",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub articles: i64,
    pub feeds: i64,
    pub rules: i64,
    pub threads: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub app_version: String,
    /// 备份时库的 `user_version`。
    pub schema_version: i32,
    pub created_at: i64,
    pub kind: BackupKind,
    pub counts: Counts,
    // 旧清单没有此项，旧版完整备份包含密钥。
    #[serde(default = "legacy_secrets_included")]
    pub secrets_included: bool,
}

fn legacy_secrets_included() -> bool {
    true
}

/// 各主要表的行数；缺表（老库）按 0 计。
pub fn counts(conn: &Connection) -> Result<Counts> {
    Ok(Counts {
        articles: count_rows(conn, "articles")?,
        feeds: count_rows(conn, "feeds")?,
        rules: count_rows(conn, "rules")?,
        threads: count_rows(conn, "threads")?,
    })
}

fn count_rows(conn: &Connection, table: &str) -> Result<i64> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
        [table],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(0);
    }
    Ok(
        conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
            r.get(0)
        })?,
    )
}

fn user_version(conn: &Connection) -> Result<i32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// 生成完整备份到 `dest`。先写同目录的临时文件，完成后再改名，中途失败不留半个文件。
pub fn create(
    conn: &Connection,
    data_dir: &Path,
    dest: &Path,
    kind: BackupKind,
    app_version: &str,
    include_secrets: bool,
) -> Result<Manifest> {
    let dir = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir)?;
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let copy = TempFile(dir.join(format!(".rrss-backup-{tag}.db")));
    let partial = TempFile(dir.join(format!(".rrss-backup-{tag}.partial")));

    let copy_path = copy
        .0
        .to_str()
        .ok_or_else(|| Error::Backup(BackupProblem::PathEncoding))?;
    conn.execute("VACUUM INTO ?1", [copy_path])?;
    // 自动路径无论调用参数如何都不允许带出密钥。
    let secrets_included = include_secrets && kind == BackupKind::Manual;
    let manifest = {
        let snap = Connection::open(&copy.0)?;
        if !secrets_included {
            strip_secrets(&snap)?;
        }
        Manifest {
            format: FORMAT,
            app_version: app_version.to_string(),
            schema_version: user_version(&snap)?,
            created_at: crate::feeds::now_ms(),
            kind,
            counts: counts(&snap)?,
            secrets_included,
        }
    };

    let mut zip = zip::ZipWriter::new(File::create(&partial.0)?);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    zip.start_file(MANIFEST_ENTRY, opts).map_err(zip_err)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(json_err)?)?;
    zip.start_file(DB_ENTRY, opts).map_err(zip_err)?;
    io::copy(&mut File::open(&copy.0)?, &mut zip)?;
    add_tree(&mut zip, &data_dir.join(SESSIONS), SESSIONS, opts)?;
    for (name, path) in user_skills(data_dir) {
        add_tree(&mut zip, &path, &format!("{SKILLS}/{name}"), opts)?;
    }
    zip.finish().map_err(zip_err)?.sync_all()?;
    fs::rename(&partial.0, dest)?;
    Ok(manifest)
}

// 只改 VACUUM INTO 生成的副本；旧库缺少新字段时跳过，绝不为备份迁移原库。
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(names.iter().any(|n| n == column))
}

fn strip_secrets(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "secure_delete", true)?;
    let tx = conn.unchecked_transaction()?;
    if has_column(&tx, "ai_providers", "api_key")? {
        tx.execute(
            "UPDATE ai_providers SET api_key = '' WHERE api_key IS NOT NULL",
            [],
        )?;
    }
    if has_column(&tx, "settings", "key")? {
        crate::network::strip_backup_auth(&tx)?;
        if crate::translations::deepl_uses_legacy_key(&tx)?
            && crate::translations::http_key(&tx)?.is_some()
        {
            crate::db::set_setting(
                &tx,
                &crate::translations::missing_key(crate::translations::TRANSLATE_DEEPL_KEY_KEY),
                "1",
            )?;
        }
        for key in crate::translations::SECRET_KEYS {
            if crate::db::get_setting(&tx, key)?.is_some_and(|v| !v.is_empty()) {
                crate::db::set_setting(&tx, &crate::translations::missing_key(key), "1")?;
                crate::db::set_setting(&tx, key, "")?;
            }
        }
    }
    for column in ["env_json", "headers_json"] {
        if !has_column(&tx, "mcp_servers", column)? {
            continue;
        }
        let rows = {
            let mut stmt = tx.prepare(&format!(
                "SELECT id, {column} FROM mcp_servers WHERE {column} IS NOT NULL"
            ))?;
            stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (id, raw) in rows {
            let mut values: std::collections::BTreeMap<String, String> = serde_json::from_str(&raw)
                .map_err(|_| Error::Backup(BackupProblem::SecretStripping))?;
            for value in values.values_mut() {
                value.clear();
            }
            let clean = serde_json::to_string(&values).map_err(json_err)?;
            tx.execute(
                &format!("UPDATE mcp_servers SET {column} = ?1 WHERE id = ?2"),
                rusqlite::params![clean, id],
            )?;
        }
    }
    tx.commit()?;
    // 重建副本，避免被清空的值仍残留在空闲页 / 单元中；关闭连接后才打包。
    conn.execute_batch("VACUUM")?;
    Ok(())
}

/// 自动备份目录。
pub fn auto_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

/// 在自动备份目录生成一份，并只留最近 [`AUTO_KEEP`] 份。
pub fn create_auto(
    conn: &Connection,
    data_dir: &Path,
    kind: BackupKind,
    app_version: &str,
) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
    let dest = auto_dir(data_dir).join(format!("rrss-{}-{stamp}.zip", kind.as_str()));
    create(conn, data_dir, &dest, kind, app_version, false)?;
    prune_auto(data_dir, AUTO_KEEP)?;
    Ok(dest)
}

#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub path: PathBuf,
    pub bytes: u64,
    /// 读不出清单的文件仍列出，但不能恢复。
    pub manifest: Option<Manifest>,
    pub modified_ms: i64,
}

/// 自动备份，新的在前。
pub fn list_auto(data_dir: &Path) -> Vec<BackupEntry> {
    let Ok(entries) = fs::read_dir(auto_dir(data_dir)) else {
        return Vec::new();
    };
    let mut out: Vec<BackupEntry> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "zip"))
        .map(|path| {
            let meta = fs::metadata(&path).ok();
            let modified_ms = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            BackupEntry {
                bytes: meta.map(|m| m.len()).unwrap_or(0),
                manifest: read_manifest(&path).ok(),
                modified_ms,
                path,
            }
        })
        .collect();
    out.sort_by_key(|e| {
        std::cmp::Reverse(e.manifest.as_ref().map_or(e.modified_ms, |m| m.created_at))
    });
    out
}

/// 删除较旧的自动备份，返回删掉的份数。
pub fn prune_auto(data_dir: &Path, keep: usize) -> Result<usize> {
    let stale: Vec<_> = list_auto(data_dir).into_iter().skip(keep).collect();
    for entry in &stale {
        fs::remove_file(&entry.path)?;
    }
    Ok(stale.len())
}

/// 最近一份指定种类的自动备份时间。
pub fn latest_auto(data_dir: &Path, kind: BackupKind) -> Option<i64> {
    list_auto(data_dir)
        .into_iter()
        .filter_map(|e| e.manifest)
        .filter(|m| m.kind == kind)
        .map(|m| m.created_at)
        .max()
}

pub fn read_manifest(zip_path: &Path) -> Result<Manifest> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?).map_err(zip_err)?;
    let mut entry = archive
        .by_name(MANIFEST_ENTRY)
        .map_err(|_| Error::Backup(BackupProblem::ManifestMissing))?;
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    serde_json::from_str(&text).map_err(|_| Error::Backup(BackupProblem::ManifestInvalid))
}

/// 启动时、建连接池之前：库版本低于程序时先备份一次。备份失败返回错误，调用方不得继续迁移。
pub fn before_upgrade(
    db_path: &Path,
    data_dir: &Path,
    app_version: &str,
) -> Result<Option<PathBuf>> {
    if !db_path.is_file() {
        return Ok(None);
    }
    let conn = Connection::open(db_path)?;
    let version = user_version(&conn)?;
    if version == 0 || version >= SCHEMA_VERSION {
        return Ok(None);
    }
    let path = create_auto(&conn, data_dir, BackupKind::Upgrade, app_version)?;
    tracing::info!(from = version, to = SCHEMA_VERSION, file = %crate::logsafe::path(&path), "backed up before schema upgrade");
    Ok(Some(path))
}

// ---------------------------------------------------------------------------
// 恢复
// ---------------------------------------------------------------------------

/// 暂存检查通过的备份。
#[derive(Debug, Clone)]
pub struct RestoreCheck {
    pub source: PathBuf,
    pub manifest: Manifest,
    /// 按暂存库实际统计，不信任清单里的数。
    pub counts: Counts,
    pub schema_version: i32,
}

/// 解压到暂存目录并检查：完整性、库版本不低于最低支持且不高于程序。失败时清掉暂存。
pub fn inspect(zip_path: &Path, data_dir: &Path) -> Result<RestoreCheck> {
    let staging = data_dir.join(STAGING);
    let _ = fs::remove_file(data_dir.join(PENDING));
    remove_path(&staging)?;
    let result = stage(zip_path, &staging);
    if result.is_err() {
        let _ = remove_path(&staging);
    }
    result
}

fn stage(zip_path: &Path, staging: &Path) -> Result<RestoreCheck> {
    let manifest = read_manifest(zip_path)?;
    if manifest.format > FORMAT {
        return Err(Error::Backup(BackupProblem::FormatUnsupported));
    }
    fs::create_dir_all(staging)?;
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?).map_err(zip_err)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(zip_err)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        if !allowed_entry(&rel) || entry.is_dir() {
            continue;
        }
        let out = staging.join(&rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut File::create(&out)?)?;
    }
    let db = staging.join(DB_ENTRY);
    if !db.is_file() {
        return Err(Error::Backup(BackupProblem::DatabaseMissing));
    }
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| Error::Backup(BackupProblem::DatabaseUnreadable))?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|_| Error::Backup(BackupProblem::DatabaseCorrupt))?;
    if integrity != "ok" {
        return Err(Error::Backup(BackupProblem::DatabaseCorrupt));
    }
    let schema_version = user_version(&conn)?;
    if schema_version > SCHEMA_VERSION {
        return Err(Error::Backup(BackupProblem::SchemaUnsupported {
            found: schema_version,
            supported: SCHEMA_VERSION,
        }));
    }
    if schema_version == 0 {
        return Err(Error::Backup(BackupProblem::DatabaseEmpty));
    }
    if schema_version < MIN_UPGRADE_VERSION {
        return Err(Error::Backup(BackupProblem::SchemaTooOld {
            found: schema_version,
            min: MIN_UPGRADE_VERSION,
        }));
    }
    Ok(RestoreCheck {
        source: zip_path.to_path_buf(),
        counts: counts(&conn)?,
        manifest,
        schema_version,
    })
}

fn allowed_entry(rel: &Path) -> bool {
    if rel == Path::new(DB_ENTRY) || rel == Path::new(MANIFEST_ENTRY) {
        return true;
    }
    if rel.starts_with(SESSIONS) {
        return true;
    }
    [SKILLS, LEGACY_SKILLS].iter().any(|root| {
        rel.strip_prefix(root)
            .ok()
            .and_then(|r| r.components().next())
            .is_some_and(|c| !c.as_os_str().to_string_lossy().starts_with('_'))
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct Pending {
    source: PathBuf,
    pid: u32,
    created_at: i64,
}

/// 确认恢复：写待办文件，下次启动时替换。调用方随后重启应用。
pub fn schedule_restore(data_dir: &Path, source: &Path) -> Result<()> {
    if !data_dir.join(STAGING).join(DB_ENTRY).is_file() {
        return Err(Error::Backup(BackupProblem::StagingMissing));
    }
    let pending = Pending {
        source: source.to_path_buf(),
        pid: std::process::id(),
        created_at: crate::feeds::now_ms(),
    };
    write_json(&data_dir.join(PENDING), &pending)
}

/// 放弃已检查的备份。
pub fn cancel_restore(data_dir: &Path) -> Result<()> {
    let _ = fs::remove_file(data_dir.join(PENDING));
    remove_path(&data_dir.join(STAGING))
}

/// 上一次启动时执行恢复的结果，显示给用户后由 [`clear_restore_result`] 清除。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResult {
    pub ok: bool,
    pub message: String,
    pub at: i64,
}

pub fn restore_result(data_dir: &Path) -> Option<RestoreResult> {
    let text = fs::read_to_string(data_dir.join(RESULT)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn clear_restore_result(data_dir: &Path) -> Result<()> {
    match fs::remove_file(data_dir.join(RESULT)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// 等上次运行的进程退出的上限。
const EXIT_WAIT: Duration = Duration::from_secs(10);

/// 启动时、打开数据库之前调用：有待办恢复就替换文件。替换失败时按相反顺序移回原文件，结果写入结果文件。
pub fn apply_pending(data_dir: &Path, db_path: &Path) -> Result<Option<RestoreResult>> {
    apply_pending_with(data_dir, db_path, EXIT_WAIT, &|from, to| {
        fs::rename(from, to)
    })
}

/// `rollback` 是回滚时移回文件用的改名，测试里用它模拟移回失败。
fn apply_pending_with(
    data_dir: &Path,
    db_path: &Path,
    exit_wait: Duration,
    rollback: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> Result<Option<RestoreResult>> {
    let pending_path = data_dir.join(PENDING);
    let Ok(text) = fs::read_to_string(&pending_path) else {
        // 检查过但没确认的暂存，启动时顺手清掉。
        let _ = remove_path(&data_dir.join(STAGING));
        return Ok(None);
    };
    // 先删待办，替换中途崩溃也不会在每次启动时重试。
    fs::remove_file(&pending_path)?;
    let pending: Option<Pending> = serde_json::from_str(&text).ok();
    let staging = data_dir.join(STAGING);
    let name = pending
        .as_ref()
        .and_then(|p| p.source.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "备份".into());
    let exited = pending
        .as_ref()
        .is_none_or(|p| wait_for_exit(p.pid, exit_wait));
    let outcome =
        if !exited {
            // 旧进程还开着库就换文件，它可能继续写到新库旁边，所以宁可不换。
            Err(SwapError::RolledBack(Error::Backup(format!(
            "上次运行的 RRSS 在 {} 秒内没有退出，没有替换文件。请关掉所有 RRSS 窗口后重新恢复",
            exit_wait.as_secs()
        ).into())))
        } else if !staging.join(DB_ENTRY).is_file() {
            Err(SwapError::RolledBack(Error::Backup(
                "暂存的备份不完整".into(),
            )))
        } else {
            swap(data_dir, db_path, &staging, rollback)
        };
    let result = match outcome {
        Ok(()) => {
            let _ = remove_path(&staging);
            RestoreResult {
                ok: true,
                message: format!("已从 {name} 恢复。"),
                at: crate::feeds::now_ms(),
            }
        }
        Err(SwapError::RolledBack(err)) => {
            tracing::error!(error = %crate::logsafe::error(&err), "restore failed; original data kept");
            let _ = remove_path(&staging);
            RestoreResult {
                ok: false,
                message: format!("从 {name} 恢复失败，当前数据未改动：{err}"),
                at: crate::feeds::now_ms(),
            }
        }
        Err(SwapError::Partial(err)) => {
            tracing::error!(error = %crate::logsafe::error(&err), "restore failed and rollback incomplete");
            // 暂存里可能还有移回来的备份文件，改名留着，免得下次启动当成没确认的暂存删掉。
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let kept = data_dir.join(format!("{STAGING}-{stamp}"));
            let kept_note = match fs::rename(&staging, &kept) {
                Ok(()) => format!("，备份里的文件留在 {}", kept.display()),
                Err(_) => String::new(),
            };
            RestoreResult {
                ok: false,
                message: format!(
                    "从 {name} 恢复失败，而且有文件没能移回原处：{err}。原来的数据留在 {}{kept_note}。请先退出 RRSS，把其中的文件移回 {} 再打开，不要删除这些文件夹。",
                    data_dir.join(OLD).display(),
                    data_dir.display()
                ),
                at: crate::feeds::now_ms(),
            }
        }
    };
    write_json(&data_dir.join(RESULT), &result)?;
    Ok(Some(result))
}

/// 替换失败时，原文件是否都已回到原处。
enum SwapError {
    /// 没动文件，或已全部移回。
    RolledBack(Error),
    /// 有文件没能移回：库、会话、技能可能新旧混在一起，`restore-old/` 里还有原数据。
    Partial(Error),
}

/// 当前文件移到 `restore-old/`，暂存文件移入；任何一步失败都按相反顺序移回。
fn swap(
    data_dir: &Path,
    db_path: &Path,
    staging: &Path,
    rollback: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> std::result::Result<(), SwapError> {
    let old = data_dir.join(OLD);
    if old.exists() {
        // 上次回滚没走完时这里可能还有用户数据，改名留着，不删。
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        fs::rename(&old, data_dir.join(format!("{OLD}-{stamp}")))
            .map_err(|e| SwapError::RolledBack(e.into()))?;
    }
    fs::create_dir_all(&old).map_err(|e| SwapError::RolledBack(e.into()))?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = (|| -> Result<()> {
        let db_name = db_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut current: Vec<(PathBuf, PathBuf)> = ["", "-wal", "-shm"]
            .iter()
            .map(|suffix| {
                let name = format!("{db_name}{suffix}");
                (db_path.with_file_name(&name), old.join(&name))
            })
            .collect();
        current.push((data_dir.join(SESSIONS), old.join(SESSIONS)));
        for (name, path) in user_skills(data_dir) {
            current.push((path, old.join(SKILLS).join(name)));
        }
        for (from, to) in current {
            if from.symlink_metadata().is_ok() {
                move_path(&from, &to, &mut moved)?;
            }
        }
        move_path(&staging.join(DB_ENTRY), db_path, &mut moved)?;
        if staging.join(SESSIONS).is_dir() {
            move_path(
                &staging.join(SESSIONS),
                &data_dir.join(SESSIONS),
                &mut moved,
            )?;
        }
        for (name, path) in user_skills(staging) {
            move_path(&path, &data_dir.join(SKILLS).join(name), &mut moved)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            let _ = remove_path(&old);
            Ok(())
        }
        Err(err) => {
            let mut complete = true;
            for (from, to) in moved.iter().rev() {
                if let Err(e) = rollback(to, from) {
                    complete = false;
                    tracing::error!(error = %e, from = %crate::logsafe::path(to), to = %crate::logsafe::path(from), "restore rollback step failed");
                }
            }
            Err(if complete {
                SwapError::RolledBack(err)
            } else {
                SwapError::Partial(err)
            })
        }
    }
}

fn move_path(from: &Path, to: &Path, moved: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(from, to)?;
    moved.push((from.to_path_buf(), to.to_path_buf()));
    Ok(())
}

/// 等旧进程退出再动文件。等满 `limit` 仍没退出返回 `false`，调用方不得替换。
/// 查不到进程状态的平台（见 [`crate::platform::process_alive`]）只给一点退出时间：
/// Windows 上 SQLite 打开的文件不许改名，旧进程没退出时替换会失败并整体移回。
fn wait_for_exit(pid: u32, limit: Duration) -> bool {
    if pid == std::process::id() {
        return true;
    }
    if crate::platform::process_alive(pid).is_none() {
        std::thread::sleep(Duration::from_secs(2));
        return true;
    }
    let deadline = std::time::Instant::now() + limit;
    while crate::platform::process_alive(pid) == Some(true) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// `<root>/pi/user-skills/` 下的用户技能；也读旧位置 `pi-project/.pi/skills/`，同名以新位置为准。
/// `_` 开头的是随应用提供的，不备份。
fn user_skills(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for dir in [SKILLS, LEGACY_SKILLS] {
        let Ok(entries) = fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir()
                && !name.starts_with('_')
                && !out.iter().any(|(n, _)| *n == name)
            {
                out.push((name, entry.path()));
            }
        }
    }
    out.sort();
    out
}

fn add_tree<W: Write + io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &Path,
    prefix: &str,
    opts: SimpleFileOptions,
) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        if meta.is_dir() {
            add_tree(zip, &entry.path(), &name, opts)?;
        } else if meta.is_file() {
            zip.start_file(name, opts).map_err(zip_err)?;
            io::copy(&mut File::open(entry.path())?, zip)?;
        }
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(_) => {}
    }
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value).map_err(json_err)?)?;
    Ok(())
}

fn zip_err(err: zip::result::ZipError) -> Error {
    match err {
        zip::result::ZipError::Io(e) => Error::Io(e),
        _ => Error::Backup(BackupProblem::ZipInvalid),
    }
}

fn json_err(_err: serde_json::Error) -> Error {
    Error::Backup(BackupProblem::Serialization)
}

/// 出错时删掉的临时文件。
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    const V: &str = "0.0.0-test";

    fn seed(data: &Path) -> PathBuf {
        let db = data.join("rrss.db");
        let conn = open_at(&db).unwrap();
        conn.execute(
            "INSERT INTO feeds (url, title, created_at) VALUES ('https://a.example/rss', 'A', 1)",
            [],
        )
        .unwrap();
        fs::create_dir_all(data.join(SESSIONS)).unwrap();
        fs::write(data.join(SESSIONS).join("s1.jsonl"), "{}\n").unwrap();
        fs::create_dir_all(data.join(SKILLS).join("mine")).unwrap();
        fs::write(data.join(SKILLS).join("mine/SKILL.md"), "mine").unwrap();
        fs::create_dir_all(data.join(SKILLS).join("_bundled/x")).unwrap();
        fs::write(data.join(SKILLS).join("_bundled/x/SKILL.md"), "bundled").unwrap();
        db
    }

    fn entry_names(zip_path: &Path) -> Vec<String> {
        let archive = zip::ZipArchive::new(File::open(zip_path).unwrap()).unwrap();
        let mut names: Vec<_> = archive.file_names().map(str::to_string).collect();
        names.sort();
        names
    }

    #[test]
    fn upgrade_backup_works_before_the_first_pool_opens() {
        const CHILD_DB: &str = "RRSS_TEST_BACKUP_COLD_DB";
        if let Some(path) = std::env::var_os(CHILD_DB) {
            let db = PathBuf::from(path);
            assert!(
                before_upgrade(&db, db.parent().unwrap(), V)
                    .unwrap()
                    .is_some()
            );
            return;
        }
        let dir = tempdir().unwrap();
        let path = seed(dir.path());
        let conn = open_at(&path).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION - 1)
            .unwrap();
        drop(conn);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backup::tests::upgrade_backup_works_before_the_first_pool_opens",
                "--nocapture",
            ])
            .env(CHILD_DB, &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn secrets_are_stripped_only_in_the_copy_and_manual_opt_in_round_trips() {
        let source = tempdir().unwrap();
        let db = seed(source.path());
        let conn = open_at(&db).unwrap();
        conn.execute("INSERT INTO ai_providers (name, base_url, api_key, created_at) VALUES ('keyed', 'https://example.test', 'provider-private-9876', 0), ('anonymous', 'http://localhost', NULL, 0)", []).unwrap();
        let provider_id: i64 = conn
            .query_row(
                "SELECT id FROM ai_providers WHERE name = 'keyed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::ai_store::set_task_binding(
            &conn,
            crate::ai_store::TASK_ARTICLE_SUMMARY,
            Some(provider_id),
            Some("kept-model"),
        )
        .unwrap();
        for key in crate::translations::SECRET_KEYS {
            crate::db::set_setting(&conn, key, &format!("{key}-private-1234")).unwrap();
        }
        conn.execute("INSERT INTO mcp_servers (name, env_json, headers_json, created_at) VALUES ('tools', '{\"TOKEN\":\"mcp-env-private\"}', '{\"Authorization\":\"mcp-header-private\"}', 0)", []).unwrap();
        let proxy_config = crate::network::ProxyConfig {
            global: crate::network::ProxyRule {
                mode: crate::network::ProxyMode::Custom,
                url: "socks5h://proxy-user-private:proxy-password-private@localhost:1080".into(),
                no_proxy: "localhost".into(),
            },
            ..Default::default()
        };
        let proxy_config = crate::network::save(&conn, &proxy_config).unwrap();
        for included in [false, true] {
            let dest = source.path().join(format!("{included}.zip"));
            let manifest =
                create(&conn, source.path(), &dest, BackupKind::Manual, V, included).unwrap();
            assert_eq!(manifest.secrets_included, included);
            let restore = tempdir().unwrap();
            inspect(&dest, restore.path()).unwrap();
            schedule_restore(restore.path(), &dest).unwrap();
            let restored = restore.path().join("rrss.db");
            assert!(
                apply_pending(restore.path(), &restored)
                    .unwrap()
                    .unwrap()
                    .ok
            );
            let raw = fs::read(&restored).unwrap();
            let target = Connection::open(&restored).unwrap();
            let key: String = target
                .query_row(
                    "SELECT api_key FROM ai_providers WHERE name = 'keyed'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                key,
                if included {
                    "provider-private-9876"
                } else {
                    ""
                }
            );
            assert_eq!(
                target
                    .query_row(
                        "SELECT api_key FROM ai_providers WHERE name = 'anonymous'",
                        [],
                        |r| r.get::<_, Option<String>>(0)
                    )
                    .unwrap(),
                None
            );
            assert_eq!(
                crate::network::load(&target).unwrap(),
                if included {
                    proxy_config.clone()
                } else {
                    proxy_config.without_auth().unwrap()
                }
            );
            let mut literals = vec![
                "proxy-user-private".to_string(),
                "proxy-password-private".into(),
                "provider-private-9876".to_string(),
                "mcp-env-private".into(),
                "mcp-header-private".into(),
            ];
            for key in crate::translations::SECRET_KEYS {
                let secret = format!("{key}-private-1234");
                assert_eq!(
                    crate::db::get_setting(&target, key).unwrap().unwrap(),
                    if included {
                        secret.clone()
                    } else {
                        String::new()
                    }
                );
                assert_eq!(
                    crate::db::get_setting(&target, &crate::translations::missing_key(key))
                        .unwrap()
                        .is_some(),
                    !included
                );
                literals.push(secret);
            }
            for secret in literals {
                assert_eq!(
                    raw.windows(secret.len())
                        .any(|bytes| bytes == secret.as_bytes()),
                    included
                );
            }
            let vars: String = target
                .query_row("SELECT env_json FROM mcp_servers", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&vars).unwrap()["TOKEN"],
                if included { "mcp-env-private" } else { "" }
            );
            assert_eq!(counts(&target).unwrap(), manifest.counts);
            let binding =
                crate::ai_store::get_task_binding(&target, crate::ai_store::TASK_ARTICLE_SUMMARY)
                    .unwrap();
            assert_eq!(binding.provider_id, Some(provider_id));
            assert_eq!(binding.model.as_deref(), Some("kept-model"));
        }
        // 自动调用即使误传 true 也不带密钥；日常库原值不变。
        let automatic = source.path().join("automatic.zip");
        assert!(
            !create(
                &conn,
                source.path(),
                &automatic,
                BackupKind::Scheduled,
                V,
                true
            )
            .unwrap()
            .secrets_included
        );
        assert_eq!(crate::network::load(&conn).unwrap(), proxy_config);
        let auto_copy = tempdir().unwrap();
        inspect(&automatic, auto_copy.path()).unwrap();
        schedule_restore(auto_copy.path(), &automatic).unwrap();
        let auto_db = auto_copy.path().join("rrss.db");
        assert!(
            apply_pending(auto_copy.path(), &auto_db)
                .unwrap()
                .unwrap()
                .ok
        );
        let auto_conn = Connection::open(&auto_db).unwrap();
        assert_eq!(
            crate::network::load(&auto_conn).unwrap(),
            proxy_config.without_auth().unwrap()
        );
        assert_eq!(
            conn.query_row(
                "SELECT api_key FROM ai_providers WHERE name = 'keyed'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "provider-private-9876"
        );
        assert_eq!(
            crate::db::get_setting(&conn, crate::translations::TRANSLATE_HTTP_KEY_KEY)
                .unwrap()
                .as_deref(),
            Some("translate_http_key-private-1234")
        );
    }

    #[test]
    fn old_manifests_are_recognized_as_containing_secrets() {
        let old = serde_json::json!({"format": 1, "app_version": "old", "schema_version": 20, "created_at": 0,
            "kind": "manual", "counts": {"articles": 0, "feeds": 0, "rules": 0, "threads": 0}});
        assert!(
            serde_json::from_value::<Manifest>(old)
                .unwrap()
                .secrets_included
        );
    }

    #[test]
    fn backup_contains_db_sessions_user_skills_and_manifest() {
        let dir = tempdir().unwrap();
        let db = seed(dir.path());
        let conn = open_at(&db).unwrap();
        let dest = dir.path().join("out/b.zip");
        let manifest = create(&conn, dir.path(), &dest, BackupKind::Manual, V, false).unwrap();
        assert_eq!(manifest.counts.feeds, 1);
        assert_eq!(manifest.schema_version, SCHEMA_VERSION);
        assert_eq!(
            entry_names(&dest),
            vec![
                "manifest.json",
                "pi/sessions/s1.jsonl",
                "pi/user-skills/mine/SKILL.md",
                "rrss.db",
            ]
        );
        assert_eq!(read_manifest(&dest).unwrap(), manifest);
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("out"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["b.zip"]);
    }

    #[test]
    fn auto_backups_keep_latest_five() {
        let dir = tempdir().unwrap();
        let db = seed(dir.path());
        let conn = open_at(&db).unwrap();
        fs::create_dir_all(auto_dir(dir.path())).unwrap();
        for i in 0..7 {
            let path = auto_dir(dir.path()).join(format!("rrss-scheduled-{i}.zip"));
            let mut m = create(&conn, dir.path(), &path, BackupKind::Scheduled, V, false).unwrap();
            // 手工改写时间，保证顺序确定。
            m.created_at = i;
            rewrite_manifest(&path, &m);
        }
        assert_eq!(prune_auto(dir.path(), AUTO_KEEP).unwrap(), 2);
        let left: Vec<_> = list_auto(dir.path())
            .iter()
            .map(|e| e.manifest.as_ref().unwrap().created_at)
            .collect();
        assert_eq!(left, vec![6, 5, 4, 3, 2]);
        assert_eq!(latest_auto(dir.path(), BackupKind::Scheduled), Some(6));
        assert_eq!(latest_auto(dir.path(), BackupKind::Upgrade), None);
    }

    fn rewrite_manifest(path: &Path, manifest: &Manifest) {
        let mut src = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let tmp = path.with_extension("tmp");
        let mut out = zip::ZipWriter::new(File::create(&tmp).unwrap());
        for i in 0..src.len() {
            let file = src.by_index(i).unwrap();
            if file.name() == MANIFEST_ENTRY {
                continue;
            }
            out.raw_copy_file(file).unwrap();
        }
        out.start_file(MANIFEST_ENTRY, SimpleFileOptions::default())
            .unwrap();
        out.write_all(&serde_json::to_vec(manifest).unwrap())
            .unwrap();
        out.finish().unwrap();
        fs::rename(tmp, path).unwrap();
    }

    #[test]
    fn upgrade_backup_only_for_older_schema() {
        let dir = tempdir().unwrap();
        let db = seed(dir.path());
        assert!(before_upgrade(&db, dir.path(), V).unwrap().is_none());
        // 独立连接改版本号，模拟老库（池里的连接不受影响）。
        let raw = Connection::open(&db).unwrap();
        raw.pragma_update(None, "user_version", SCHEMA_VERSION - 1)
            .unwrap();
        let path = before_upgrade(&db, dir.path(), V).unwrap().expect("backup");
        let m = read_manifest(&path).unwrap();
        assert_eq!(m.kind, BackupKind::Upgrade);
        assert_eq!(m.schema_version, SCHEMA_VERSION - 1);
        assert!(
            before_upgrade(&dir.path().join("missing.db"), dir.path(), V)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn restore_round_trip_replaces_data_and_keeps_bundled_skills() {
        let src = tempdir().unwrap();
        let db = seed(src.path());
        let backup = src.path().join("b.zip");
        create(
            &open_at(&db).unwrap(),
            src.path(),
            &backup,
            BackupKind::Manual,
            V,
            false,
        )
        .unwrap();

        // 目标目录：不同的数据，库尚未打开（模拟启动前）。
        let dst = tempdir().unwrap();
        let dst_db = dst.path().join("rrss.db");
        {
            let conn = Connection::open(&dst_db).unwrap();
            conn.execute_batch("CREATE TABLE junk (x); PRAGMA user_version = 1;")
                .unwrap();
        }
        fs::create_dir_all(dst.path().join(SESSIONS)).unwrap();
        fs::write(dst.path().join(SESSIONS).join("other.jsonl"), "x").unwrap();
        fs::create_dir_all(dst.path().join(SKILLS).join("_bundled/y")).unwrap();
        fs::create_dir_all(dst.path().join(SKILLS).join("theirs")).unwrap();

        let check = inspect(&backup, dst.path()).unwrap();
        assert_eq!(check.counts.feeds, 1);
        schedule_restore(dst.path(), &check.source).unwrap();
        let result = apply_pending(dst.path(), &dst_db)
            .unwrap()
            .expect("applied");
        assert!(result.ok, "{}", result.message);
        assert_eq!(restore_result(dst.path()), Some(result));

        let conn = Connection::open(&dst_db).unwrap();
        assert_eq!(counts(&conn).unwrap().feeds, 1);
        assert!(dst.path().join(SESSIONS).join("s1.jsonl").is_file());
        assert!(!dst.path().join(SESSIONS).join("other.jsonl").exists());
        assert!(dst.path().join(SKILLS).join("mine/SKILL.md").is_file());
        assert!(!dst.path().join(SKILLS).join("theirs").exists());
        assert!(dst.path().join(SKILLS).join("_bundled/y").is_dir());
        assert!(!dst.path().join(STAGING).exists());
        assert!(!dst.path().join(OLD).exists());
        assert!(apply_pending(dst.path(), &dst_db).unwrap().is_none());
    }

    #[test]
    fn failed_swap_rolls_back_original_files() {
        let src = tempdir().unwrap();
        let db = seed(src.path());
        let backup = src.path().join("b.zip");
        create(
            &open_at(&db).unwrap(),
            src.path(),
            &backup,
            BackupKind::Manual,
            V,
            false,
        )
        .unwrap();

        let dst = tempdir().unwrap();
        let dst_db = dst.path().join("rrss.db");
        fs::write(&dst_db, b"original").unwrap();
        fs::write(dst.path().join("rrss.db-wal"), b"wal").unwrap();
        fs::create_dir_all(dst.path().join(SESSIONS)).unwrap();
        fs::write(dst.path().join(SESSIONS).join("keep.jsonl"), "k").unwrap();
        let check = inspect(&backup, dst.path()).unwrap();
        schedule_restore(dst.path(), &check.source).unwrap();
        // 库和会话移入之后，技能目标的父级是文件，移入技能失败，前面几步都要移回。
        fs::write(dst.path().join(SKILLS), b"file").unwrap();

        let result = apply_pending(dst.path(), &dst_db)
            .unwrap()
            .expect("attempted");
        assert!(!result.ok);
        assert_eq!(fs::read(&dst_db).unwrap(), b"original");
        assert_eq!(fs::read(dst.path().join("rrss.db-wal")).unwrap(), b"wal");
        assert!(dst.path().join(SESSIONS).join("keep.jsonl").is_file());
        assert!(!dst.path().join(STAGING).exists());
    }

    /// 目标目录里有原数据、已检查并确认一份备份，但技能目录移入会失败（父级是文件）。
    fn pending_restore_that_fails_midway() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let src = tempdir().unwrap();
        let db = seed(src.path());
        let backup = src.path().join("b.zip");
        create(
            &open_at(&db).unwrap(),
            src.path(),
            &backup,
            BackupKind::Manual,
            V,
            false,
        )
        .unwrap();
        let dst = tempdir().unwrap();
        let dst_db = dst.path().join("rrss.db");
        fs::write(&dst_db, b"original").unwrap();
        fs::create_dir_all(dst.path().join(SESSIONS)).unwrap();
        fs::write(dst.path().join(SESSIONS).join("keep.jsonl"), "k").unwrap();
        let check = inspect(&backup, dst.path()).unwrap();
        schedule_restore(dst.path(), &check.source).unwrap();
        // 技能目标的父级是文件：库和会话移入之后，移入技能失败。
        fs::write(dst.path().join(SKILLS), b"file").unwrap();
        (src, dst, dst_db)
    }

    #[test]
    fn incomplete_rollback_says_so_and_keeps_both_copies() {
        let (_src, dst, dst_db) = pending_restore_that_fails_midway();
        let old_db = dst.path().join(OLD).join("rrss.db");
        // 移回原库这一步失败。
        let rollback = |from: &Path, to: &Path| {
            if from == old_db {
                Err(io::Error::other("simulated"))
            } else {
                fs::rename(from, to)
            }
        };
        let result = apply_pending_with(dst.path(), &dst_db, EXIT_WAIT, &rollback)
            .unwrap()
            .expect("attempted");
        assert!(!result.ok);
        assert!(!result.message.contains("未改动"), "{}", result.message);
        assert!(result.message.contains("没能移回"), "{}", result.message);
        assert_eq!(fs::read(&old_db).unwrap(), b"original");
        // 其他步骤照常移回。
        assert!(dst.path().join(SESSIONS).join("keep.jsonl").is_file());
        // 暂存改名保留，下次启动也不会被当成没确认的暂存清掉。
        assert!(!dst.path().join(STAGING).exists());
        let kept: Vec<PathBuf> = fs::read_dir(dst.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{STAGING}-"))
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].join(DB_ENTRY).is_file());
        assert!(apply_pending(dst.path(), &dst_db).unwrap().is_none());
        assert!(kept[0].join(DB_ENTRY).is_file());
        assert!(old_db.is_file());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn restore_is_skipped_while_old_process_still_runs() {
        let (_src, dst, dst_db) = pending_restore_that_fails_midway();
        fs::remove_file(dst.path().join(SKILLS)).unwrap();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut pending: Pending =
            serde_json::from_str(&fs::read_to_string(dst.path().join(PENDING)).unwrap()).unwrap();
        pending.pid = child.id();
        write_json(&dst.path().join(PENDING), &pending).unwrap();

        let result =
            apply_pending_with(dst.path(), &dst_db, Duration::from_millis(300), &|a, b| {
                fs::rename(a, b)
            })
            .unwrap()
            .expect("attempted");
        let _ = child.kill();
        let _ = child.wait();
        assert!(!result.ok);
        assert!(result.message.contains("没有退出"), "{}", result.message);
        assert_eq!(fs::read(&dst_db).unwrap(), b"original");
        assert!(dst.path().join(SESSIONS).join("keep.jsonl").is_file());
        assert!(!dst.path().join(OLD).exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exited_but_unreaped_process_counts_as_gone() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        // 不回收，等它变成僵尸。
        for _ in 0..100 {
            if crate::platform::process_alive(pid) != Some(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(crate::platform::process_alive(pid), Some(false));
        assert!(
            fs::metadata(format!("/proc/{pid}")).is_ok(),
            "zombie should still be listed"
        );
        assert!(wait_for_exit(pid, Duration::from_millis(100)));
        child.wait().unwrap();
        assert_eq!(
            crate::platform::process_alive(std::process::id()),
            Some(true)
        );
    }

    #[test]
    fn inspect_rejects_newer_schema_and_non_backups() {
        let src = tempdir().unwrap();
        let db = seed(src.path());
        let backup = src.path().join("b.zip");
        create(
            &open_at(&db).unwrap(),
            src.path(),
            &backup,
            BackupKind::Manual,
            V,
            false,
        )
        .unwrap();

        let dst = tempdir().unwrap();
        // 把暂存库版本改高：重新打包一份库版本更高的备份。
        let staged = inspect(&backup, dst.path()).unwrap();
        assert_eq!(staged.schema_version, SCHEMA_VERSION);
        let newer = dst.path().join(STAGING).join(DB_ENTRY);
        Connection::open(&newer)
            .unwrap()
            .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        let newer_bytes = fs::read(&newer).unwrap();
        Connection::open(&newer)
            .unwrap()
            .pragma_update(None, "user_version", MIN_UPGRADE_VERSION - 1)
            .unwrap();
        let older_bytes = fs::read(&newer).unwrap();
        let repacked = src.path().join("newer.zip");
        {
            let mut out = zip::ZipWriter::new(File::create(&repacked).unwrap());
            out.start_file(MANIFEST_ENTRY, SimpleFileOptions::default())
                .unwrap();
            out.write_all(&serde_json::to_vec(&staged.manifest).unwrap())
                .unwrap();
            out.start_file(DB_ENTRY, SimpleFileOptions::default())
                .unwrap();
            out.write_all(&newer_bytes).unwrap();
            out.finish().unwrap();
        }
        let err = inspect(&repacked, dst.path()).unwrap_err();
        assert!(
            matches!(err, Error::Backup(BackupProblem::SchemaUnsupported { found, supported }) if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION)
        );
        assert!(!dst.path().join(STAGING).exists());

        let too_old = src.path().join("too-old.zip");
        {
            let mut out = zip::ZipWriter::new(File::create(&too_old).unwrap());
            out.start_file(MANIFEST_ENTRY, SimpleFileOptions::default())
                .unwrap();
            out.write_all(&serde_json::to_vec(&staged.manifest).unwrap())
                .unwrap();
            out.start_file(DB_ENTRY, SimpleFileOptions::default())
                .unwrap();
            out.write_all(&older_bytes).unwrap();
            out.finish().unwrap();
        }
        let err = inspect(&too_old, dst.path()).unwrap_err();
        assert!(
            matches!(err, Error::Backup(BackupProblem::SchemaTooOld { found, min }) if found == MIN_UPGRADE_VERSION - 1 && min == MIN_UPGRADE_VERSION)
        );
        assert!(!dst.path().join(STAGING).exists());

        let junk = src.path().join("junk.zip");
        fs::write(&junk, b"not a zip").unwrap();
        assert!(inspect(&junk, dst.path()).is_err());
        assert!(schedule_restore(dst.path(), &staged.source).is_err());
    }

    #[test]
    fn startup_without_pending_restore_drops_unconfirmed_staging() {
        let src = tempdir().unwrap();
        let db = seed(src.path());
        let backup = src.path().join("b.zip");
        create(
            &open_at(&db).unwrap(),
            src.path(),
            &backup,
            BackupKind::Manual,
            V,
            false,
        )
        .unwrap();
        inspect(&backup, src.path()).unwrap();
        assert!(src.path().join(STAGING).exists());
        assert!(apply_pending(src.path(), &db).unwrap().is_none());
        assert!(!src.path().join(STAGING).exists());
    }

    #[test]
    fn skills_in_the_legacy_location_are_backed_up_and_restored_to_the_new_one() {
        // 还没迁移的数据目录：技能仍在旧位置，备份里写成新位置。
        let src = tempdir().unwrap();
        let db = seed(src.path());
        fs::create_dir_all(src.path().join(LEGACY_SKILLS).join("old")).unwrap();
        fs::write(src.path().join(LEGACY_SKILLS).join("old/SKILL.md"), "old").unwrap();
        fs::create_dir_all(src.path().join(LEGACY_SKILLS).join("mine")).unwrap();
        fs::write(
            src.path().join(LEGACY_SKILLS).join("mine/SKILL.md"),
            "stale",
        )
        .unwrap();
        let backup = src.path().join("b.zip");
        create(
            &open_at(&db).unwrap(),
            src.path(),
            &backup,
            BackupKind::Manual,
            V,
            false,
        )
        .unwrap();
        let names = entry_names(&backup);
        assert!(
            names.contains(&"pi/user-skills/old/SKILL.md".to_string()),
            "{names:?}"
        );
        assert_eq!(
            names.iter().filter(|n| n.contains("/mine/")).count(),
            1,
            "same name: the new location wins"
        );

        // 阶段 18 之前的备份：技能在旧路径，恢复到新位置。
        let staged = inspect(&backup, src.path()).unwrap();
        let legacy_zip = src.path().join("legacy.zip");
        {
            let mut out = zip::ZipWriter::new(File::create(&legacy_zip).unwrap());
            let opts = SimpleFileOptions::default();
            out.start_file(MANIFEST_ENTRY, opts).unwrap();
            out.write_all(&serde_json::to_vec(&staged.manifest).unwrap())
                .unwrap();
            out.start_file(DB_ENTRY, opts).unwrap();
            out.write_all(&fs::read(src.path().join(STAGING).join(DB_ENTRY)).unwrap())
                .unwrap();
            out.start_file("pi-project/.pi/skills/legacy/SKILL.md", opts)
                .unwrap();
            out.write_all(b"legacy").unwrap();
            out.finish().unwrap();
        }
        let dst = tempdir().unwrap();
        let dst_db = dst.path().join("rrss.db");
        fs::write(&dst_db, b"original").unwrap();
        let check = inspect(&legacy_zip, dst.path()).unwrap();
        schedule_restore(dst.path(), &check.source).unwrap();
        let result = apply_pending(dst.path(), &dst_db)
            .unwrap()
            .expect("applied");
        assert!(result.ok, "{}", result.message);
        assert_eq!(
            fs::read_to_string(dst.path().join(SKILLS).join("legacy/SKILL.md")).unwrap(),
            "legacy"
        );
    }

    #[test]
    fn entries_outside_backup_layout_are_ignored() {
        assert!(allowed_entry(Path::new("rrss.db")));
        assert!(allowed_entry(Path::new("pi/sessions/a/b.jsonl")));
        assert!(allowed_entry(Path::new("pi/user-skills/mine/SKILL.md")));
        assert!(!allowed_entry(Path::new("pi/user-skills/_x/SKILL.md")));
        assert!(!allowed_entry(Path::new("pi/agent/settings.json")));
        assert!(allowed_entry(Path::new(
            "pi-project/.pi/skills/mine/SKILL.md"
        )));
        assert!(!allowed_entry(Path::new(
            "pi-project/.pi/skills/_bundled/x/SKILL.md"
        )));
        assert!(!allowed_entry(Path::new("pi-project/.pi/extensions/x.ts")));
        assert!(!allowed_entry(Path::new("cache/media/1.jpg")));
    }
}
