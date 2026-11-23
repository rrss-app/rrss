use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Once, OnceLock};
use std::time::Instant;

use rusqlite::{Connection, Transaction, TransactionBehavior};

use super::migrations::migrate;
use crate::error::{Error, Result};

/// 进程内按路径复用的连接池上限（WAL 下多连接可并发读）。
const POOL_MAX_SIZE: usize = 8;
/// Leave two physical connections available to foreground commands/media reads.
pub const BACKGROUND_CONNECTIONS: usize = POOL_MAX_SIZE - 2;

struct PoolInner {
    path: PathBuf,
    idle: Vec<Connection>,
    open_count: usize,
}

struct DbPool {
    inner: Mutex<PoolInner>,
    condvar: Condvar,
    metrics: Arc<super::metrics::DatabaseMetrics>,
}

/// 从连接池借出的连接；`drop` 时归还，不要长期持有。
pub struct PooledConnection {
    conn: Option<Connection>,
    pool: Arc<DbPool>,
    _held: super::latency::Guard,
}

impl Deref for PooledConnection {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("pooled connection")
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.release(conn);
        }
    }
}

#[derive(Default)]
struct PoolEntry {
    pool: OnceLock<Arc<DbPool>>,
    initialize: Mutex<()>,
    metrics: Arc<super::metrics::DatabaseMetrics>,
}

static POOLS: OnceLock<Mutex<HashMap<PathBuf, Arc<PoolEntry>>>> = OnceLock::new();

pub fn open() -> Result<PooledConnection> {
    open_at(&crate::paths::db_path()?)
}

/// 从进程内连接池借出一条连接。父目录不存在时创建；同一 `path` 只建池一次。
pub fn open_at(path: &Path) -> Result<PooledConnection> {
    pool_for(path)?.acquire()
}

/// 开始一次写事务。一上来申请写锁，其它写者在连接的 `busy_timeout` 里等待。
/// 默认的 deferred 事务会先读再升级写锁，那种升级 SQLite 会立刻返回锁定。
pub fn write_transaction(conn: &Connection) -> Result<WriteTransaction<'_>> {
    let metrics = super::metrics::for_connection(conn);
    let start = Instant::now();
    let timeout =
        std::time::Duration::from_millis(
            conn.pragma_query_value(None, "busy_timeout", |row| row.get::<_, u64>(0))?,
        );
    let waiting = metrics
        .as_ref()
        .map(|m| m.activity.start("writer_admission"));
    // Admission and SQLite share the original busy budget; do not double the deadline.
    let permit = if conn.is_autocommit() {
        metrics
            .as_ref()
            .map(|metrics| metrics.writer.acquire(timeout))
            .transpose()
    } else {
        Ok(None)
    };
    drop(waiting);
    let result = (|| -> rusqlite::Result<_> {
        let permit = permit?;
        if permit.is_some() {
            conn.busy_timeout(timeout.saturating_sub(start.elapsed()))?;
        }
        let waiting = metrics
            .as_ref()
            .map(|m| m.activity.start("sqlite_write_wait"));
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate);
        drop(waiting);
        if permit.is_some() {
            conn.busy_timeout(timeout)?;
        }
        Ok((tx?, permit))
    })();
    if let Some(metrics) = &metrics {
        metrics.write_wait.record(start.elapsed());
    }
    let (tx, permit) = result?;
    Ok(WriteTransaction {
        tx: Some(tx),
        _held: metrics
            .as_ref()
            .map(|m| m.activity.start("write_transaction")),
        metrics,
        acquired: Instant::now(),
        _permit: permit,
    })
}

/// Same transaction semantics, with wait and lifetime measured separately. The
/// lifetime includes COMMIT's post-write-lock WAL checkpoint work. Drop
/// records rollback and cancellation too; nested savepoints are not double-counted.
pub struct WriteTransaction<'a> {
    tx: Option<Transaction<'a>>,
    metrics: Option<Arc<super::metrics::DatabaseMetrics>>,
    acquired: Instant,
    _permit: Option<super::write_gate::Permit>,
    _held: Option<super::latency::Guard>,
}
impl<'a> Deref for WriteTransaction<'a> {
    type Target = Transaction<'a>;
    fn deref(&self) -> &Self::Target {
        self.tx.as_ref().expect("active transaction")
    }
}
impl WriteTransaction<'_> {
    pub fn commit(mut self) -> rusqlite::Result<()> {
        let _timing = self.metrics.as_ref().map(|m| m.commit.start());
        let _activity = self.metrics.as_ref().map(|m| m.activity.start("commit"));
        // Admit the next writer before COMMIT: SQLite retains its own write lock
        // until the transaction is durable enough to proceed. Its automatic WAL
        // checkpoint can continue after releasing that lock; do not serialize
        // unrelated writers behind the checkpoint with our application permit.
        drop(self._permit.take());
        self.tx.take().expect("active transaction").commit()
    }
    pub fn rollback(mut self) -> rusqlite::Result<()> {
        self.tx.take().expect("active transaction").rollback()
    }
}
impl Drop for WriteTransaction<'_> {
    fn drop(&mut self) {
        // Release the write lock before recording the complete hold time.
        drop(self.tx.take());
        if let Some(metrics) = &self.metrics {
            metrics.write_hold.record(self.acquired.elapsed());
        }
    }
}

