//! Web 控制台 HTTP 服务器（axum + 优雅关停）。
//!
//! 路由：
//! - `GET  /api/panel/auth-state`：读取面板认证状态（无需鉴权）。
//! - `POST /api/panel/setup`：首次访问时设置访问密码（仅未设置密码时允许）。
//! - `POST /api/panel/login`：使用访问密码登录并创建会话。
//! - `POST /api/panel/logout`：注销当前会话。
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const WEB_PANEL_PASSWORD_HASH_KEY: &str = "web_panel_password_hash";
const PASSWORD_HASH_SCHEME: &str = "sha256";
const MIN_PASSWORD_CHARS: usize = 8;

/// 共享状态
#[derive(Clone)]
pub struct WebState {
    pub app: Arc<AppState>,
    /// 当前进程内有效 Web 会话令牌。密码持久化，会话随服务重启失效。
    sessions: Arc<Mutex<HashSet<String>>>,
    /// 串行化首次密码设置，避免并发首次访问覆盖。
    setup_lock: Arc<Mutex<()>>,
    /// 监听端口（用于 Origin 兜底白名单）
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
    /// - `bind_address`：Web 控制台监听地址；局域网访问通常使用 `0.0.0.0`。
    pub fn new(app: Arc<AppState>, bind_address: String, port: u16) -> Self {
        let state = WebState {
            app,
            sessions: Arc::new(Mutex::new(HashSet::new())),
            setup_lock: Arc::new(Mutex::new(())),
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

        // 安全：非回环地址必须有访问密码与会话鉴权，此处显式提示网络暴露范围。
        let is_loopback = matches!(
            self.bind_address.as_str(),
            "127.0.0.1" | "::1" | "localhost"
        );
        if !is_loopback {
            log::warn!(
                "[WebPanel] 绑定非回环地址 {}，请确保网络可信；已强制启用访问密码与会话鉴权",
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
        .route("/api/panel/auth-state", get(panel_auth_state_handler))
        .route("/api/panel/setup", post(panel_setup_handler))
        .route("/api/panel/login", post(panel_login_handler))
        .route("/api/panel/logout", post(panel_logout_handler))
        .route("/api/invoke/:command", post(invoke_handler))
        .route("/healthz", get(|| async { "ok" }))
        .fallback(get(static_handler))
        .with_state(state)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelAuthState {
    setup_required: bool,
    authenticated: bool,
}

#[derive(Debug, Deserialize)]
struct PasswordPayload {
    password: String,
}

#[derive(Debug, Serialize)]
struct TokenResponse {
    token: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// 校验 Origin 是否为当前请求 Host 的同源地址（防御跨源页面 CSRF 读取）。
fn origin_allowed(headers: &HeaderMap, origin: &str, port: u16) -> bool {
    if let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) {
        let expected_http = format!("http://{host}");
        let expected_https = format!("https://{host}");
        if origin == expected_http || origin == expected_https {
            return true;
        }
    }

    // 兜底保留本机地址，兼容部分客户端未按预期设置 Host 的情况。
    let loopback_allowed = [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ];
    loopback_allowed.iter().any(|a| a == origin)
}

fn reject_bad_origin_if_present(headers: &HeaderMap, port: u16) -> Option<Response> {
    headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|origin| !origin_allowed(headers, origin, port))
        .map(|_| (StatusCode::FORBIDDEN, "bad origin").into_response())
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.trim().is_empty())
}

fn session_valid(st: &WebState, token: &str) -> bool {
    st.sessions
        .lock()
        .map(|sessions| sessions.contains(token))
        .unwrap_or(false)
}

fn create_session(st: &WebState) -> Result<String, String> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut sessions = st
        .sessions
        .lock()
        .map_err(|e| format!("会话状态不可用: {e}"))?;
    sessions.insert(token.clone());
    Ok(token)
}

fn remove_session(st: &WebState, token: &str) {
    if let Ok(mut sessions) = st.sessions.lock() {
        sessions.remove(token);
    }
}

fn password_hash(st: &WebState) -> Result<Option<String>, String> {
    st.app
        .db
        .get_setting(WEB_PANEL_PASSWORD_HASH_KEY)
        .map_err(|e| format!("读取访问密码配置失败: {e}"))
}

fn set_password_hash(st: &WebState, value: &str) -> Result<(), String> {
    st.app
        .db
        .set_setting(WEB_PANEL_PASSWORD_HASH_KEY, value)
        .map_err(|e| format!("保存访问密码配置失败: {e}"))
}

fn validate_password(password: &str) -> Result<(), String> {
    if password.trim().is_empty() {
        return Err("访问密码不能为空".to_string());
    }
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!("访问密码至少需要 {MIN_PASSWORD_CHARS} 个字符"));
    }
    Ok(())
}

