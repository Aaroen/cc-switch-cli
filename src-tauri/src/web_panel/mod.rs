//! CLI 专属 Web 控制台
//!
//! 仅在无头 CLI 模式下提供（GUI 已内嵌面板，故不重复）。
//! 设计原则：100% 复用同一套前端构建产物（dist-web），并通过 `/api/invoke/:command`
//! 网关调用与对应 Tauri 命令**完全相同**的后端逻辑（不重写 WebDAV/导入导出等任何业务）。
//!
//! 安全：默认绑定回环；强制随机 Bearer 令牌 + 自定义头 `X-CC-Switch-Panel` + Origin 校验。
//! 约束：绝不调用 `crate::proxy::http_client::set_proxy_port`（那是代理专属全局状态）。

mod assets;
pub mod dispatch;
pub mod server;

pub use server::WebPanelServer;
