//! 无头（CLI / daemon）模式的轻量日志后端。
//!
//! 桌面端由 `tauri-plugin-log` 为 `log` facade 提供后端；而 CLI `server start`
//! 路径此前从未初始化任何后端，导致 `log::warn!`（含 USG-001 用量写入失败提示）、
//! `log::info!`、`log::debug!` 等被静默丢弃，使代理/用量问题在守护进程模式下不可诊断。
//!
//! 本模块实现一个最小 `log::Log`，将记录追加到与 `file_logger` 相同的 server.log，
//! 级别由数据库 `LogConfig` 决定。`file_logger` 已直接写入 server.log 并同时 mirror
//! 到 log facade，为避免“正常/错误”摘要行重复，跳过来源于 `file_logger` 模块的记录。

use chrono::{FixedOffset, Utc};
use log::{LevelFilter, Log, Metadata, Record};

struct HeadlessLogger {
    level: LevelFilter,
}

impl Log for HeadlessLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let target = record.target();
        // file_logger 自身已直接写入 server.log，跳过其 facade mirror，避免重复摘要行。
        if target.contains("file_logger") {
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
            target,
            record.args()
        );
        crate::proxy::file_logger::get_file_logger().write(&line);
    }

    fn flush(&self) {}
}

/// 在无头模式初始化 `log` facade 后端。
///
/// 幂等：`set_boxed_logger` 仅首次成功；若已存在后端（重复调用/测试环境），仅调整级别。
pub fn init(level: LevelFilter) {
    let logger = Box::new(HeadlessLogger { level });
    if log::set_boxed_logger(logger).is_ok() {
        log::set_max_level(level);
        log::info!("[headless-log] 日志后端已初始化，级别={level}");
    } else {
        log::set_max_level(level);
    }
}
