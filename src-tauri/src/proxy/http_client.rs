//! 全局 HTTP 客户端模块
//!
//! 提供支持全局代理配置的 HTTP 客户端。
//! 所有需要发送 HTTP 请求的模块都应使用此模块提供的客户端。

use crate::provider::ProviderProxyConfig;
use once_cell::sync::OnceCell;
use reqwest::Client;
use std::net::IpAddr;
use std::sync::RwLock;
use std::time::Duration;
use url::Url;

/// 全局 HTTP 客户端实例
static GLOBAL_CLIENT: OnceCell<RwLock<Client>> = OnceCell::new();

/// 当前代理 URL（用于日志和状态查询）
static CURRENT_PROXY_URL: OnceCell<RwLock<Option<String>>> = OnceCell::new();

/// 代理策略（用于区分 Auto/Direct/Proxy）
static CURRENT_POLICY: OnceCell<RwLock<ProxyPolicy>> = OnceCell::new();

/// 直连 HTTP 客户端（永远不走任何代理，避免与系统/Clash 代理产生冲突）
static DIRECT_CLIENT: OnceCell<Client> = OnceCell::new();

/// CC Switch 代理服务器当前监听端口（用于上游兼容接口）
static CC_SWITCH_PROXY_PORT: OnceCell<RwLock<u16>> = OnceCell::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyPolicy {
    /// 未显式设置：启动时可按需继承环境变量代理
    Auto,
    /// 显式直连：忽略环境变量代理
    Direct,
    /// 显式代理：忽略环境变量代理，使用配置的代理 URL
    Proxy,
}

fn set_policy(policy: ProxyPolicy) {
    if CURRENT_POLICY.set(RwLock::new(policy)).is_err() {
        if let Some(lock) = CURRENT_POLICY.get() {
            if let Ok(mut guard) = lock.write() {
                *guard = policy;
            }
        }
    }
}

/// 获取当前代理策略
pub fn get_policy() -> ProxyPolicy {
    CURRENT_POLICY
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|p| *p)
        .unwrap_or(ProxyPolicy::Auto)
}

/// 设置 CC Switch 代理服务器监听端口
pub fn set_proxy_port(port: u16) {
    if let Some(lock) = CC_SWITCH_PROXY_PORT.get() {
        if let Ok(mut current_port) = lock.write() {
            *current_port = port;
        }
    } else {
        let _ = CC_SWITCH_PROXY_PORT.set(RwLock::new(port));
    }
}

