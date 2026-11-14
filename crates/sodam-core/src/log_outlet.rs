//! 诊断日志出口：本机文件、5 MB 轮转、保留 5 份、写盘失败降级到内存。
//!
//! 纯本地播放器不再有网络请求，所以这里只保留本地日志需要的东西：
//! 级别/话题过滤、单行 key=value、文件轮转、清空、退出前 flush。
//! 时间戳用 UTC（`...Z`），不再做跨平台时区 FFI。
//!
//! 安全约束仍然保留：键名像 Cookie / Authorization / token / password 的字段
//! 直接丢弃，不写进日志。

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;
pub const DEFAULT_MAX_OLD_FILES: usize = 5;
pub const LOG_FILE_NAME: &str = "sodam.log";
pub const MEMORY_BUFFER_LIMIT: usize = 500;

/// 旧的环境变量开关：存在且不是 `0` 时，把所属话题提到 DEBUG。
pub const TOPIC_VERBOSITY_ENV: [(&str, LogTopic); 2] = [
    ("SODAM_PLAYER_LOG", LogTopic::Player),
    ("SODAM_FRAME_LOG", LogTopic::App),
];

const NO_TOPIC_OVERRIDE: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl LogLevel {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

fn level_from_u8(value: u8) -> LogLevel {
    match value {
        value if value == LogLevel::Warn as u8 => LogLevel::Warn,
        value if value == LogLevel::Error as u8 => LogLevel::Error,
        value if value == LogLevel::Debug as u8 => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogTopic {
    Player,
    Audio,
    App,
}

impl LogTopic {
    pub const ALL: [LogTopic; 3] = [LogTopic::Player, LogTopic::Audio, LogTopic::App];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Audio => "audio",
            Self::App => "app",
        }
    }

    pub const fn index(&self) -> usize {
        match self {
            Self::Player => 0,
            Self::Audio => 1,
            Self::App => 2,
        }
    }
}

/// 日志目录：`SODAM_LOG_DIR` 优先，否则用平台数据目录下的 `sodam/logs`。
pub fn log_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("SODAM_LOG_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    default_log_dir()
}

fn default_log_dir() -> PathBuf {
    crate::app_data_root().join("logs")
}

/// 一条结构化日志。
pub struct LogRecord<'a> {
    pub timestamp: SystemTime,
    pub level: LogLevel,
    pub topic: LogTopic,
    pub event: &'a str,
    pub fields: Vec<(&'a str, String)>,
}

impl<'a> LogRecord<'a> {
    pub fn new(level: LogLevel, topic: LogTopic, event: &'a str) -> Self {
        Self {
            timestamp: SystemTime::now(),
            level,
            topic,
            event,
            fields: Vec::new(),
        }
    }

    pub fn field(mut self, key: &'a str, value: impl ToString) -> Self {
        if is_sensitive_key(key) {
            return self;
        }
        self.fields.push((key, value.to_string()));
        self
    }

    pub fn format_line(&self) -> String {
        let mut line = format!(
            "{} {} {} {}",
            format_timestamp(self.timestamp),
            self.level.as_str(),
            self.topic.as_str(),
            self.event
        );
        for (key, value) in &self.fields {
            line.push(' ');
            line.push_str(&format_field(key, value));
        }
        line
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["cookie", "authorization", "token", "password", "secret"]
        .iter()
        .any(|needle| key.contains(needle))
}

fn format_timestamp(now: SystemTime) -> String {
    let duration = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        day_secs / 3600,
        (day_secs % 3600) / 60,
        day_secs % 60,
        duration.subsec_millis()
    )
}

/// Howard Hinnant 民用历算法：UNIX 天数 → 年月日。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year as i32, month, day)
}

