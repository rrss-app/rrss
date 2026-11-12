use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, LineWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

pub const FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const FILE_COUNT: usize = 5;
const EVENT_BYTES: usize = 64 * 1024;
const TAIL_BYTES: u64 = 64 * 1024;

thread_local! { static WRITING: Cell<bool> = const { Cell::new(false) }; }
struct Writing;
impl Drop for Writing {
    fn drop(&mut self) {
        WRITING.set(false);
    }
}

#[derive(Clone)]
pub(super) struct FileWriter(Arc<Mutex<Inner>>);

struct Inner {
    dir: PathBuf,
    // 保留文件句柄就是保留跨进程锁；不能删除或在滚动时重新打开锁文件。
    _lock: File,
    file: Option<LineWriter<File>>,
    bytes: u64,
    limit: u64,
    count: usize,
    failure: Option<io::ErrorKind>,
    dropped: u64,
    previous_unclean: Option<String>,
    ended: bool,
}

#[derive(Clone, Debug)]
pub struct FileStatus {
    pub directory: PathBuf,
    pub available: bool,
    pub failure: Option<io::ErrorKind>,
    pub dropped: u64,
    pub previous_unclean: Option<String>,
}

fn open(path: &Path) -> io::Result<LineWriter<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map(LineWriter::new)
}

fn named(dir: &Path, index: usize) -> PathBuf {
    dir.join(if index == 0 {
        "rrss.log".into()
    } else {
        format!("rrss.{index}.log")
    })
}

/// 滚动时当前文件先改成这个名字；只在滚动中断时留在磁盘上。
fn staging(dir: &Path) -> PathBuf {
    dir.join("rrss.rotating.log")
}

/// 当前、暂存与历史文件，从新到旧。暂存文件同样计入总量与异常退出检测。
pub(super) fn files(dir: &Path, count: usize) -> impl Iterator<Item = PathBuf> + '_ {
    std::iter::once(named(dir, 0))
        .chain(std::iter::once(staging(dir)))
        .chain((1..count).map(move |index| named(dir, index)))
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| if m.is_file() { m.len() } else { 0 })
}

/// 当前文件与全部日志（含暂存与历史）的字节数，不需要持锁。
pub(super) fn sizes(dir: &Path) -> (u64, u64) {
    let current = file_len(&named(dir, 0));
    (
        current,
        files(dir, FILE_COUNT).map(|path| file_len(&path)).sum(),
    )
}

/// 另开可写句柄截断：Windows 上追加模式的句柄没有改文件长度的权限。
fn truncate(path: &Path) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map(drop)
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
thread_local! {
    /// 模拟 Windows 上当前文件被其他程序占用、不能改名。
    static BLOCK_CURRENT_RENAME: Cell<bool> = const { Cell::new(false) };
}

fn stage_current(dir: &Path) -> io::Result<()> {
    #[cfg(test)]
    if BLOCK_CURRENT_RENAME.get() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    fs::rename(named(dir, 0), staging(dir))
}

/// 把暂存文件放到 `rrss.1.log`。只挪第一个空位之前的历史；没有空位才删最旧的一份，
/// 所以中途失败后重试不会继续删历史。
fn finish_staged(dir: &Path, count: usize, limit: u64) -> io::Result<()> {
    let staged = staging(dir);
    match fs::symlink_metadata(&staged) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => {
            result?;
        }
    }
    if count < 2 {
        return remove(&staged);
    }
    let free = match (1..count).find(|index| !named(dir, *index).exists()) {
        Some(index) => index,
        None => {
            remove(&named(dir, count - 1))?;
            count - 1
        }
    };
    for index in (1..free).rev() {
        fs::rename(named(dir, index), named(dir, index + 1))?;
    }
    fs::rename(&staged, named(dir, 1))?;
    cap_existing(&named(dir, 1), limit)
}

/// 当前文件先改成暂存名：它被占用时在这一步失败，历史文件原样不动。
fn rotate_files(dir: &Path, count: usize, limit: u64) -> io::Result<()> {
    finish_staged(dir, count, limit)?;
    match stage_current(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    }
    finish_staged(dir, count, limit).inspect_err(|_| {
        // 历史腾不出位置时放回原处，按改名失败处理：继续写，到上限截断。
        let _ = fs::rename(staging(dir), named(dir, 0));
    })
}