/// Only consult an already-open pool; diagnostics never open/migrate a database.
pub fn metrics(path: &Path) -> Option<Arc<super::metrics::DatabaseMetrics>> {
    POOLS
        .get()?
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(path)
        .map(|pool| pool.metrics.clone())
}

/// 原子地执行一组读后写。独立调用时开 IMMEDIATE 写事务：先拿写锁再读，
/// 其它写者提交不会让读到的快照过期（deferred 事务读后升级写锁会直接报忙）。
/// 已在调用方事务内时改用 SAVEPOINT，失败只回滚这一段。
pub fn atomic_write<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    if conn.is_autocommit() {
        let tx = write_transaction(conn)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    } else {
        let sp = Savepoint::enter(conn)?;
        let out = f(conn)?;
        sp.release()?;
        Ok(out)
    }
}

/// 在同一个读快照里执行多条查询，不占写锁（WAL 下写者照常提交）。
/// 用于把重计算移出写事务：先在这里读和算，再用短写事务核对版本后写回。
/// 已在调用方事务内时直接沿用该事务。
pub fn read_snapshot<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    if !conn.is_autocommit() {
        return f(conn);
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

/// `&Connection` 上的 SAVEPOINT（rusqlite 自带的需要 `&mut`）；未释放即在 drop 时回滚。
struct Savepoint<'a> {
    conn: &'a Connection,
    released: bool,
}

impl<'a> Savepoint<'a> {
    const NAME: &'static str = "rrss_atomic_write";

    fn enter(conn: &'a Connection) -> Result<Self> {
        conn.execute_batch(&format!("SAVEPOINT {}", Self::NAME))?;
        Ok(Self {
            conn,
            released: false,
        })
    }

    fn release(mut self) -> Result<()> {
        self.conn
            .execute_batch(&format!("RELEASE SAVEPOINT {}", Self::NAME))?;
        self.released = true;
        Ok(())
    }
}

impl Drop for Savepoint<'_> {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.conn.execute_batch(&format!(
                "ROLLBACK TO SAVEPOINT {name}; RELEASE SAVEPOINT {name}",
                name = Self::NAME
            ));
        }
    }
}

fn pool_for(path: &Path) -> Result<Arc<DbPool>> {
    let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    let entry = pools
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .entry(path.to_path_buf())
        .or_default()
        .clone();
    if let Some(pool) = entry.pool.get() {
        return Ok(Arc::clone(pool));
    }
    // Migrations may themselves record transaction metrics. Never retain the
    // registry lock across SQLite initialization (or serialize unrelated DBs).
    let _initialize = entry.initialize.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pool) = entry.pool.get() {
        return Ok(Arc::clone(pool));
    }
    let pool = DbPool::bootstrap(path, entry.metrics.clone())?;
    let _ = entry.pool.set(pool.clone());
    Ok(pool)
}

impl DbPool {
    fn bootstrap(path: &Path, metrics: Arc<super::metrics::DatabaseMetrics>) -> Result<Arc<Self>> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|source| Error::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        register_sqlite_vec();
        tracing::info!(file = %crate::logsafe::path(path), max_size = POOL_MAX_SIZE, "opened sqlite pool");
        let conn = Connection::open(path)?;
        super::metrics::attach(&conn, metrics.clone())?;
        configure(&conn)?;
        migrate(&conn)?;
        Ok(Arc::new(Self {
            inner: Mutex::new(PoolInner {
                path: path.to_path_buf(),
                idle: vec![conn],
                open_count: 1,
            }),
            condvar: Condvar::new(),
            metrics,
        }))
    }

    fn acquire(self: &Arc<Self>) -> Result<PooledConnection> {
        let _timing = self.metrics.connection_wait.start();
        let _activity = self.metrics.activity.start("connection_wait");
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        loop {
            if let Some(conn) = inner.idle.pop() {
                return Ok(PooledConnection {
                    conn: Some(conn),
                    pool: Arc::clone(self),
                    _held: self.metrics.activity.start("connection_held"),
                });
            }
            if inner.open_count < POOL_MAX_SIZE {
                let path = inner.path.clone();
                inner.open_count += 1;
                // Opening/configuring a physical connection must not prevent returns.
                drop(inner);
                match open_pool_connection(&path, self.metrics.clone()) {
                    Ok(conn) => {
                        return Ok(PooledConnection {
                            conn: Some(conn),
                            pool: Arc::clone(self),
                            _held: self.metrics.activity.start("connection_held"),
                        });
                    }
                    Err(err) => {
                        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        inner.open_count -= 1;
                        self.condvar.notify_all();
                        return Err(err);
                    }
                }
            }
            inner = self
                .condvar
                .wait(inner)
                .unwrap_or_else(|err| err.into_inner());
        }
    }

    fn release(self: &Arc<Self>, conn: Connection) {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        inner.idle.push(conn);
        self.condvar.notify_one();
    }
}

