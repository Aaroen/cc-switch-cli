//! 嵌入 Web 控制台前端构建产物。
//!
//! 复用与桌面一致的前端构建产物（`../dist`，由 `pnpm build:renderer` 生成）。
//! 借助运行时传输切换（transport.ts 的 isTauri 判定），同一份构建产物既可在 Tauri WebView 运行，
//! 也可在浏览器经 HTTP 网关运行，无需单独的 Web 构建。
//!
//! rust-embed：release 构建嵌入二进制；debug 构建默认从磁盘读取，便于前端迭代。
//! 编译期要求 `../dist` 存在（与 `pnpm tauri build` 一致：需先构建前端）。

#[derive(rust_embed::RustEmbed)]
#[folder = "../dist"]
pub struct WebAssets;
