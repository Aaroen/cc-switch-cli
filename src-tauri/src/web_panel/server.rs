//! Web 控制台 HTTP 服务器（axum + 优雅关停）。
//!
//! 路由：
//! - `POST /api/invoke/:command`：命令网关（强制鉴权），转发到 [`super::dispatch`]。
//! - `GET  /healthz`：健康检查（无需鉴权）。
//! - 其余：复用前端 SPA 静态资源（SPA 回退到 index.html）。

use crate::store::AppState;
use crate::web_panel::assets::WebAssets;
use crate::web_panel::dispatch;
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// 共享状态
#[derive(Clone)]
pub struct WebState {
    pub app: Arc<AppState>,
    /// 强制 Bearer 令牌
    pub token: Arc<str>,
    /// 监听端口（用于 Origin 白名单）
    pub port: u16,
}

/// Web 控制台服务器（独立于代理服务器的生命周期）
pub struct WebPanelServer {
    bind_address: String,
    port: u16,
    state: WebState,
    shutdown_tx: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl WebPanelServer {
    /// 创建服务器实例。
    ///
    /// - `app`：与代理共享的 AppState（db + proxy_service）。
    /// - `bind_address`：默认应为 `127.0.0.1`。
    /// - `token`：强制 Bearer 令牌（调用方生成，并通过 URL/CLI 告知用户）。
    pub fn new(app: Arc<AppState>, bind_address: String, port: u16, token: String) -> Self {
        let state = WebState {
            app,
            token: Arc::from(token.as_str()),
            port,
        };
        Self {
            bind_address,
            port,
            state,
            shutdown_tx: None,
            handle: None,
        }
    }

    /// 启动服务器。失败应由调用方按“非致命”处理（记录日志后继续运行代理）。
    pub async fn start(&mut self) -> Result<(), String> {
        let addr: SocketAddr = format!("{}:{}", self.bind_address, self.port)
            .parse()
            .map_err(|e| format!("无效的 Web 控制台地址: {e}"))?;

        // 安全：非回环地址必须有令牌（令牌恒有，此处仅显式校验并提示风险）
        let is_loopback = matches!(self.bind_address.as_str(), "127.0.0.1" | "::1" | "localhost");
        if !is_loopback {
            log::warn!(
                "[WebPanel] 绑定非回环地址 {}，请确保网络可信；已强制启用 Bearer 令牌鉴权",
                self.bind_address
            );
        }

        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Web 控制台端口绑定失败 ({addr}): {e}"))?;

        let app = build_router(self.state.clone());
        let (tx, rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = rx.await;
            });
            if let Err(e) = server.await {
                log::error!("[WebPanel] 服务器异常退出: {e}");
            }
        });

        self.shutdown_tx = Some(tx);
        self.handle = Some(handle);
        log::info!("[WebPanel] Web 控制台已启动于 http://{addr}");
        Ok(())
    }

    /// 关停服务器（优雅）。
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

fn build_router(state: WebState) -> Router {
    Router::new()
        .route("/api/invoke/:command", post(invoke_handler))
        .route("/healthz", get(|| async { "ok" }))
        .fallback(get(static_handler))
        .with_state(state)
}

/// 校验 Origin 是否为本机面板同源（防御本地跨源页面 CSRF 读取）。
fn origin_allowed(origin: &str, port: u16) -> bool {
    let allowed = [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ];
    allowed.iter().any(|a| a == origin)
}

async fn invoke_handler(
    State(st): State<WebState>,
    Path(command): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1) 自定义头：强制 CORS 预检，阻断表单型 CSRF
    let panel_hdr = headers
        .get("x-cc-switch-panel")
        .and_then(|v| v.to_str().ok());
    if panel_hdr != Some("1") {
        return (StatusCode::FORBIDDEN, "missing panel header").into_response();
    }

    // 2) Bearer 令牌
    let expected = format!("Bearer {}", st.token);
    let auth_ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v == expected)
        .unwrap_or(false);
    if !auth_ok {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    // 3) Origin 纵深防御（存在则必须同源）
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if !origin_allowed(origin, st.port) {
            return (StatusCode::FORBIDDEN, "bad origin").into_response();
        }
    }

    // 与内容类型无关地解析 JSON body（前端恒发送 application/json；空 body 视为无参数）
    let args: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    match dispatch::dispatch(&st.app, &command, args).await {
        // 与 Tauri invoke 语义对齐：用统一信封，错误也返回 200（前端 transport 据 ok 字段 reject）
        Ok(data) => Json(json!({ "ok": true, "data": data })).into_response(),
        Err(error) => Json(json!({ "ok": false, "error": error })).into_response(),
    }
}

/// 提供前端 SPA 静态资源（未命中回退到 index.html，支持前端路由）。
async fn static_handler(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    if let Some(content) = WebAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .header(header::CONTENT_TYPE, mime.to_string())
            .body(Body::from(content.data.into_owned()))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    // SPA 回退
    match WebAssets::get("index.html") {
        Some(content) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(content.data.into_owned()))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        None => (
            StatusCode::NOT_FOUND,
            "Web 控制台前端尚未构建，请运行 `pnpm build:web`",
        )
            .into_response(),
    }
}