fn escape_field_value(value: &str) -> String {
    if !value.is_empty() && !value.contains([' ', '\t', '"', '\n', '\r', '=']) {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn format_field(key: &str, value: &str) -> String {
    format!("{key}={}", escape_field_value(value))
}

enum WriterMsg {
    Line(String),
    Flush(Sender<()>),
    Shutdown,
}

struct WriterState {
    dir: PathBuf,
    file: Option<File>,
    max_file_size: u64,
    max_old_files: usize,
    memory: Arc<Mutex<VecDeque<String>>>,
}

impl WriterState {
    fn open(&mut self) -> std::io::Result<File> {
        fs::create_dir_all(&self.dir)?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(LOG_FILE_NAME))
    }

    fn fallback(&self, line: String) {
        let mut memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        if memory.len() >= MEMORY_BUFFER_LIMIT {
            memory.pop_front();
        }
        memory.push_back(line);
    }

    fn write(&mut self, line: String) {
        if self.file.is_none() {
            match self.open() {
                Ok(file) => self.file = Some(file),
                Err(_) => return self.fallback(line),
            }
        }
        let size = self
            .file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map(|meta| meta.len())
            .unwrap_or(0);
        if size + line.len() as u64 + 1 > self.max_file_size {
            self.rotate();
        }
        let Some(file) = self.file.as_mut() else {
            return self.fallback(line);
        };
        if writeln!(file, "{line}").is_err() {
            self.file = None;
            self.fallback(line);
        }
    }

    fn rotate(&mut self) {
        self.file = None;
        let base = self.dir.join(LOG_FILE_NAME);
        let backup = |index: usize| self.dir.join(format!("{LOG_FILE_NAME}.{index}"));
        let _ = fs::remove_file(backup(self.max_old_files));
        for index in (1..self.max_old_files).rev() {
            let _ = fs::rename(backup(index), backup(index + 1));
        }
        let _ = fs::rename(&base, backup(1));
        match self.open() {
            Ok(file) => self.file = Some(file),
            Err(_) => self.file = None,
        }
    }

    fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
    }
}

fn writer_loop(rx: Receiver<WriterMsg>, mut state: WriterState) {
    while let Ok(message) = rx.recv() {
        match message {
            WriterMsg::Line(line) => state.write(line),
            WriterMsg::Flush(ack) => {
                state.flush();
                let _ = ack.send(());
            }
            WriterMsg::Shutdown => break,
        }
    }
    state.flush();
}

/// 日志出口：UI 侧只投递，文件 IO 在专属线程。
pub struct LogOutlet {
    level: AtomicU8,
    detailed: AtomicBool,
    topic_levels: [AtomicU8; LogTopic::ALL.len()],
    tx: Mutex<Option<Sender<WriterMsg>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
    memory: Arc<Mutex<VecDeque<String>>>,
}

impl LogOutlet {
    pub fn new(log_dir: PathBuf) -> Self {
        Self::new_with_limits(log_dir, DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_OLD_FILES)
    }

    pub fn new_with_limits(log_dir: PathBuf, max_file_size: u64, max_old_files: usize) -> Self {
        let memory = Arc::new(Mutex::new(VecDeque::new()));
        let state = WriterState {
            dir: log_dir,
            file: None,
            max_file_size,
            max_old_files,
            memory: memory.clone(),
        };
        let (tx, rx) = mpsc::channel();
        let writer = std::thread::Builder::new()
            .name("sodam-log-writer".to_string())
            .spawn(move || writer_loop(rx, state))
            .ok();
        Self {
            level: AtomicU8::new(LogLevel::Info as u8),
            detailed: AtomicBool::new(false),
            topic_levels: std::array::from_fn(|_| AtomicU8::new(NO_TOPIC_OVERRIDE)),
            tx: Mutex::new(writer.as_ref().map(|_| tx)),
            writer: Mutex::new(writer),
            memory,
        }
    }

    pub fn level(&self) -> LogLevel {
        level_from_u8(self.level.load(Ordering::Relaxed))
    }

    /// 话题覆盖只会「多记」，不会比全局或详细开关更安静。
    pub fn set_topic_level(&self, topic: LogTopic, level: LogLevel) {
        self.topic_levels[topic.index()].store(level as u8, Ordering::Relaxed);
    }

