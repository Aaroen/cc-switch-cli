//! 错误类型到 HTTP 状态码的映射
//!
//! 将 ProxyError 映射到合适的 HTTP 状态码，用于日志记录和手动构建错误响应

use super::ProxyError;
use serde_json::Value;

const MAX_ERROR_DETAIL_LEN: usize = 240;
const TRUNCATE_TAIL_LEN: usize = 48;

/// 将 ProxyError 映射到 HTTP 状态码
///
/// 映射规则：
/// - 上游错误：直接使用上游返回的状态码
/// - 超时：504 Gateway Timeout
/// - 连接失败：502 Bad Gateway
/// - 无可用 Provider：503 Service Unavailable
/// - 重试耗尽：503 Service Unavailable
/// - 认证错误：401 Unauthorized
/// - 配置/请求错误：400 Bad Request
/// - 转换错误：422 Unprocessable Entity
/// - 其他错误：500 Internal Server Error
pub fn map_proxy_error_to_status(error: &ProxyError) -> u16 {
    match error {
        // 服务状态错误：与 IntoResponse 保持一致
        ProxyError::AlreadyRunning => 409,
        ProxyError::NotRunning => 503,

        // 上游错误：使用实际状态码
        ProxyError::UpstreamError { status, .. } => *status,

        // 超时错误：504 Gateway Timeout
        ProxyError::Timeout(_) | ProxyError::StreamIdleTimeout(_) => 504,

        // 转发失败/连接失败：502 Bad Gateway
        ProxyError::ForwardFailed(_) => 502,

        // 无可用 Provider：503 Service Unavailable
        ProxyError::NoAvailableProvider => 503,

        // 所有供应商已熔断：503 Service Unavailable
        ProxyError::AllProvidersCircuitOpen => 503,

        // 未配置供应商：503 Service Unavailable
        ProxyError::NoProvidersConfigured => 503,

        // 重试耗尽：503 Service Unavailable
        ProxyError::MaxRetriesExceeded => 503,

        // Provider 不健康：503 Service Unavailable
        ProxyError::ProviderUnhealthy(_) => 503,

        // 配置错误/无效请求：400 Bad Request
        ProxyError::ConfigError(_) | ProxyError::InvalidRequest(_) => 400,

        // 认证错误：401 Unauthorized
        ProxyError::AuthError(_) => 401,

        // 数据库错误：500 Internal Server Error
        ProxyError::DatabaseError(_) => 500,

        // 转换错误：422 Unprocessable Entity
        ProxyError::TransformError(_) => 422,

        // 其他未知错误：500 Internal Server Error
        _ => 500,
    }
}

fn normalize_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(MAX_ERROR_DETAIL_LEN + 32));
    let mut last_space = false;
    for ch in input.chars() {
        let is_ws = ch.is_whitespace();
        if is_ws {
            if !last_space {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
        last_space = is_ws;
    }
    out.trim().to_string()
}

fn truncate_with_tail(input: &str, max_len: usize, tail_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input.to_string();
    }
    let max_len = max_len.max(8);
    let tail_len = tail_len.min(max_len.saturating_sub(4)).max(0);
    let head_len = max_len.saturating_sub(tail_len).saturating_sub(1); // 1 for ellipsis

    let head: String = input.chars().take(head_len).collect();
    let tail: String = if tail_len == 0 {
        String::new()
    } else {
        input
            .chars()
            .rev()
            .take(tail_len)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    };

    if tail.is_empty() {
        format!("{head}…")
    } else {
        format!("{head}…{tail}")
    }
}

fn unescape_if_quoted(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        if let Ok(s) = serde_json::from_str::<String>(trimmed) {
            return s;
        }
    }
    trimmed.to_string()
}

