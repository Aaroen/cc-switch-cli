// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 在 Linux 上设置 WebKit 环境变量以解决 DMA-BUF 渲染问题
    // 某些 Linux 系统（如 Debian 13.2、Nvidia GPU）上 WebKitGTK 的 DMA-BUF 渲染器可能导致白屏/黑屏
    // 参考: https://github.com/tauri-apps/tauri/issues/9394
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        // 禁用 WebKitGTK 合成模式，规避部分 Wayland 环境下的窗口交互异常。
        if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
    }

    // 兼容既有 `cc-switch ...` CLI 调用，同时允许后续逐步迁移到独立 `cc-switch-cli` 二进制。
    if cc_switch_lib::cli::has_cli_args() {
        cc_switch_lib::cli::run_from_env();
    }

    cc_switch_lib::run();
}
