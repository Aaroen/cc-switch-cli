//! 文件日志模块
//!
//! 将请求详情写入 ~/.cc-switch/logs/server.log
//! 格式与 v3.8.3 对齐，支持北京时间 (UTC+8)

use chrono::{FixedOffset, Utc};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 文件日志器
pub struct FileLogger {
    file: Mutex<Option<File>>,
    log_path: PathBuf,
}

impl FileLogger {
    /// 创建文件日志器
    pub fn new() -> Self {
        let log_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".cc-switch")
            .join("logs");

        // 确保日志目录存在
        let _ = fs::create_dir_all(&log_dir);

        let log_path = log_dir.join("server.log");

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();

        Self {
            file: Mutex::new(file),
            log_path,
        }
    }

    /// 获取日志文件路径
    pub fn log_path(&self) -> &PathBuf {
        &self.log_path
    }

    /// 写入日志行
    pub fn write(&self, line: &str) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "{}", line);
                let _ = file.flush();
            }
        }
    }

    /// 格式化时间戳（北京时间 UTC+8）
    fn format_timestamp() -> String {
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let now = Utc::now().with_timezone(&tz);
        now.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
    }

    /// 格式化工具标签
    fn format_tool_tag(app_type: &str) -> &'static str {
        match app_type {
            "claude" => "[claude]",
            "codex" => "[codex ]",
            "gemini" => "[gemini]",
            _ => "[other ]",
        }
    }

    /// 记录成功请求
    ///
    /// 格式: [2026-01-16 18:02:37.257 INFO] [claude] 正常 200 - provider-name                      ( 2.770s) [上游: model-name]
    pub fn log_success(
        &self,
        app_type: &str,
        status_code: u16,
        provider_name: &str,
        latency_ms: u64,
        model: &str,
    ) {
        let timestamp = Self::format_timestamp();
        let tool = Self::format_tool_tag(app_type);
        let secs = (latency_ms as f64) / 1000.0;

        let line = format!(
            "[{} INFO] {} 正常 {} - {:<35} ({:>6.3}s) [上游: {}]",
            timestamp, tool, status_code, provider_name, secs, model
        );

        self.write(&line);
        log::info!("{}", line);
    }

    /// 记录失败请求
    ///
    /// 格式: [2026-01-16 18:02:39.456 ERROR] [gemini] 错误 429 - provider-name                      ( 0.567s) [上游: model-name] - 详情: error message
    pub fn log_error(
        &self,
        app_type: &str,
        status_code: u16,
        provider_name: &str,
        latency_ms: u64,
        model: &str,
        error: &str,
    ) {
        let timestamp = Self::format_timestamp();
        let tool = Self::format_tool_tag(app_type);
        let secs = (latency_ms as f64) / 1000.0;

        let line = format!(
            "[{} ERROR] {} 错误 {} - {:<35} ({:>6.3}s) [上游: {}] - 详情: {}",
            timestamp, tool, status_code, provider_name, secs, model, error
        );

        self.write(&line);
        log::error!("{}", line);
    }

    /// 检查并轮转日志文件
    ///
    /// 当日志文件超过指定大小时，重命名为 server.log.1 并创建新文件
    pub fn rotate_if_needed(&self, max_size_mb: u64) {
        if let Ok(metadata) = fs::metadata(&self.log_path) {
            let size_mb = metadata.len() / (1024 * 1024);
            if size_mb >= max_size_mb {
                // 重命名为 server.log.1
                let backup_path = self.log_path.with_extension("log.1");
                let _ = fs::rename(&self.log_path, &backup_path);

                // 重新打开文件
                if let Ok(mut guard) = self.file.lock() {
                    *guard = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.log_path)
                        .ok();
                }
            }
        }
    }
}

impl Default for FileLogger {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局文件日志器实例
static FILE_LOGGER: std::sync::OnceLock<FileLogger> = std::sync::OnceLock::new();

/// 获取全局文件日志器
pub fn get_file_logger() -> &'static FileLogger {
    FILE_LOGGER.get_or_init(FileLogger::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tool_tag() {
        assert_eq!(FileLogger::format_tool_tag("claude"), "[claude]");
        assert_eq!(FileLogger::format_tool_tag("codex"), "[codex ]");
        assert_eq!(FileLogger::format_tool_tag("gemini"), "[gemini]");
        assert_eq!(FileLogger::format_tool_tag("unknown"), "[other ]");
    }

    #[test]
    fn test_format_timestamp() {
        let timestamp = FileLogger::format_timestamp();
        // 验证格式: YYYY-MM-DD HH:MM:SS.mmm
        assert!(timestamp.len() >= 23);
        assert!(timestamp.contains('-'));
        assert!(timestamp.contains(':'));
        assert!(timestamp.contains('.'));
    }
}