/// 初始化全局 HTTP 客户端
///
/// 应在应用启动时调用一次。
///
/// # Arguments
/// * `proxy_url` - 代理 URL，如 `http://127.0.0.1:7890` 或 `socks5://127.0.0.1:1080`
///   传入 None 或空字符串表示直连
///
/// # 行为说明
/// - 若数据库/调用方未提供 proxy_url：
///   会尝试读取系统环境变量（HTTPS_PROXY/https_proxy/ALL_PROXY/http_proxy 等）作为“默认出站代理”。
///   这样可兼容在受限网络环境下必须经由本地代理才能访问上游的场景。
/// - 若调用方显式传入 None/空字符串：
///   视为“直连”，将忽略系统环境变量代理。
pub fn init(proxy_url: Option<&str>) -> Result<(), String> {
    // 1) 显式配置优先
    //    - Some("") / Some("   ")：显式直连（忽略环境变量代理）
    //    - Some(url)：显式代理
    //    - None：未提供（可继承环境变量代理）
    let explicit_direct = matches!(proxy_url, Some(s) if s.trim().is_empty());
    let policy = if proxy_url.is_none() {
        ProxyPolicy::Auto
    } else if explicit_direct {
        ProxyPolicy::Direct
    } else {
        ProxyPolicy::Proxy
    };
    let mut effective_url: Option<String> = proxy_url.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });

    // 2) 若未显式配置：继承系统环境变量代理（仅在 init 阶段）
    if !explicit_direct && effective_url.is_none() {
        if let Some(env_url) = detect_system_proxy_url() {
            log::info!(
                "[GlobalProxy] No saved proxy config, inheriting from environment: {}",
                mask_url(&env_url)
            );
            effective_url = Some(env_url);
        }
    }

    let client = build_client(effective_url.as_deref())?;
    set_policy(policy);

    // 尝试初始化全局客户端，如果已存在则记录警告并使用 apply_proxy 更新
    if GLOBAL_CLIENT.set(RwLock::new(client.clone())).is_err() {
        log::warn!(
            "[GlobalProxy] [GP-003] Already initialized, updating instead: {}",
            effective_url
                .as_deref()
                .map(mask_url)
                .unwrap_or_else(|| "direct connection".to_string())
        );
        // 已初始化：改用 apply_proxy 更新客户端与代理 URL，但保持 policy 语义（Auto/Direct/Proxy）
        // - Auto: 允许继承环境变量代理（effective_url 可能来自 env）
        // - Direct/Proxy: 不继承环境变量
        let result = apply_proxy(effective_url.as_deref());
        set_policy(policy);
        return result;
    }

    // 初始化代理 URL 记录
    let _ = CURRENT_PROXY_URL.set(RwLock::new(effective_url.clone()));

    log::info!(
        "[GlobalProxy] Initialized: {}",
        effective_url
            .as_deref()
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

/// 验证代理配置（不应用）
///
/// 只验证代理 URL 是否有效，不实际更新全局客户端。
/// 用于在持久化之前验证配置的有效性。
///
/// # Arguments
/// * `proxy_url` - 代理 URL，None 或空字符串表示直连
///
/// # Returns
/// 验证成功返回 Ok(())，失败返回错误信息
pub fn validate_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    // 只调用 build_client 来验证，但不应用
    build_client(effective_url)?;
    Ok(())
}

/// 构建临时 HTTP 客户端（不影响全局客户端）
///
/// 用途：
/// - 单次请求的兜底重试（例如代理冲突时切换直连/环境代理）
/// - CLI/测试场景快速构建不同出站策略的 Client
pub fn build_ephemeral_client(proxy_url: Option<&str>) -> Result<Client, String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    build_client(effective_url)
}

/// 应用代理配置（假设已验证）
///
/// 直接应用代理配置到全局客户端，不做额外验证。
/// 应在 validate_proxy 成功后调用。
///
/// # Arguments
/// * `proxy_url` - 代理 URL，None 或空字符串表示直连
pub fn apply_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    let new_client = build_client(effective_url)?;
    set_policy(if effective_url.is_some() {
        ProxyPolicy::Proxy
    } else {
        ProxyPolicy::Direct
    });

    // 更新客户端
    if let Some(lock) = GLOBAL_CLIENT.get() {
        let mut client = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-001] Failed to acquire write lock: {e}");
            "Failed to update proxy: lock poisoned".to_string()
        })?;
        *client = new_client;
    } else {
        // 如果还没初始化，则初始化
        return init(proxy_url);
    }

    // 更新代理 URL 记录
    if let Some(lock) = CURRENT_PROXY_URL.get() {
        let mut url = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-002] Failed to acquire URL write lock: {e}");
            "Failed to update proxy URL record: lock poisoned".to_string()
        })?;
        *url = effective_url.map(|s| s.to_string());
    }

    log::info!(
        "[GlobalProxy] Applied: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

/// 更新代理配置（热更新）
///
/// 可在运行时调用以更改代理设置，无需重启应用。
/// 注意：此函数同时验证和应用，如果需要先验证后持久化再应用，
/// 请使用 validate_proxy + apply_proxy 组合。
///
/// # Arguments
/// * `proxy_url` - 新的代理 URL，None 或空字符串表示直连
#[allow(dead_code)]
pub fn update_proxy(proxy_url: Option<&str>) -> Result<(), String> {
    let effective_url = proxy_url.filter(|s| !s.trim().is_empty());
    let new_client = build_client(effective_url)?;
    set_policy(if effective_url.is_some() {
        ProxyPolicy::Proxy
    } else {
        ProxyPolicy::Direct
    });

    // 更新客户端
    if let Some(lock) = GLOBAL_CLIENT.get() {
        let mut client = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-001] Failed to acquire write lock: {e}");
            "Failed to update proxy: lock poisoned".to_string()
        })?;
        *client = new_client;
    } else {
        // 如果还没初始化，则初始化
        return init(proxy_url);
    }

    // 更新代理 URL 记录
    if let Some(lock) = CURRENT_PROXY_URL.get() {
        let mut url = lock.write().map_err(|e| {
            log::error!("[GlobalProxy] [GP-002] Failed to acquire URL write lock: {e}");
            "Failed to update proxy URL record: lock poisoned".to_string()
        })?;
        *url = effective_url.map(|s| s.to_string());
    }

    log::info!(
        "[GlobalProxy] Updated: {}",
        effective_url
            .map(mask_url)
            .unwrap_or_else(|| "direct connection".to_string())
    );

    Ok(())
}