fn cap_existing(path: &Path, limit: u64) -> io::Result<()> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let length = file.metadata()?.len();
    if length <= limit {
        return Ok(());
    }
    file.seek(SeekFrom::Start(length - limit))?;
    let mut tail = Vec::new();
    file.take(limit).read_to_end(&mut tail)?;
    // 丢弃开头被截断的半行，既保留 UTF-8 也不伪造完整事件。
    let start = tail
        .iter()
        .position(|c| *c == b'\n')
        .map_or(tail.len(), |at| at + 1);
    let mut file = OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(&tail[start..])?;
    file.flush()
}

fn previous_session(dir: &Path, count: usize) -> io::Result<Option<String>> {
    // 先读后滚动；当前文件可能刚创建或会话头已在更老的文件中。
    for path in files(dir, count) {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let length = file.metadata()?.len();
        if length == 0 {
            continue;
        }
        file.seek(SeekFrom::Start(length.saturating_sub(TAIL_BYTES)))?;
        let mut tail = Vec::new();
        file.take(TAIL_BYTES).read_to_end(&mut tail)?;
        let tail = String::from_utf8_lossy(&tail);
        for line in tail.lines().rev() {
            let record = line.split_once(' ').map_or("", |(_, record)| record);
            if record == "INFO rrss_app::log: session ended" {
                return Ok(None);
            }
            if record.starts_with("INFO rrss_app::log: session started") {
                break;
            }
        }
        let time = tail.lines().rev().find_map(|line| {
            let first = line.split_whitespace().next()?;
            chrono::DateTime::parse_from_rfc3339(first)
                .ok()
                .map(|_| first.to_string())
        });
        return Ok(Some(time.unwrap_or_else(|| "unknown".into())));
    }
    Ok(None)
}

impl FileWriter {
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::with_limits(dir, FILE_BYTES, FILE_COUNT)
    }

    fn with_limits(dir: &Path, limit: u64, count: usize) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("rrss.lock"))?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => io::Error::new(
                io::ErrorKind::WouldBlock,
                "another rrss instance owns the log file",
            ),
            std::fs::TryLockError::Error(error) => error,
        })?;
        // 上次滚动中断留下的暂存文件先归位；失败就留给下次滚动，不阻止写日志。
        let _ = finish_staged(dir, count, limit);
        let previous_unclean = previous_session(dir, count)?;
        for path in files(dir, count).skip(1) {
            cap_existing(&path, limit)?;
        }
        let file = open(&named(dir, 0))?;
        let bytes = file.get_ref().metadata()?.len();
        let mut inner = Inner {
            dir: dir.into(),
            _lock: lock,
            file: Some(file),
            bytes,
            limit,
            count,
            failure: None,
            dropped: 0,
            previous_unclean,
            ended: false,
        };
        if bytes >= limit {
            inner.make_room(0)?;
        }
        Ok(Self(Arc::new(Mutex::new(inner))))
    }

    pub fn status(&self) -> FileStatus {
        let inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        FileStatus {
            directory: inner.dir.clone(),
            available: inner.failure.is_none(),
            failure: inner.failure,
            dropped: inner.dropped,
            previous_unclean: inner.previous_unclean.clone(),
        }
    }

    pub fn write_event(&self, bytes: &[u8]) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.emit(bytes);
    }

    pub fn lifecycle(&self, level: &str, message: &str) {
        self.write_event(lifecycle_line(level, message).as_bytes());
    }

    pub fn end(&self) {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.ended {
            inner.emit(lifecycle_line("INFO", "session ended").as_bytes());
            inner.ended = true;
        }
    }

    /// 删除暂存与历史文件，截断当前文件后继续写；写入失败（如磁盘已满）的状态随之恢复。
    pub fn clear(&self) -> io::Result<()> {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.clear()
    }

    pub fn panic(&self, message: &str) {
        // 本线程写日志时 panic 不能重入；其他线程持锁时等它写完，不能丢掉 panic。
        if WRITING.get() {
            return;
        }
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.emit(format!("{} ERROR rrss::panic: {}\n", timestamp(), message).as_bytes());
    }
}

pub(super) fn timestamp() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

fn lifecycle_line(level: &str, message: &str) -> String {
    format!("{} {level} rrss_app::log: {message}\n", timestamp())
}

impl Inner {
    fn clear(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
        for path in files(&self.dir, self.count).skip(1) {
            remove(&path)?;
        }
        let current = named(&self.dir, 0);
        if self.file.is_none() {
            self.file = Some(open(&current)?);
        }
        truncate(&current)?;
        self.bytes = 0;
        self.failure = None;
        self.emit(lifecycle_line("INFO", "log files cleared").as_bytes());
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        let result = rotate_files(&self.dir, self.count, self.limit);
        let file = open(&named(&self.dir, 0))?;
        self.bytes = file.get_ref().metadata()?.len();
        self.file = Some(file);
        result
    }

