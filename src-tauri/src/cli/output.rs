//! CLI输出格式化工具
//!
//! 通过单一 anstream/anstyle 门控统一着色：
//! - 自动识别 NO_COLOR / 非 TTY（管道）/ Windows-WSL 虚拟终端（经 anstream）。
//! - 全局 ColorMode 由 `--color/--no-color` 在启动时设置一次。
//! 机器可读输出（如 `provider export -o -`）必须经 [`raw_stdout`] 或 [`json`] 输出，永不着色。

use anstyle::{AnsiColor, Color, Effects, Style};
use comfy_table::{
    presets::{ASCII_FULL, UTF8_FULL},
    *,
};
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

/// 颜色模式（由 --color/--no-color 决定）
#[derive(Clone, Copy, Debug)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

static COLOR_MODE: OnceLock<ColorMode> = OnceLock::new();

/// 在任何输出之前由 entry.rs 调用一次，设置全局颜色模式。
pub fn init_color(mode: ColorMode) {
    let _ = COLOR_MODE.set(mode);
}

fn mode() -> ColorMode {
    COLOR_MODE.get().copied().unwrap_or(ColorMode::Auto)
}

fn color_choice() -> anstream::ColorChoice {
    match mode() {
        ColorMode::Auto => anstream::ColorChoice::Auto,
        ColorMode::Always => anstream::ColorChoice::Always,
        ColorMode::Never => anstream::ColorChoice::Never,
    }
}

/// 着色后的 stdout（非 TTY/NO_COLOR/Never 时自动剥离 ANSI）
fn out() -> anstream::AutoStream<std::io::Stdout> {
    anstream::AutoStream::new(std::io::stdout(), color_choice())
}

/// 着色后的 stderr
fn err_out() -> anstream::AutoStream<std::io::Stderr> {
    anstream::AutoStream::new(std::io::stderr(), color_choice())
}

/// 当前是否应着色（用于 comfy-table 等绕过 AutoStream 的输出路径，保持与 out() 一致）
fn color_enabled() -> bool {
    match mode() {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
        }
    }
}

const S_OK: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)))
    .effects(Effects::BOLD);
const S_ERR: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)))
    .effects(Effects::BOLD);
const S_WARN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const S_INFO: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const S_DIM: Style = Style::new().effects(Effects::DIMMED);
const S_HEAD: Style = Style::new().effects(Effects::BOLD);
const S_GREEN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

/// 拆出前导换行，避免字形前缀落在前导换行之前（如 info("\n...")）。
fn split_leading_newlines(msg: &str) -> (&str, &str) {
    let body = msg.trim_start_matches('\n');
    let nl = &msg[..msg.len() - body.len()];
    (nl, body)
}

/// 格式化成功消息
pub fn success(msg: &str) {
    let (nl, body) = split_leading_newlines(msg);
    let mut o = out();
    let _ = write!(o, "{nl}");
    let _ = writeln!(o, "{S_OK}✓{S_OK:#} {body}");
}

/// 格式化错误消息
pub fn error(msg: &str) {
    let (nl, body) = split_leading_newlines(msg);
    let mut e = err_out();
    let _ = write!(e, "{nl}");
    let _ = writeln!(e, "{S_ERR}✗{S_ERR:#} {body}");
}

/// 格式化警告消息
pub fn warning(msg: &str) {
    let (nl, body) = split_leading_newlines(msg);
    let mut o = out();
    let _ = write!(o, "{nl}");
    let _ = writeln!(o, "{S_WARN}⚠{S_WARN:#} {body}");
}

/// 警告到 stderr（用于机器可读 stdout 路径前的提示，保持 stdout 纯净便于管道消费）
pub fn warning_stderr(msg: &str) {
    let (nl, body) = split_leading_newlines(msg);
    let mut e = err_out();
    let _ = write!(e, "{nl}");
    let _ = writeln!(e, "{S_WARN}⚠{S_WARN:#} {body}");
}

/// 格式化信息消息
pub fn info(msg: &str) {
    let (nl, body) = split_leading_newlines(msg);
    let mut o = out();
    let _ = write!(o, "{nl}");
    let _ = writeln!(o, "{S_INFO}ℹ{S_INFO:#} {body}");
}