    pub fn topic_level(&self, topic: LogTopic) -> LogLevel {
        let mut level = self.level();
        if self.detailed() {
            level = level.min(LogLevel::Debug);
        }
        let raw = self.topic_levels[topic.index()].load(Ordering::Relaxed);
        if raw != NO_TOPIC_OVERRIDE {
            level = level.min(level_from_u8(raw));
        }
        level
    }

    pub fn set_detailed(&self, detailed: bool) {
        self.detailed.store(detailed, Ordering::Relaxed);
    }

    pub fn detailed(&self) -> bool {
        self.detailed.load(Ordering::Relaxed)
    }

    pub fn apply_env_verbosity(&self) {
        for (name, topic) in TOPIC_VERBOSITY_ENV {
            let value = std::env::var(name).unwrap_or_default();
            if !value.trim().is_empty() && value.trim() != "0" {
                self.set_topic_level(topic, LogLevel::Debug);
            }
        }
    }

    pub fn enabled(&self, level: LogLevel, topic: LogTopic) -> bool {
        level >= self.topic_level(topic)
    }

    pub fn log(&self, level: LogLevel, topic: LogTopic, event: &str, fields: &[(&str, &str)]) {
        if !self.enabled(level, topic) {
            return;
        }
        let mut record = LogRecord::new(level, topic, event);
        for (key, value) in fields {
            record = record.field(key, *value);
        }
        self.record(&record);
    }

    pub fn record(&self, record: &LogRecord<'_>) {
        if self.enabled(record.level, record.topic) {
            self.write_line(&record.format_line());
        }
    }

    /// 单行写入；内部换行会被转义，保证一条记录只占一行。
    pub fn write_line(&self, line: &str) {
        let line = line.replace("\r\n", "\\n").replace(['\n', '\r'], "\\n");
        let sent = self
            .tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|tx| tx.send(WriterMsg::Line(line.clone())).is_ok());
        if !sent {
            let mut memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
            if memory.len() >= MEMORY_BUFFER_LIMIT {
                memory.pop_front();
            }
            memory.push_back(line);
        }
    }

    pub fn record_startup(&self) {
        self.log(
            LogLevel::Info,
            LogTopic::App,
            "startup",
            &[
                ("version", env!("CARGO_PKG_VERSION")),
                ("os", std::env::consts::OS),
                ("arch", std::env::consts::ARCH),
            ],
        );
        self.flush();
    }

    /// 刷新到文件；返回时此前投递的行都已处理。
    pub fn flush(&self) {
        let ack = {
            let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
            let Some(tx) = guard.as_ref() else {
                return;
            };
            let (ack_tx, ack_rx) = mpsc::channel();
            if tx.send(WriterMsg::Flush(ack_tx)).is_err() {
                return;
            }
            ack_rx
        };
        let _ = ack.recv_timeout(std::time::Duration::from_secs(2));
    }
}

impl Drop for LogOutlet {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(WriterMsg::Shutdown);
        }
        if let Some(writer) = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = writer.join();
        }
    }
}

static GLOBAL_OUTLET: OnceLock<LogOutlet> = OnceLock::new();
static PANIC_HOOK_SET: OnceLock<()> = OnceLock::new();

fn ensure_panic_hook_registered() {
    PANIC_HOOK_SET.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            flush_global();
            previous(info);
        }));
    });
}

pub fn init() -> &'static LogOutlet {
    init_with_dir(log_dir())
}

pub fn init_with_dir(dir: PathBuf) -> &'static LogOutlet {
    ensure_panic_hook_registered();
    GLOBAL_OUTLET.get_or_init(|| {
        let outlet = LogOutlet::new(dir);
        outlet.apply_env_verbosity();
        outlet.record_startup();
        outlet
    })
}

pub fn global() -> Option<&'static LogOutlet> {
    GLOBAL_OUTLET.get()
}

pub fn flush_global() {
    if let Some(outlet) = global() {
        outlet.flush();
    }
}