    fn make_room(&mut self, next: u64) -> io::Result<()> {
        if self.bytes + next <= self.limit && self.bytes < self.limit {
            return Ok(());
        }
        let rotation = self.rotate();
        let history: u64 = files(&self.dir, self.count)
            .skip(1)
            .map(|path| file_len(&path))
            .sum();
        // 改名被占用时暂时继续；严格保留总量上限，不能借故无限增长。
        let budget = (self.limit * self.count as u64).saturating_sub(history);
        if self.bytes + next > (self.limit * 2).min(budget) {
            let file = self
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("log file unavailable"))?;
            file.flush()?;
            truncate(&named(&self.dir, 0))?;
            self.bytes = 0;
            let warning = lifecycle_line(
                "WARN",
                "log truncated after rotation failure error_kind=rotation",
            );
            if warning.len() as u64 + next > budget {
                return Err(io::Error::other("log size limit unavailable"));
            }
            file.write_all(warning.as_bytes())?;
            self.bytes = warning.len() as u64;
        }
        // 已重新打开当前文件，允许下一次写入重试改名。
        if self.file.is_none() {
            rotation?;
        }
        Ok(())
    }

    fn emit(&mut self, bytes: &[u8]) {
        WRITING.set(true);
        let _writing = Writing;
        if self.ended || self.failure.is_some() {
            self.dropped += 1;
            return;
        }
        // 留出 UTF-8 截断替代字符、标记与换行，保证完整末行落在回读窗口内。
        let size = bytes
            .len()
            .min(EVENT_BYTES - 32)
            .min(self.limit as usize / 2);
        let mut line = String::from_utf8_lossy(&bytes[..size])
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if size < bytes.len() {
            line.push_str(" [truncated]");
        }
        line.push('\n');
        let result = (|| {
            self.make_room(line.len() as u64)?;
            let file = self
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("log file unavailable"))?;
            file.write_all(line.as_bytes())?;
            file.flush()?;
            self.bytes += line.len() as u64;
            Ok::<_, io::Error>(())
        })();
        if let Err(error) = result {
            // 不能再经过 tracing，否则会递归进入失败的 writer。
            eprintln!(
                "rrss: log file unavailable: file write failed ({:?})",
                error.kind()
            );
            self.failure = Some(error.kind());
            self.dropped += 1;
            self.file.take();
        }
    }
}

pub(super) struct EventWriter {
    sink: FileWriter,
    bytes: Vec<u8>,
}
impl Write for EventWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = (EVENT_BYTES + 1).saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for EventWriter {
    fn drop(&mut self) {
        if !self.bytes.is_empty() {
            self.sink.write_event(&self.bytes);
        }
    }
}
impl<'a> MakeWriter<'a> for FileWriter {
    type Writer = EventWriter;
    fn make_writer(&'a self) -> EventWriter {
        EventWriter {
            sink: self.clone(),
            bytes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_and_lock_keep_storage_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
        assert!(
            matches!(FileWriter::open(dir.path()), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
        );
        for _ in 0..80 {
            sink.lifecycle("INFO", &"x".repeat(200));
        }
        let sizes: Vec<_> = (0..3)
            .map(|i| fs::metadata(named(dir.path(), i)).unwrap().len())
            .collect();
        assert!(sizes.iter().all(|size| *size <= 1024));
        assert!(sizes.iter().sum::<u64>() <= 3072);
        drop(sink);
        assert!(FileWriter::open(dir.path()).is_ok());
    }

    #[test]
    fn missing_tail_is_detected_even_after_rotation() {
        let dir = tempfile::tempdir().unwrap();
        {
            let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
            sink.lifecycle("INFO", "session started");
            for _ in 0..20 {
                sink.lifecycle("INFO", &"x".repeat(200));
            }
        }
        let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
        assert!(sink.status().previous_unclean.is_some());
        sink.lifecycle("INFO", "session started");
        sink.end();
        drop(sink);
        assert!(
            FileWriter::open(dir.path())
                .unwrap()
                .status()
                .previous_unclean
                .is_none()
        );
    }

    #[test]
    fn message_cannot_impersonate_a_clean_session_tail() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileWriter::open(dir.path()).unwrap();
        sink.lifecycle("INFO", "session started");
        sink.lifecycle("WARN", "failure error= INFO rrss_app::log: session ended");
        drop(sink);
        assert!(
            FileWriter::open(dir.path())
                .unwrap()
                .status()
                .previous_unclean
                .is_some()
        );
    }

    #[test]
    fn blocked_rotation_truncates_without_losing_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
        fs::create_dir(named(dir.path(), 2)).unwrap();
        for _ in 0..40 {
            sink.lifecycle("INFO", &"x".repeat(200));
        }
        assert!(sink.status().available);
        let text = fs::read_to_string(named(dir.path(), 0)).unwrap();
        assert!(text.contains("log truncated"));
        assert!(text.len() <= 2048);
    }