/// 池内新增物理连接：只跑 `configure`，迁移在建池时已完成。
fn open_pool_connection(
    path: &Path,
    metrics: Arc<super::metrics::DatabaseMetrics>,
) -> Result<Connection> {
    let conn = Connection::open(path)?;
    super::metrics::attach(&conn, metrics)?;
    configure(&conn)?;
    Ok(conn)
}

fn register_sqlite_vec() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

fn configure(conn: &Connection) -> Result<()> {
    let mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") && !mode.eq_ignore_ascii_case("memory") {
        tracing::warn!(journal_mode = %mode, error_kind = "journal_mode", "sqlite did not switch to wal");
    }
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // 写锁被占用时在库内重试。配合 write_transaction 的 IMMEDIATE，后来的写会等而不是立刻失败。
    conn.pragma_update(None, "busy_timeout", 30_000)?;
    // WAL 下用 NORMAL：提交写入 WAL 后不再逐次 fsync，落盘交给检查点。程序崩溃不丢，
    // 断电 / 系统崩溃可能回滚最后几次提交，库不损坏。FULL 时每次提交持写锁 fsync，
    // 遇到检查点写库文件会拖到数百毫秒，避免阻塞交互写入排队。
    // 备份与恢复另开连接，仍是默认的 FULL。
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn post_commit_wal_work_does_not_hold_writer_admission() {
        // SQLite invokes the WAL hook after releasing the writer lock, where its
        // automatic checkpoint normally runs. Hold that callback deterministically.
        struct Hook {
            committed: mpsc::Sender<()>,
            resume: mpsc::Receiver<()>,
        }
        unsafe extern "C" fn hold_checkpoint(
            context: *mut std::ffi::c_void,
            _: *mut rusqlite::ffi::sqlite3,
            _: *const std::ffi::c_char,
            _: std::ffi::c_int,
        ) -> std::ffi::c_int {
            // SAFETY: the hook's boxed state lives until registration is removed.
            let hook = unsafe { &*context.cast::<Hook>() };
            let _ = hook.committed.send(());
            let _ = hook.resume.recv_timeout(Duration::from_secs(5));
            rusqlite::ffi::SQLITE_OK
        }
        struct Registration<'a>(&'a Connection, Box<Hook>);
        impl Drop for Registration<'_> {
            fn drop(&mut self) {
                // SAFETY: remove the callback before its state or pooled connection
                // is released, including when the test unwinds.
                unsafe {
                    rusqlite::ffi::sqlite3_wal_hook(self.0.handle(), None, std::ptr::null_mut());
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checkpoint.db");
        drop(open_at(&path).unwrap());
        let (committed, ready) = mpsc::channel();
        let (resume, release) = mpsc::channel();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            let conn = open_at(&writer_path).unwrap();
            let mut hook = Registration(
                &conn,
                Box::new(Hook {
                    committed,
                    resume: release,
                }),
            );
            // SAFETY: only this thread accesses conn; Registration owns callback state.
            unsafe {
                rusqlite::ffi::sqlite3_wal_hook(
                    conn.handle(),
                    Some(hold_checkpoint),
                    (&mut *hook.1 as *mut Hook).cast(),
                );
            }
            super::super::set_setting(&conn, "checkpoint_fixture", "committed").unwrap();
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let (finished, result) = mpsc::channel();
        let next = std::thread::spawn(move || {
            let conn = open_at(&path).unwrap();
            let outcome = super::super::set_setting(&conn, "next_fixture", "committed");
            let _ = finished.send(outcome);
        });
        let outcome = result.recv_timeout(Duration::from_secs(2));
        resume.send(()).unwrap();
        writer.join().unwrap();
        next.join().unwrap();
        outcome
            .expect("post-commit WAL work blocked another writer")
            .unwrap();
    }
}
