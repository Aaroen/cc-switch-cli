//! CLI输出格式化工具

use comfy_table::{presets::UTF8_FULL, *};
use serde_json::Value;

/// 格式化成功消息
pub fn success(msg: &str) {
    println!("✓ {}", msg);
}

/// 格式化错误消息
pub fn error(msg: &str) {
    eprintln!("✗ {}", msg);
}

/// 格式化警告消息
pub fn warning(msg: &str) {
    println!("⚠ {}", msg);
}

/// 格式化信息消息
pub fn info(msg: &str) {
    println!("ℹ {}", msg);
}

/// 输出表格
pub fn table(headers: Vec<&str>, rows: Vec<Vec<String>>) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(headers);

    for row in rows {
        table.add_row(row);
    }

    println!("{}", table);
}

/// 输出键值对
pub fn key_value(pairs: Vec<(&str, String)>) {
    let max_key_len = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);

    for (key, value) in pairs {
        println!("{:width$}: {}", key, value, width = max_key_len);
    }
}

/// 输出JSON（美化）
pub fn json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

/// 输出状态指示器
pub fn status_indicator(status: bool) -> &'static str {
    if status {
        "●" // 绿色圆点
    } else {
        "○" // 白色圆点
    }
}

/// 输出进度条
pub fn progress_bar(current: usize, total: usize, msg: &str) {
    let percent = (current as f32 / total as f32 * 100.0) as usize;
    let filled = current * 40 / total;
    let empty = 40 - filled;

    print!(
        "\r[{}{}] {:3}% {}",
        "=".repeat(filled),
        "-".repeat(empty),
        percent,
        msg
    );

    if current == total {
        println!();
    }
}

/// 输出分隔线
pub fn separator() {
    println!("{}", "─".repeat(80));
}

/// 输出章节标题
pub fn section(title: &str) {
    println!("\n{}", title);
    separator();
}

/// 格式化字节大小
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_idx])
}

/// 格式化持续时间
pub fn format_duration(millis: u64) -> String {
    if millis < 1000 {
        format!("{}ms", millis)
    } else if millis < 60000 {
        format!("{:.2}s", millis as f64 / 1000.0)
    } else {
        format!("{:.2}m", millis as f64 / 60000.0)
    }
}

/// 格式化百分比
pub fn format_percent(value: f32) -> String {
    format!("{:.1}%", value)
}

/// 输出供应商权重说明
pub fn weight_help() {
    info("权重说明:");
    println!("  0 = 禁用供应商");
    println!("  1 = 每轮都使用（默认）");
    println!("  2 = 每2轮使用一次");
    println!("  N = 每N轮使用一次（频率=1/N）");
}

/// 输出配置差异
pub fn diff(old: &str, new: &str) {
    println!("- {}", old);
    println!("+ {}", new);
}

/// 输出服务状态
pub fn service_status(name: &str, running: bool, pid: Option<u32>) {
    let status_text = if running { "运行中" } else { "已停止" };
    let pid_text = pid
        .map(|p| format!(" (PID: {})", p))
        .unwrap_or_default();

    println!(
        "{} {} {}{}",
        status_indicator(running),
        name,
        status_text,
        pid_text
    );
}

/// 输出提示文本（灰色/次要）
pub fn hint(msg: &str) {
    println!("\x1b[90m{}\x1b[0m", msg); // ANSI灰色
}

/// 确认提示
pub fn confirm(prompt: &str) -> bool {
    print!("{} [y/N]: ", prompt);
    use std::io::{self, Write};
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();

    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}