/// 获取全局 HTTP 客户端
///
/// 返回配置了代理的客户端（如果已配置代理），否则返回直连客户端。
pub fn get() -> Client {
    GLOBAL_CLIENT
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|c| c.clone())
        .unwrap_or_else(|| {
            // 如果还没初始化，创建一个默认客户端（配置与 build_client 一致）
            log::warn!("[GlobalProxy] [GP-004] Client not initialized, using fallback");
            Client::builder()
                .timeout(Duration::from_secs(600))
                .connect_timeout(Duration::from_secs(30))
                .pool_max_idle_per_host(10)
                .tcp_keepalive(Duration::from_secs(60))
                .no_proxy()
                .build()
                .unwrap_or_default()
        })
}

/// 获取直连 HTTP 客户端（强制不使用任何代理）
pub fn get_direct() -> Client {
    DIRECT_CLIENT
        .get_or_init(|| {
            Client::builder()
                .timeout(Duration::from_secs(600))
                .connect_timeout(Duration::from_secs(30))
                .pool_max_idle_per_host(10)
                .tcp_keepalive(Duration::from_secs(60))
                .no_proxy()
                .build()
                .unwrap_or_default()
        })
        .clone()
}

/// 获取当前代理 URL
///
/// 返回当前配置的代理 URL，None 表示直连。
pub fn get_current_proxy_url() -> Option<String> {
    CURRENT_PROXY_URL
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|url| url.clone())
}

/// 检查是否正在使用代理
#[allow(dead_code)]
pub fn is_proxy_enabled() -> bool {
    get_current_proxy_url().is_some()
}

/// 从系统环境变量推断出站代理 URL。
///
/// 优先级：
/// 1) HTTPS_PROXY / https_proxy
/// 2) ALL_PROXY / all_proxy
/// 3) HTTP_PROXY / http_proxy
///
/// 说明：
/// - 仅用于 `init()` 在“未显式配置代理”时兜底。
/// - 这里不会做 DB 持久化；只影响当前运行态。
fn detect_system_proxy_url() -> Option<String> {
    // 注意：环境变量常见写法包括 http(s):// 与 socks5://
    // 也可能是 socks5h://（由 build_client 允许）
    const KEYS: [&str; 6] = [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];

    for key in KEYS {
        if let Ok(value) = std::env::var(key) {
            let v = value.trim();
            if v.is_empty() {
                continue;
            }
            return Some(v.to_string());
        }
    }

    None
}

/// 暴露给调用方的环境代理探测（只读）
///
/// 用于在请求转发失败时做兜底（例如 Clash TUN 与显式代理冲突时可切换直连/环境代理重试）。
pub fn detect_env_proxy_url() -> Option<String> {
    detect_system_proxy_url()
}

