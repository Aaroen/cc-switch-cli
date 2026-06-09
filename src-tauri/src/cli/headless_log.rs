//! 无头（CLI / daemon）模式的轻量日志后端。
//!
//! 桌面端由 `tauri-plugin-log` 为 `log` facade 提供后端；而 CLI `server start`
//! 路径此前从未初始化任何后端，导致 `log::warn!`（含 USG-001 用量写入失败提示）、
//! `log::info!`、`log::debug!` 等被静默丢弃，使代理/用量问题在守护进程模式下不可诊断。
//!
//! 本模块实现一个最小 `log::Log`，将详细日志写入 summary.log，
//! 而 `file_logger` 将简洁的供应商调用摘要写入 server.log。
//! 两个日志文件各自独立，互不干扰。

use chrono::{FixedOffset, Utc};
use log::{LevelFilter, Log, Metadata, Record};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

struct HeadlessLogger {
    level: LevelFilter,
    summary_file: Mutex<Option<std::fs::File>>,
}

impl HeadlessLogger {
    fn new(level: LevelFilter) -> Self {
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cc-switch")
            .join("logs");

        let _ = fs::create_dir_all(&log_dir);

        let summary_path = log_dir.join("summary.log");
        let summary_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&summary_path)
            .ok();

        Self {
            level,
            summary_file: Mutex::new(summary_file),
        }
    }
}

impl Log for HeadlessLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let tz = match FixedOffset::east_opt(8 * 3600) {
            Some(tz) => tz,
            None => return,
        };
        let ts = Utc::now().with_timezone(&tz).format("%Y-%m-%d %H:%M:%S%.3f");
        let line = format!(
            "[{ts} {}] [{}] {}",
            record.level(),
            record.target(),
            record.args()
        );

        if let Ok(mut guard) = self.summary_file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "{}", line);
                let _ = file.flush();
            }
        }
    }

    fn flush(&self) {}
}

/// 在无头模式初始化 `log` facade 后端。
///
/// 幂等：`set_boxed_logger` 仅首次成功；若已存在后端（重复调用/测试环境），仅调整级别。
pub fn init(level: LevelFilter) {
    let logger = Box::new(HeadlessLogger::new(level));
    if log::set_boxed_logger(logger).is_ok() {
        log::set_max_level(level);
        log::info!("[headless-log] 日志后端已初始化，级别={level}");
    } else {
        log::set_max_level(level);
    }
}
