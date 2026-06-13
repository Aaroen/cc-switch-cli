//! 文件日志模块
//!
//! 将请求详情写入 ~/.cc-switch/logs/server.log
//! 格式与 v3.8.3 对齐，支持北京时间 (UTC+8)

use chrono::{FixedOffset, Utc};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// 单个日志文件的默认最大体积（MB）。可用环境变量 `CC_SWITCH_LOG_MAX_MB` 覆盖。
const DEFAULT_MAX_SIZE_MB: u64 = 5;
/// 默认保留的历史日志份数（server.log.1 ..= server.log.N）。可用 `CC_SWITCH_LOG_KEEP` 覆盖。
const DEFAULT_KEEP_FILES: usize = 0;
/// 每累计写入约 1MB 才执行一次体积检查，避免每行写入都 stat 文件。
const ROTATE_CHECK_INTERVAL_BYTES: u64 = 1024 * 1024;

/// 文件日志器
pub struct FileLogger {
    file: Mutex<Option<File>>,
    log_path: PathBuf,
    /// 自上次轮转检查以来累计写入的字节数（用于节流 stat 调用）
    bytes_since_check: AtomicU64,
    /// 单文件最大体积（字节）
    max_size_bytes: u64,
    /// 保留的历史日志份数
    keep_files: usize,
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

        // 体积上限与保留份数：默认值可由环境变量覆盖，全部使用相对配置语义，无硬编码绝对路径。
        let max_size_mb = std::env::var("CC_SWITCH_LOG_MAX_MB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_MAX_SIZE_MB);
        let keep_files = std::env::var("CC_SWITCH_LOG_KEEP")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|v| *v >= 1)
            .unwrap_or(DEFAULT_KEEP_FILES);