pub fn set_global_detailed(detailed: bool) {
    if let Some(outlet) = global() {
        outlet.set_detailed(detailed);
    }
}

pub fn log_event(level: LogLevel, topic: LogTopic, event: &str, fields: &[(&str, &str)]) {
    if let Some(outlet) = global() {
        outlet.log(level, topic, event, fields);
    }
}

pub fn enabled(level: LogLevel, topic: LogTopic) -> bool {
    global().is_some_and(|outlet| outlet.enabled(level, topic))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_outlet(max_file_size: u64, max_old_files: usize) -> (tempfile::TempDir, LogOutlet) {
        let dir = tempfile::tempdir().expect("temp dir");
        let outlet =
            LogOutlet::new_with_limits(dir.path().to_path_buf(), max_file_size, max_old_files);
        (dir, outlet)
    }

    #[test]
    fn levels_have_stable_labels_and_order() {
        assert_eq!(LogLevel::Error.as_str(), "ERROR");
        assert_eq!(LogLevel::Warn.as_str(), "WARN");
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Warn < LogLevel::Error);
    }

    #[test]
    fn topics_have_stable_labels_and_indices() {
        assert_eq!(LogTopic::Player.as_str(), "player");
        assert_eq!(LogTopic::ALL.len(), 3);
        assert_eq!(LogTopic::App.index(), 2);
    }

    #[test]
    fn timestamp_is_utc_iso_with_millis() {
        let timestamp = UNIX_EPOCH + std::time::Duration::from_millis(1_000);
        assert_eq!(format_timestamp(timestamp), "1970-01-01T00:00:01.000Z");
    }

    #[test]
    fn field_values_are_quoted_only_when_needed() {
        assert_eq!(escape_field_value("plain"), "plain");
        assert_eq!(escape_field_value("two words"), "\"two words\"");
        assert_eq!(escape_field_value("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn sensitive_fields_are_dropped() {
        let record = LogRecord::new(LogLevel::Info, LogTopic::App, "evt")
            .field("cookie", "secret")
            .field("file", "a.flac");
        assert_eq!(record.fields.len(), 1);
        assert_eq!(record.fields[0].0, "file");
    }

    #[test]
    fn writes_and_rotates_files() {
        let (dir, outlet) = temp_outlet(120, 2);
        for index in 0..8 {
            let value = index.to_string();
            outlet.log(
                LogLevel::Info,
                LogTopic::App,
                "test_event",
                &[("index", value.as_str())],
            );
        }
        outlet.flush();
        let base = dir.path().join(LOG_FILE_NAME);
        assert!(base.is_file());
        assert!(dir.path().join(format!("{LOG_FILE_NAME}.1")).is_file());
        let content = fs::read_to_string(&base).expect("read log");
        assert!(content.contains("test_event"), "{content}");
    }

    #[test]
    fn unwritable_directory_degrades_to_memory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let blocker = dir.path().join("not-a-directory");
        fs::write(&blocker, b"x").expect("blocker");
        let outlet = LogOutlet::new(blocker);
        outlet.log(LogLevel::Info, LogTopic::App, "degraded_event", &[]);
        outlet.flush();
        assert!(outlet
            .memory
            .lock()
            .expect("memory")
            .iter()
            .any(|line| line.contains("degraded_event")));
    }

    #[test]
    fn detailed_switch_and_topic_override_only_increase_verbosity() {
        let (_dir, outlet) = temp_outlet(DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_OLD_FILES);
        assert!(!outlet.enabled(LogLevel::Debug, LogTopic::Player));
        outlet.set_detailed(true);
        assert!(outlet.enabled(LogLevel::Debug, LogTopic::Player));
        outlet.set_detailed(false);
        outlet.set_topic_level(LogTopic::Player, LogLevel::Debug);
        assert!(outlet.enabled(LogLevel::Debug, LogTopic::Player));
        assert!(!outlet.enabled(LogLevel::Debug, LogTopic::App));
    }
}