fn summarize_upstream_body(body: &str) -> String {
    let raw = unescape_if_quoted(body);

    // 优先尝试解析 JSON 并提取核心字段
    if let Ok(v) = serde_json::from_str::<Value>(&raw) {
        let msg = v
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .or_else(|| v.pointer("/message").and_then(|v| v.as_str()))
            .or_else(|| v.pointer("/error").and_then(|v| v.as_str()));

        let code = v
            .pointer("/error/code")
            .and_then(|v| v.as_str())
            .or_else(|| v.pointer("/code").and_then(|v| v.as_str()));

        let typ = v
            .pointer("/error/type")
            .and_then(|v| v.as_str())
            .or_else(|| v.pointer("/type").and_then(|v| v.as_str()));

        let mut parts: Vec<String> = Vec::new();
        if let Some(code) = code {
            parts.push(format!("code={code}"));
        }
        if let Some(typ) = typ {
            parts.push(format!("type={typ}"));
        }
        if let Some(msg) = msg {
            let msg = truncate_with_tail(
                &normalize_whitespace(msg),
                MAX_ERROR_DETAIL_LEN,
                TRUNCATE_TAIL_LEN,
            );
            parts.push(format!("msg={msg}"));
        } else {
            let compact = truncate_with_tail(
                &normalize_whitespace(&raw),
                MAX_ERROR_DETAIL_LEN,
                TRUNCATE_TAIL_LEN,
            );
            parts.push(format!("body={compact}"));
        }

        return parts.join(" ");
    }

    truncate_with_tail(
        &normalize_whitespace(&raw),
        MAX_ERROR_DETAIL_LEN,
        TRUNCATE_TAIL_LEN,
    )
}

fn summarize_generic_message(msg: &str) -> String {
    truncate_with_tail(
        &normalize_whitespace(msg),
        MAX_ERROR_DETAIL_LEN,
        TRUNCATE_TAIL_LEN,
    )
}

/// 将 ProxyError 转换为用户友好的错误消息
pub fn get_error_message(error: &ProxyError) -> String {
    match error {
        ProxyError::UpstreamError { status, body } => {
            if let Some(body) = body {
                format!("上游错误 ({status}): {}", summarize_upstream_body(body))
            } else {
                format!("上游错误 ({status})")
            }
        }
        ProxyError::Timeout(msg) => format!("请求超时: {}", summarize_generic_message(msg)),
        ProxyError::ForwardFailed(msg) => format!("转发失败: {}", summarize_generic_message(msg)),
        ProxyError::NoAvailableProvider => "无可用 Provider".to_string(),
        ProxyError::AllProvidersCircuitOpen => "所有供应商已熔断，无可用渠道".to_string(),
        ProxyError::NoProvidersConfigured => "未配置供应商".to_string(),
        ProxyError::MaxRetriesExceeded => "所有 Provider 都失败，重试耗尽".to_string(),
        ProxyError::ProviderUnhealthy(msg) => {
            format!("Provider 不健康: {}", summarize_generic_message(msg))
        }
        ProxyError::DatabaseError(msg) => format!("数据库错误: {}", summarize_generic_message(msg)),
        ProxyError::TransformError(msg) => {
            format!("请求/响应转换错误: {}", summarize_generic_message(msg))
        }
        _ => summarize_generic_message(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_upstream_error() {
        let error = ProxyError::UpstreamError {
            status: 401,
            body: Some("Unauthorized".to_string()),
        };
        assert_eq!(map_proxy_error_to_status(&error), 401);
    }

    #[test]
    fn test_map_timeout_error() {
        let error = ProxyError::Timeout("Request timeout".to_string());
        assert_eq!(map_proxy_error_to_status(&error), 504);
    }

    #[test]
    fn test_map_connection_error() {
        let error = ProxyError::ForwardFailed("Connection refused".to_string());
        assert_eq!(map_proxy_error_to_status(&error), 502);
    }

    #[test]
    fn test_map_no_provider_error() {
        let error = ProxyError::NoAvailableProvider;
        assert_eq!(map_proxy_error_to_status(&error), 503);
    }

    #[test]
    fn test_map_status_matches_proxy_error_response_semantics() {
        assert_eq!(
            map_proxy_error_to_status(&ProxyError::AuthError("bad token".to_string())),
            401
        );
        assert_eq!(
            map_proxy_error_to_status(&ProxyError::ConfigError("bad config".to_string())),
            400
        );
        assert_eq!(
            map_proxy_error_to_status(&ProxyError::InvalidRequest("bad request".to_string())),
            400
        );
        assert_eq!(
            map_proxy_error_to_status(&ProxyError::TransformError("bad transform".to_string())),
            422
        );
        assert_eq!(
            map_proxy_error_to_status(&ProxyError::StreamIdleTimeout(30)),
            504
        );
    }

    #[test]
    fn test_get_error_message() {
        let error = ProxyError::UpstreamError {
            status: 500,
            body: Some("Internal Server Error".to_string()),
        };
        let msg = get_error_message(&error);
        assert!(msg.contains("上游错误"));
        assert!(msg.contains("500"));
        assert!(msg.contains("Internal Server Error"));
    }
}