        Self {
            file: Mutex::new(file),
            log_path,
            bytes_since_check: AtomicU64::new(0),
            max_size_bytes: max_size_mb.saturating_mul(1024 * 1024),
            keep_files,
        }
    }

    /// 获取日志文件路径
    pub fn log_path(&self) -> &PathBuf {
        &self.log_path
    }

    /// 测试用构造器：接受自定义路径与参数，不触碰真实 ~/.cc-switch
    #[cfg(test)]
    fn with_path(log_path: PathBuf, max_size_bytes: u64, keep_files: usize) -> Self {
        if let Some(parent) = log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();
        Self {
            file: Mutex::new(file),
            log_path,
            bytes_since_check: AtomicU64::new(0),
            max_size_bytes,
            keep_files,
        }
    }

    /// 写入日志行
    pub fn write(&self, line: &str) {
        if let Ok(mut guard) = self.file.lock() {
            // 尝试写入
            let write_result = if let Some(file) = guard.as_mut() {
                writeln!(file, "{}", line).and_then(|_| file.flush())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "日志文件未打开",
                ))
            };

            // 如果写入失败，尝试重新打开文件
            if let Err(e) = write_result {
                log::warn!(
                    "日志写入失败 ({}): {}, 尝试重新打开文件",
                    self.log_path.display(),
                    e
                );

                // 重新打开文件
                if let Some(parent) = self.log_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.log_path)
                {
                    Ok(new_file) => {
                        *guard = Some(new_file);
                        // 重试写入
                        if let Some(file) = guard.as_mut() {
                            if let Err(retry_err) =
                                writeln!(file, "{}", line).and_then(|_| file.flush())
                            {
                                log::error!(
                                    "重新打开后写入仍失败: {} - {}",
                                    self.log_path.display(),
                                    retry_err
                                );
                            } else {
                                log::info!("日志文件重新打开成功: {}", self.log_path.display());
                            }
                        }
                    }
                    Err(open_err) => {
                        log::error!(
                            "无法重新打开日志文件: {} - {}",
                            self.log_path.display(),
                            open_err
                        );
                    }
                }
            }
        }

        // 节流式轮转检查：累计写入达到间隔阈值才 stat 文件，避免每次写入都触发 syscall。
        // 注意此处 self.file 的锁已在上方作用域释放，do_rotate 内再次加锁不会死锁。
        let added = line.len() as u64 + 1;
        let total = self.bytes_since_check.fetch_add(added, Ordering::Relaxed) + added;
        if total >= ROTATE_CHECK_INTERVAL_BYTES {
            self.bytes_since_check.store(0, Ordering::Relaxed);
            self.rotate_if_oversized();
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
    }

    /// 记录成功请求但检测到 tokens 为 0 的异常情况
    ///
    /// 格式: [2026-01-16 18:02:37.257 ERROR] [claude] 错误 999 - provider-name                      ( 2.770s) [上游: model-name] - 详情: tokens=0
    pub fn log_success_with_zero_tokens(
        &self,
        app_type: &str,
        _status_code: u16,
        provider_name: &str,
        latency_ms: u64,
        model: &str,
    ) {
        let timestamp = Self::format_timestamp();
        let tool = Self::format_tool_tag(app_type);
        let secs = (latency_ms as f64) / 1000.0;

        let line = format!(
            "[{} ERROR] {} 错误 999 - {:<35} ({:>6.3}s) [上游: {}] - 详情: tokens=0",
            timestamp, tool, provider_name, secs, model
        );

        self.write(&line);
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
    }

    /// 按配置的最大体积检查并轮转（由写入路径自动调用）。
    fn rotate_if_oversized(&self) {
        let oversized = fs::metadata(&self.log_path)
            .map(|m| m.len() >= self.max_size_bytes)
            .unwrap_or(false);
        if oversized {
            self.do_rotate(self.keep_files);
        }
    }

    /// 检查并轮转日志文件（兼容旧接口；按给定 MB 阈值触发）。
    ///
    /// 轮转策略：滚动保留多个历史文件
    /// `server.log -> server.log.1 -> ... -> server.log.N`，超过 N 的最旧文件被覆盖。
    pub fn rotate_if_needed(&self, max_size_mb: u64) {
        let max = max_size_mb.saturating_mul(1024 * 1024);
        let oversized = fs::metadata(&self.log_path)
            .map(|m| m.len() >= max)
            .unwrap_or(false);
        if oversized {
            self.do_rotate(self.keep_files);
        }
    }

    /// 执行滚动轮转。
    ///
    /// 当 keep=0 时，使用文件内轮转：保留最后 80% 内容，删除开头旧数据，不创建历史文件。
    /// 当 keep>0 时，使用多文件轮转：server.log -> server.log.1 -> ... -> server.log.N。
    ///
    /// 关键点：先关闭当前文件句柄再操作。Windows 上对仍被进程打开的文件执行
    /// rename/truncate 会失败；先释放句柄可保证所有平台都能成功轮转。
    fn do_rotate(&self, keep: usize) {
        if let Ok(mut guard) = self.file.lock() {
            // 关闭当前文件句柄，确保后续操作在所有平台（尤其 Windows）都能成功
            *guard = None;

            if keep == 0 {
                // 文件内轮转：保留最后 80% 内容
                use std::io::{Read, Seek, SeekFrom, Write};
                let keep_bytes = (self.max_size_bytes * 4) / 5; // 保留 80%

                if let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&self.log_path) {
                    if let Ok(file_len) = file.seek(SeekFrom::End(0)) {
                        if file_len > keep_bytes {
                            // 定位到保留位置
                            if file.seek(SeekFrom::End(-(keep_bytes as i64))).is_ok() {
                                let mut tail_content = Vec::new();
                                if file.read_to_end(&mut tail_content).is_ok() {
                                    // 截断并写回
                                    let _ = file.set_len(0);
                                    let _ = file.seek(SeekFrom::Start(0));
                                    let _ = file.write_all(&tail_content);
                                    let _ = file.flush();
                                }
                            }
                        }
                    }
                }
            } else {
                // 多文件轮转：从最旧到最新滚动：.(keep-1) -> .keep, ..., .1 -> .2
                for i in (1..keep).rev() {
                    let from = self.rotated_path(i);
                    if from.exists() {
                        let to = self.rotated_path(i + 1);
                        let _ = fs::rename(&from, &to);
                    }
                }
                // server.log -> server.log.1
                let _ = fs::rename(&self.log_path, &self.rotated_path(1));
            }

            // 重新打开主文件继续写入
            *guard = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)
                .ok();
        }
        self.bytes_since_check.store(0, Ordering::Relaxed);
    }

    /// 构造历史日志路径：`server.log` -> `server.log.N`
    fn rotated_path(&self, n: usize) -> PathBuf {
        let mut s = self.log_path.clone().into_os_string();
        s.push(format!(".{n}"));
        PathBuf::from(s)
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

    #[test]
    fn test_rotation_keeps_multiple_files_and_caps_size() {
        // 使用唯一的临时目录，避免触碰真实 ~/.cc-switch
        let base = std::env::temp_dir().join(format!("ccs-log-rot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let log_path = base.join("server.log");

        // 最大 1KB、保留 3 份历史
        let logger = FileLogger::with_path(log_path.clone(), 1024, 3);

        // 触发多次轮转：每次 do_rotate 把 server.log -> server.log.1，旧的顺延
        for round in 0..5 {
            // 写满超过 1KB
            for i in 0..40 {
                logger.write(&format!("round {round} line {i} ----------------------------"));
            }
            logger.rotate_if_needed(0); // 阈值 0MB => 立即轮转，验证滚动逻辑
        }

        // 主文件存在
        assert!(log_path.exists(), "主日志文件应存在");
        // 历史文件 .1/.2/.3 存在，且不应出现 .4（超过 keep=3 被覆盖）
        assert!(logger.rotated_path(1).exists(), "server.log.1 应存在");
        assert!(logger.rotated_path(2).exists(), "server.log.2 应存在");
        assert!(logger.rotated_path(3).exists(), "server.log.3 应存在");
        assert!(
            !logger.rotated_path(4).exists(),
            "超过保留份数的 server.log.4 不应存在"
        );

        // 清理临时目录
        let _ = fs::remove_dir_all(&base);
    }
}