/// 构建 HTTP 客户端
fn build_client(proxy_url: Option<&str>) -> Result<Client, String> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .tcp_keepalive(Duration::from_secs(60))
        // 统一关闭 reqwest 的“自动读取环境代理”，避免与 Clash/系统代理叠加导致不可预期行为。
        // 如果需要继承环境代理，会在 init() 阶段显式读取并注入。
        .no_proxy()
        // 禁用 reqwest 自动解压：防止 reqwest 覆盖客户端原始 accept-encoding header。
        // 响应解压由 response_processor 根据 content-encoding 手动处理。
        .no_gzip()
        .no_brotli()
        .no_deflate();

    // 有代理地址则使用代理，否则直连
    if let Some(url) = proxy_url {
        // 先验证 URL 格式和 scheme
        let parsed =
            Url::parse(url).map_err(|e| format!("Invalid proxy URL '{}': {}", mask_url(url), e))?;

        let scheme = parsed.scheme();
        if !["http", "https", "socks5", "socks5h"].contains(&scheme) {
            return Err(format!(
                "Invalid proxy scheme '{}' in URL '{}'. Supported: http, https, socks5, socks5h",
                scheme,
                mask_url(url)
            ));
        }

        // 兼容 NO_PROXY / no_proxy，避免把本地回环流量也错误地走出站代理（会导致自我代理/环路）
        // - 始终绕过 localhost / 127.0.0.1 / ::1
        // - 解析 NO_PROXY/no_proxy 的常见语义（host / .suffix / host:port）
        let no_proxy = std::sync::Arc::new(NoProxyMatcher::from_env());
        let proxy_url = parsed.clone();

        let proxy = reqwest::Proxy::custom(move |destination| {
            if should_bypass_proxy(destination, &no_proxy) {
                None
            } else {
                Some(proxy_url.clone())
            }
        });

        builder = builder.proxy(proxy);
        log::debug!("[GlobalProxy] Proxy configured: {}", mask_url(url));
    } else {
        log::debug!("[GlobalProxy] Direct connection (no proxy)");
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

#[derive(Debug, Clone, Default)]
struct NoProxyMatcher {
    rules: Vec<NoProxyRule>,
}

#[derive(Debug, Clone)]
enum NoProxyRule {
    Any,
    Host(String),
    Suffix(String),
    HostPort { host: String, port: u16 },
    Ip(IpAddr),
}

impl NoProxyMatcher {
    fn from_env() -> Self {
        let raw = std::env::var("NO_PROXY")
            .or_else(|_| std::env::var("no_proxy"))
            .unwrap_or_default();

        let mut rules = Vec::new();
        for part in raw.split(',') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            if p == "*" {
                rules.push(NoProxyRule::Any);
                continue;
            }

            // host:port
            if let Some((h, port)) = p.rsplit_once(':') {
                if let Ok(port) = port.parse::<u16>() {
                    let host = h.trim();
                    if !host.is_empty() {
                        rules.push(NoProxyRule::HostPort {
                            host: host.to_string(),
                            port,
                        });
                        continue;
                    }
                }
            }

            // IP
            if let Ok(ip) = p.parse::<IpAddr>() {
                rules.push(NoProxyRule::Ip(ip));
                continue;
            }

            // .suffix
            if let Some(suffix) = p.strip_prefix('.') {
                if !suffix.is_empty() {
                    rules.push(NoProxyRule::Suffix(suffix.to_ascii_lowercase()));
                }
                continue;
            }

            rules.push(NoProxyRule::Host(p.to_ascii_lowercase()));
        }

        Self { rules }
    }

    fn matches(&self, host: &str, port: Option<u16>) -> bool {
        let h = host.to_ascii_lowercase();

        for rule in &self.rules {
            match rule {
                NoProxyRule::Any => return true,
                NoProxyRule::Host(expected) => {
                    if h == *expected || h.ends_with(&format!(".{expected}")) {
                        return true;
                    }
                }
                NoProxyRule::Suffix(suffix) => {
                    if h == *suffix || h.ends_with(&format!(".{suffix}")) {
                        return true;
                    }
                }
                NoProxyRule::HostPort { host, port: p } => {
                    if port == Some(*p) {
                        let expected = host.to_ascii_lowercase();
                        if h == expected || h.ends_with(&format!(".{expected}")) {
                            return true;
                        }
                    }
                }
                NoProxyRule::Ip(ip) => {
                    if let Ok(h_ip) = h.parse::<IpAddr>() {
                        if &h_ip == ip {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }
}

fn should_bypass_proxy(destination: &Url, no_proxy: &NoProxyMatcher) -> bool {
    let Some(host) = destination.host_str() else {
        return false;
    };

    // 永远绕过本地回环，避免“软件代理 -> 再走系统代理 -> 回到软件代理”的环路
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() {
            return true;
        }
    }

    let port = destination.port_or_known_default();
    no_proxy.matches(host, port)
}

/// 隐藏 URL 中的敏感信息（用于日志）
pub fn mask_url(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        // 隐藏用户名和密码，保留 scheme、host 和端口
        let host = parsed.host_str().unwrap_or("?");
        match parsed.port() {
            Some(port) => format!("{}://{}:{}", parsed.scheme(), host, port),
            None => format!("{}://{}", parsed.scheme(), host),
        }
    } else {
        // URL 解析失败，返回部分内容
        if url.len() > 20 {
            format!("{}...", &url[..20])
        } else {
            url.to_string()
        }
    }
}

/// 根据供应商单独代理配置构建代理 URL
pub fn build_proxy_url_from_config(config: &ProviderProxyConfig) -> Option<String> {
    let proxy_type = config.proxy_type.as_deref().unwrap_or("http");
    let host = config.proxy_host.as_deref()?;
    let port = config.proxy_port?;

    if let (Some(username), Some(password)) = (&config.proxy_username, &config.proxy_password) {
        if !username.is_empty() && !password.is_empty() {
            return Some(format!(
                "{proxy_type}://{username}:{password}@{host}:{port}"
            ));
        }
    }

    Some(format!("{proxy_type}://{host}:{port}"))
}

/// 根据供应商代理配置构建 HTTP 客户端
pub fn build_client_for_provider(proxy_config: Option<&ProviderProxyConfig>) -> Option<Client> {
    let config = proxy_config.filter(|c| c.enabled)?;
    let proxy_url = build_proxy_url_from_config(config)?;

    match build_client(Some(&proxy_url)) {
        Ok(client) => Some(client),
        Err(e) => {
            log::error!(
                "[ProviderProxy] Failed to build client with proxy {}: {}",
                mask_url(&proxy_url),
                e
            );
            None
        }
    }
}

/// 获取供应商专用 HTTP 客户端（优先使用供应商代理）
pub fn get_for_provider(proxy_config: Option<&ProviderProxyConfig>) -> Client {
    if let Some(client) = build_client_for_provider(proxy_config) {
        return client;
    }
    get()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_url() {
        assert_eq!(mask_url("http://127.0.0.1:7890"), "http://127.0.0.1:7890");
        assert_eq!(
            mask_url("http://user:pass@127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            mask_url("socks5://admin:secret@proxy.example.com:1080"),
            "socks5://proxy.example.com:1080"
        );
        // 无端口的 URL 不应显示 ":?"
        assert_eq!(
            mask_url("http://proxy.example.com"),
            "http://proxy.example.com"
        );
        assert_eq!(
            mask_url("https://user:pass@proxy.example.com"),
            "https://proxy.example.com"
        );
    }

    #[test]
    fn test_build_client_direct() {
        let result = build_client(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_with_http_proxy() {
        let result = build_client(Some("http://127.0.0.1:7890"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_with_socks5_proxy() {
        let result = build_client(Some("socks5://127.0.0.1:1080"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_invalid_url() {
        // reqwest::Proxy::all 对某些无效 URL 不会立即报错
        // 使用明确无效的 scheme 来触发错误
        let result = build_client(Some("invalid-scheme://127.0.0.1:7890"));
        assert!(result.is_err(), "Should reject invalid proxy scheme");
    }
}