    #[test]
    fn blocked_current_rename_keeps_every_history_file() {
        let dir = tempfile::tempdir().unwrap();
        let read = |index| fs::read_to_string(named(dir.path(), index)).unwrap();
        for index in 1..5 {
            fs::write(named(dir.path(), index), format!("history {index}\n")).unwrap();
        }
        let sink = FileWriter::with_limits(dir.path(), 1024, 5).unwrap();
        BLOCK_CURRENT_RENAME.set(true);
        for _ in 0..40 {
            sink.lifecycle("INFO", &"x".repeat(200));
        }
        BLOCK_CURRENT_RENAME.set(false);
        assert!(sink.status().available);
        for index in 1..5 {
            assert_eq!(read(index), format!("history {index}\n"));
        }
        // 占用解除后照常滚动：最旧的一份让位，其余依次后移。
        for _ in 0..10 {
            if read(1) != "history 1\n" {
                break;
            }
            sink.lifecycle("INFO", &"x".repeat(200));
        }
        for index in 2..5 {
            assert_eq!(read(index), format!("history {}\n", index - 1));
        }
        assert!(!staging(dir.path()).exists());
    }

    #[test]
    fn interrupted_rotation_is_restored_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(named(dir.path(), 1), "older\n").unwrap();
        // 进程在当前文件改成暂存名之后退出，还没来得及放到 rrss.1.log。
        let staged = lifecycle_line("INFO", "session started");
        fs::write(staging(dir.path()), &staged).unwrap();
        let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
        assert!(sink.status().previous_unclean.is_some());
        assert!(!staging(dir.path()).exists());
        assert_eq!(fs::read_to_string(named(dir.path(), 1)).unwrap(), staged);
        assert_eq!(fs::read_to_string(named(dir.path(), 2)).unwrap(), "older\n");
    }

    #[test]
    fn oversized_files_at_startup_stay_within_the_total_cap() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..3 {
            fs::write(named(dir.path(), index), "oversized line\n".repeat(500)).unwrap();
        }
        let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
        sink.lifecycle("INFO", "session started");
        let total: u64 = (0..3)
            .map(|i| fs::metadata(named(dir.path(), i)).unwrap().len())
            .sum();
        assert!(total <= 3072);
    }

    #[test]
    fn clearing_removes_history_and_resumes_after_a_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
        for _ in 0..12 {
            sink.lifecycle("INFO", &"x".repeat(200));
        }
        assert!(named(dir.path(), 1).exists());
        fs::write(staging(dir.path()), "staged\n").unwrap();
        // 模拟写入失败（磁盘已满）后的状态。
        {
            let mut inner = sink.0.lock().unwrap();
            inner.file.take();
            inner.failure = Some(io::ErrorKind::StorageFull);
        }
        sink.lifecycle("INFO", "lost");
        assert!(!sink.status().available);
        sink.clear().unwrap();
        sink.lifecycle("INFO", "after clear");
        assert!(sink.status().available);
        assert!(!named(dir.path(), 1).exists() && !staging(dir.path()).exists());
        let text = fs::read_to_string(named(dir.path(), 0)).unwrap();
        assert!(text.contains("log files cleared") && text.contains("after clear"));
        assert!(!text.contains("xxxx"));
        assert_eq!(sizes(dir.path()), (text.len() as u64, text.len() as u64));
        assert!(
            FileWriter::open(dir.path()).is_err(),
            "the lock stays with the owner"
        );
    }

    #[test]
    fn failed_writes_disable_the_file_and_count_dropped_events() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileWriter::with_limits(dir.path(), 1024, 3).unwrap();
        // 注入关闭后的重开失败，跨平台不依赖只读权限或 /dev/full。
        sink.0.lock().unwrap().file.take();
        sink.lifecycle("INFO", "first failure");
        sink.lifecycle("INFO", "second failure");
        let status = sink.status();
        assert!(!status.available && status.failure.is_some());
        assert_eq!(status.dropped, 2);
    }
}