fn sha256_hex(salt: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b":");
    hasher.update(password.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hash_password(password: &str) -> String {
    let salt = uuid::Uuid::new_v4().simple().to_string();
    let hash = sha256_hex(&salt, password);
    format!("{PASSWORD_HASH_SCHEME}:{salt}:{hash}")
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn verify_password(password: &str, encoded: &str) -> bool {
    let mut parts = encoded.splitn(3, ':');
    let scheme = parts.next().unwrap_or_default();
    let salt = parts.next().unwrap_or_default();
    let expected = parts.next().unwrap_or_default();
    if scheme != PASSWORD_HASH_SCHEME || salt.is_empty() || expected.is_empty() {
        return false;
    }
    constant_time_eq(&sha256_hex(salt, password), expected)
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}

async fn panel_auth_state_handler(State(st): State<WebState>, headers: HeaderMap) -> Response {
    if let Some(resp) = reject_bad_origin_if_present(&headers, st.port) {
        return resp;
    }

    let setup_required = match password_hash(&st) {
        Ok(hash) => hash.is_none(),
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let authenticated = bearer_token(&headers)
        .map(|token| session_valid(&st, token))
        .unwrap_or(false);

    Json(PanelAuthState {
        setup_required,
        authenticated,
    })
    .into_response()
}

async fn panel_setup_handler(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(payload): Json<PasswordPayload>,
) -> Response {
    if let Some(resp) = reject_bad_origin_if_present(&headers, st.port) {
        return resp;
    }
    if let Err(e) = validate_password(&payload.password) {
        return json_error(StatusCode::BAD_REQUEST, e);
    }

    let _guard = match st.setup_lock.lock() {
        Ok(guard) => guard,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")),
    };

    match password_hash(&st) {
        Ok(Some(_)) => return json_error(StatusCode::CONFLICT, "访问密码已设置，请登录"),
        Ok(None) => {}
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }

    let encoded = hash_password(&payload.password);
    if let Err(e) = set_password_hash(&st, &encoded) {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, e);
    }

    match create_session(&st) {
        Ok(token) => Json(TokenResponse { token }).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn panel_login_handler(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(payload): Json<PasswordPayload>,
) -> Response {
    if let Some(resp) = reject_bad_origin_if_present(&headers, st.port) {
        return resp;
    }

    let Some(encoded) = (match password_hash(&st) {
        Ok(hash) => hash,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }) else {
        return json_error(StatusCode::BAD_REQUEST, "尚未设置访问密码");
    };

    if !verify_password(&payload.password, &encoded) {
        return json_error(StatusCode::UNAUTHORIZED, "访问密码不正确");
    }

    match create_session(&st) {
        Ok(token) => Json(TokenResponse { token }).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn panel_logout_handler(State(st): State<WebState>, headers: HeaderMap) -> Response {
    if let Some(resp) = reject_bad_origin_if_present(&headers, st.port) {
        return resp;
    }
    let Some(token) = bearer_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };
    remove_session(&st, token);
    Json(json!({ "ok": true })).into_response()
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

    // 2) Bearer 会话令牌
    let Some(token) = bearer_token(&headers) else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };
    if !session_valid(&st, token) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    // 3) Origin 纵深防御（存在则必须同源）
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if !origin_allowed(&headers, origin, st.port) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn password_hash_roundtrip_accepts_only_original_password() {
        let encoded = hash_password("correct-password");

        assert!(verify_password("correct-password", &encoded));
        assert!(!verify_password("wrong-password", &encoded));
    }

    #[test]
    fn origin_allowed_accepts_same_lan_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.20:18080"));

        assert!(origin_allowed(&headers, "http://192.168.1.20:18080", 18080));
    }

    #[test]
    fn origin_allowed_rejects_cross_origin_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.20:18080"));

        assert!(!origin_allowed(
            &headers,
            "http://192.168.1.21:18080",
            18080
        ));
    }
}
