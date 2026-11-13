//! 运行日志：target 使用 Rust 模块路径，常用前缀为 `rrss_core::ingest`、
//! `rrss_app::scheduler`、`rrss_app::pi`、`rrss_ai`。
//!
//! 桌面文件默认 `warn,rrss=info`；stderr 在 Debug 为同一规则，Release 为 `warn`。
//! 工具进程只写 stderr（默认 warn），stdout 专供 JSON。`RRSS_LOG` 优先于 `RUST_LOG`，
//! 追加到各层默认规则之后。文件级别可重载，只影响本次运行。
//! 所有自由文本经 logsafe 单行化与脱敏；正文与模型内容必须在事件源处省略。

pub mod client;
mod command;
#[cfg(test)]
mod tests;
mod writer;
pub use command::CommandLog;

use crate::views::{LogStatusView, LogUnavailableReason};
use rrss_core::logsafe;
use std::{
    fmt, fs, io,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tracing::{
    Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    EnvFilter, Registry,
    fmt::{FormatFields, format::Writer, time::FormatTime},
    prelude::*,
    reload,
};
use writer::FileWriter;
pub use writer::{FILE_BYTES, FILE_COUNT, FileStatus};

pub const DEFAULT_FILTER: &str = "warn,rrss=info";
pub const RELEASE_FILTER: &str = "warn";
/// 单条事件在写入时已封顶，导出时按字符数留足余量。
const EXPORT_LINE_CHARS: usize = 70_000;

pub enum Mode {
    Desktop { log_dir: PathBuf },
    Tool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Default,
    Detailed,
}

struct Control {
    level: Level,
    reload: Box<dyn Fn(EnvFilter) -> Result<(), String> + Send + Sync>,
}

#[derive(Clone)]
pub struct Handle {
    directory: Option<PathBuf>,
    writer: Option<FileWriter>,
    unavailable: Option<LogUnavailableReason>,
    extra: String,
    control: Arc<Mutex<Control>>,
}

static LOGGER: OnceLock<Handle> = OnceLock::new();

pub fn default_filter() -> &'static str {
    if cfg!(debug_assertions) {
        DEFAULT_FILTER
    } else {
        RELEASE_FILTER
    }
}

fn extra_filter(rrss: Option<String>, rust: Option<String>) -> String {
    [rrss, rust]
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn filter_spec(base: &str, extra: &str) -> String {
    if extra.is_empty() {
        base.into()
    } else {
        format!("{base},{extra}")
    }
}

fn filter(base: &str, extra: &str) -> EnvFilter {
    EnvFilter::try_new(filter_spec(base, extra)).unwrap_or_else(|_| {
        // 不回显原始环境变量或解析错误（可能含用户的值）。
        eprintln!("rrss: invalid log filter; falling back to default");
        EnvFilter::new(base)
    })
}

// fmt 层按格式化器类型缓存 span 字段；两层分开，避免 record 时重复追加。
#[derive(Clone, Copy)]
struct SafeFields<const STDERR: bool>;
impl<'writer, const STDERR: bool> FormatFields<'writer> for SafeFields<STDERR> {
    fn format_fields<R: tracing_subscriber::field::RecordFields>(
        &self,
        writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut visitor = SafeVisitor {
            writer,
            first: true,
            result: Ok(()),
        };
        fields.record(&mut visitor);
        visitor.result
    }
}
struct SafeVisitor<'a> {
    writer: Writer<'a>,
    first: bool,
    result: fmt::Result,
}
impl SafeVisitor<'_> {
    fn value(&mut self, field: &Field, value: &str) {
        if self.result.is_err() {
            return;
        }
        let value = logsafe::text(value, 2000);
        self.result = (|| {
            if !self.first {
                self.writer.write_char(' ')?;
            }
            self.first = false;
            if field.name() == "message" {
                write!(self.writer, "{value}")
            } else {
                write!(self.writer, "{}={value}", field.name())
            }
        })();
    }
}
impl Visit for SafeVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.value(field, value);
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.value(field, &format!("{value:?}"));
    }
}
struct LocalTime;
impl FormatTime for LocalTime {
    fn format_time(&self, writer: &mut Writer<'_>) -> fmt::Result {
        write!(writer, "{}", writer::timestamp())
    }
}

/// 单测里常驻一个空分发器。只有一个已注册分发器时，tracing 按「最先触发某条日志的线程」
/// 计算并缓存它的开关；并行的、没装订阅者的测试先触发，会把正在抓日志的测试那条关掉。
#[cfg(test)]
fn keep_test_dispatchers_shared() {
    static PEER: OnceLock<tracing::Dispatch> = OnceLock::new();
    PEER.get_or_init(|| tracing::Dispatch::new(Registry::default()));
}