/// 输出表格（非 TTY/NO_COLOR 时降级为纯 ASCII，便于管道处理）
pub fn table(headers: Vec<&str>, rows: Vec<Vec<String>>) {
    let mut table = Table::new();
    let preset = if color_enabled() {
        UTF8_FULL
    } else {
        ASCII_FULL
    };
    table
        .load_preset(preset)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(headers);
    if !color_enabled() {
        table.force_no_tty();
    }

    for row in rows {
        table.add_row(row);
    }

    println!("{table}");
}

/// 输出键值对（按显示宽度对齐，正确处理中日韩全角字符）
pub fn key_value(pairs: Vec<(&str, String)>) {
    let max_w = pairs
        .iter()
        .map(|(k, _)| display_width(k))
        .max()
        .unwrap_or(0);

    let mut o = out();
    for (key, value) in pairs {
        let pad = max_w.saturating_sub(display_width(key));
        let _ = writeln!(o, "{S_DIM}{key}{S_DIM:#}{empty:pad$}: {value}", empty = "");
    }
}

/// 输出JSON（美化、纯文本、永不着色，保持机器可读字节稳定）
pub fn json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// 原样输出到 stdout（绕过 AutoStream，永不着色），用于 `provider export -o -` 等可被脚本管道消费的路径。
pub fn raw_stdout(s: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(s.as_bytes());
    let _ = o.write_all(b"\n");
}

/// 输出状态指示器（纯字形，不含 ANSI）
pub fn status_indicator(status: bool) -> &'static str {
    if status {
        "●"
    } else {
        "○"
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
    println!("{}", "─".repeat(60));
}

/// 输出章节标题（加粗，门控着色）
pub fn section(title: &str) {
    let mut o = out();
    let _ = writeln!(o, "\n{S_HEAD}{title}{S_HEAD:#}");
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
    info("权重说明（默认频率控制策略）:");
    let mut o = out();
    let _ = writeln!(o, "  0 = 禁用供应商");
    let _ = writeln!(o, "  1 = 参与频率 1/1（默认）");
    let _ = writeln!(o, "  2 = 参与频率 1/2");
    let _ = writeln!(o, "  N = 按 1/N 参与轮询槽位");
    let _ = writeln!(
        o,
        "  其它策略：weighted_random 按权重占比随机分配；hard_round_robin 等概率轮转"
    );
}

/// 输出配置差异
pub fn diff(old: &str, new: &str) {
    let mut o = out();
    let _ = writeln!(o, "{S_ERR}- {old}{S_ERR:#}");
    let _ = writeln!(o, "{S_GREEN}+ {new}{S_GREEN:#}");
}

/// 输出服务状态（运行中绿色实心，已停止灰色空心）
pub fn service_status(name: &str, running: bool, pid: Option<u32>) {
    let pid_text = pid.map(|p| format!(" (PID: {})", p)).unwrap_or_default();
    let mut o = out();
    if running {
        let _ = writeln!(
            o,
            "{S_GREEN}●{S_GREEN:#} {name} {S_OK}运行中{S_OK:#}{pid_text}"
        );
    } else {
        let _ = writeln!(o, "{S_DIM}○ {name} 已停止{pid_text}{S_DIM:#}");
    }
}

/// 输出提示文本（灰色/次要；经门控，管道时不泄漏 ANSI）
pub fn hint(msg: &str) {
    let mut o = out();
    let _ = writeln!(o, "{S_DIM}{msg}{S_DIM:#}");
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

/// 估算字符串的终端显示宽度（中日韩全角字符计 2，其余计 1）。
/// 自包含实现，避免引入 unicode-width 依赖（仓库内存在 0.2/1.x 双版本）。
fn display_width(s: &str) -> usize {
    s.chars().map(|c| if is_wide_char(c) { 2 } else { 1 }).sum()
}

fn is_wide_char(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x115F      // Hangul Jamo
            | 0x2E80..=0x303E // CJK 部首 / 假名标点
            | 0x3041..=0x33FF // 平假名/片假名/CJK 符号
            | 0x3400..=0x4DBF // CJK 扩展 A
            | 0x4E00..=0x9FFF // CJK 统一表意
            | 0xA000..=0xA4CF // 彝文
            | 0xAC00..=0xD7A3 // 谚文音节
            | 0xF900..=0xFAFF // CJK 兼容表意
            | 0xFE30..=0xFE4F // CJK 兼容形式
            | 0xFF00..=0xFF60 // 全角 ASCII
            | 0xFFE0..=0xFFE6 // 全角符号
            | 0x20000..=0x3FFFD // CJK 扩展 B+
    )
}