/// 构造独立订阅者供隔离测试使用；不设置全局订阅者或 panic hook。
/// `extra` 由调用方显式传入，测试不读取开发环境的过滤配置。
pub fn subscriber(mode: Mode, extra: &str) -> (impl Subscriber + Send + Sync, Handle) {
    #[cfg(test)]
    keep_test_dispatchers_shared();
    let desktop = matches!(&mode, Mode::Desktop { .. });
    let (directory, writer, unavailable) = match mode {
        Mode::Tool => (None, None, None),
        Mode::Desktop { log_dir } => match FileWriter::open(&log_dir) {
            Ok(writer) => (Some(log_dir), Some(writer), None),
            Err(error) => (Some(log_dir), None, Some(unavailable_reason(error.kind()))),
        },
    };
    let (file_filter, reload) =
        reload::Layer::<EnvFilter, Registry>::new(filter(DEFAULT_FILTER, extra));
    let file_layer = writer.clone().map(|writer| {
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(false)
            .with_target(true)
            .with_timer(LocalTime)
            .fmt_fields(SafeFields::<false>)
            .compact()
            .with_filter(file_filter)
    });
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .fmt_fields(SafeFields::<true>)
        .compact()
        .with_filter(filter(
            if desktop {
                default_filter()
            } else {
                RELEASE_FILTER
            },
            extra,
        ));
    let subscriber = Registry::default().with(file_layer).with(stderr);
    let handle = Handle {
        directory,
        writer,
        unavailable,
        extra: extra.into(),
        control: Arc::new(Mutex::new(Control {
            level: Level::Default,
            reload: Box::new(move |filter| {
                reload
                    .reload(filter)
                    .map_err(|_| "log filter unavailable".into())
            }),
        })),
    };
    (subscriber, handle)
}

pub fn init(mode: Mode) {
    let extra = extra_filter(
        std::env::var("RRSS_LOG").ok(),
        std::env::var("RUST_LOG").ok(),
    );
    let (subscriber, handle) = subscriber(mode, &extra);
    install(subscriber, handle);
}

fn install(subscriber: impl Subscriber + Send + Sync + 'static, handle: Handle) {
    subscriber.init(); // 保留 tracing-log 桥。
    handle.start();
    if let Some(writer) = handle.writer.clone() {
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic");
            let location = info
                .location()
                .map(|location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                })
                .unwrap_or_default();
            writer.panic(&format!(
                "panic thread={:?} location={:?} error={:?}",
                logsafe::text(thread.name().unwrap_or("unnamed"), 80),
                logsafe::text(&location, 200),
                logsafe::text(message, 1000)
            ));
            original(info);
        }));
    }
    let _ = LOGGER.set(handle);
}

/// 路径解析失败同样保留 stderr；不能阻止用户打开应用修复问题。
pub fn init_desktop() {
    match rrss_core::paths::log_dir() {
        Ok(log_dir) => init(Mode::Desktop { log_dir }),
        Err(_) => {
            let extra = extra_filter(
                std::env::var("RRSS_LOG").ok(),
                std::env::var("RUST_LOG").ok(),
            );
            let (subscriber, mut handle) =
                subscriber(Mode::Tool, &filter_spec(default_filter(), &extra));
            handle.unavailable = Some(LogUnavailableReason::NotWritable);
            install(subscriber, handle);
        }
    }
}

pub fn current() -> Option<&'static Handle> {
    LOGGER.get()
}

/// 平台差异由标准库的错误种类统一（Windows 的磁盘已满同样映射为 `StorageFull`）。
fn unavailable_reason(kind: io::ErrorKind) -> LogUnavailableReason {
    match kind {
        io::ErrorKind::WouldBlock => LogUnavailableReason::OtherInstance,
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => LogUnavailableReason::DiskFull,
        _ => LogUnavailableReason::NotWritable,
    }
}

/// 诊断包默认文件名，带本地时间，便于区分多次导出。
pub fn default_export_name() -> String {
    format!(
        "rrss-diagnostics-{}.zip",
        chrono::Local::now().format("%Y%m%d-%H%M")
    )
}

/// 诊断包：日志逐行再脱敏一遍，另附不含路径的 `diagnostics.json`。
pub fn export_diagnostics(db_path: &Path, dest: &Path) -> io::Result<()> {
    let handle = current().ok_or_else(|| io::Error::other("logging is not initialized"))?;
    let log = handle.status_view();
    let diagnostics = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "os": format!("{:?}", rrss_core::platform::Os::current()),
        "arch": format!("{:?}", rrss_core::platform::Arch::current()),
        "schema": rrss_core::SCHEMA_VERSION,
        "exported_at": writer::timestamp(),
        "log": log.map(|log| serde_json::json!({
            "unavailable": log.unavailable,
            "total_bytes": log.total_bytes,
            "detailed": log.detailed,
            "previous_unclean": log.previous_unclean,
        })),
        "scheduler": crate::scheduler::Scheduler::diagnostics_for(db_path),
    });
    handle.export(dest, &diagnostics)
}
pub fn end_session() {
    if let Some(handle) = current() {
        handle.end();
    }
}

/// anyhow 的外层 context 不应把模型或解析正文重新带回日志。
pub fn error(value: &(dyn std::error::Error + 'static)) -> String {
    let mut next = Some(value);
    while let Some(cause) = next {
        if let Some(failure) = cause.downcast_ref::<crate::scheduler::digest_jobs::Failure>() {
            return failure.reason.into();
        }
        if let Some(ai) = cause.downcast_ref::<rrss_ai::Error>() {
            return ai.log_message();
        }
        if let Some(core) = cause.downcast_ref::<rrss_core::Error>() {
            return logsafe::error(core);
        }
        next = cause.source();
    }
    // 其余错误保留原因链（同 `{:#}`），否则外层 context 会盖住真正的失败原因。
    let mut text = value.to_string();
    let mut next = value.source();
    for _ in 0..8 {
        let Some(cause) = next else { break };
        let message = cause.to_string();
        if !text.contains(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        next = cause.source();
    }
    logsafe::text(&text, 500)
}

impl Handle {
    pub fn start(&self) {
        if let Some(reason) = self.unavailable {
            eprintln!(
                "rrss: log file unavailable: {}",
                match reason {
                    LogUnavailableReason::OtherInstance =>
                        "another rrss instance owns the log file",
                    LogUnavailableReason::NotWritable => "log directory is not writable",
                    LogUnavailableReason::DiskFull => "disk is full",
                }
            );
        }
        if let Some(writer) = &self.writer {
            let previous = writer.status().previous_unclean;
            // 会话标记绕过用户过滤，避免 off / warn 使正常退出被误判。
            writer.lifecycle("INFO", &format!("session started version={} build={} os={:?} arch={:?} schema={} pid={} filter={:?}",
                env!("CARGO_PKG_VERSION"), if cfg!(debug_assertions) { "debug" } else { "release" },
                rrss_core::platform::Os::current(), rrss_core::platform::Arch::current(),
                rrss_core::SCHEMA_VERSION, std::process::id(), logsafe::text(&filter_spec(DEFAULT_FILTER, &self.extra), 300)));
            if let Some(time) = previous {
                writer.lifecycle("WARN", &format!("previous session did not exit cleanly last_event={time} error_kind=unclean_exit"));
            }
        }
    }
    pub fn end(&self) {
        if let Some(writer) = &self.writer {
            writer.end();
        }
    }
    pub fn status(&self) -> Option<FileStatus> {
        self.writer.as_ref().map(FileWriter::status)
    }
    pub fn unavailable_reason(&self) -> Option<LogUnavailableReason> {
        self.unavailable
    }
    /// 诊断页与存储页的状态，只读文件大小，不读日志内容。
    pub fn status_view(&self) -> Option<LogStatusView> {
        let directory = self.directory.as_ref()?;
        let status = self.status();
        let (current, total) = writer::sizes(directory);
        Some(LogStatusView {
            directory: directory.display().to_string(),
            file: directory.join("rrss.log").display().to_string(),
            unavailable: self
                .unavailable
                .or_else(|| status.as_ref()?.failure.map(unavailable_reason)),
            current_size: crate::queries::fmt_bytes(current as i64),
            total_size: crate::queries::fmt_bytes(total as i64),
            total_bytes: total as i64,
            detailed: self.level() == Level::Detailed,
            previous_unclean: status.and_then(|status| status.previous_unclean),
            can_clear: self.writer.is_some(),
        })
    }
    /// 只有持锁的进程能清空；另一个窗口在记录时拒绝。
    pub fn clear(&self) -> io::Result<()> {
        self.writer
            .as_ref()
            .ok_or_else(|| io::Error::other("log files are owned by another process"))?
            .clear()
    }
    /// 写到用户选择的位置；失败时删掉写了一半的文件。
    pub fn export(&self, dest: &Path, diagnostics: &serde_json::Value) -> io::Result<()> {
        let directory = self
            .directory
            .as_ref()
            .ok_or_else(|| io::Error::other("log directory unavailable"))?;
        let result = (|| {
            let mut zip = zip::ZipWriter::new(fs::File::create(dest)?);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for path in writer::files(directory, FILE_COUNT) {
                let bytes = match fs::read(&path) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                zip.start_file(name.into_owned(), options)
                    .map_err(io::Error::other)?;
                for line in String::from_utf8_lossy(&bytes).lines() {
                    writeln!(zip, "{}", logsafe::text(line, EXPORT_LINE_CHARS))?;
                }
            }
            zip.start_file("diagnostics.json", options)
                .map_err(io::Error::other)?;
            serde_json::to_writer_pretty(&mut zip, diagnostics).map_err(io::Error::other)?;
            zip.finish().map_err(io::Error::other)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(dest);
        }
        result
    }
    pub fn level(&self) -> Level {
        self.control.lock().unwrap_or_else(|e| e.into_inner()).level
    }
    pub fn set_level(&self, level: Level) -> Result<(), String> {
        if self.writer.is_none() {
            return Err("log file unavailable".into());
        }
        let mut control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        let base = if level == Level::Detailed {
            "warn,rrss=debug"
        } else {
            DEFAULT_FILTER
        };
        (control.reload)(filter(base, &self.extra))?;
        control.level = level;
        Ok(())
    }
}
