//! 分层转发器
//!
//! 实现核心的分层轮询机制，支持按priority分组、Base URL分组等优化策略

use super::{
    body_filter::filter_private_params_with_whitelist,
    file_logger::get_file_logger,             // 【新增】文件日志器
    hyper_client::ProxyResponse,
    forwarder::{ForwardError, ForwardResult}, // 【重用】直接使用forwarder定义的类型
    model_mapper,
    provider_router::ProviderRouter,
    providers::{get_adapter, ProviderAdapter},
    ProxyError,
};
use crate::{app_config::AppType, database::Database, provider::Provider};
use reqwest::Response;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Headers 黑名单 - 不透传到上游的 Headers
const HEADER_BLACKLIST: &[&str] = &[
    "authorization",
    "x-api-key",
    "host",
    "content-length",
    "transfer-encoding",
    "accept-encoding",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "forwarded",
    "cf-connecting-ip",
    "cf-ipcountry",
    "cf-ray",
    "cf-visitor",
    "true-client-ip",
    "fastly-client-ip",
    "x-azure-clientip",
    "x-azure-fdid",
    "x-azure-ref",
    "akamai-origin-hop",
    "x-akamai-config-log-detail",
    "x-request-id",
    "x-correlation-id",
    "x-trace-id",
    "x-amzn-trace-id",
    "x-b3-traceid",
    "x-b3-spanid",
    "x-b3-parentspanid",
    "x-b3-sampled",
    "traceparent",
    "tracestate",
    "anthropic-beta",
    "anthropic-version",
    "x-forwarded-for",
    "x-real-ip",
];

/// 分层转发器 - 实现智能分层轮询机制
pub struct LayeredForwarder {
    /// 共享的 ProviderRouter（持有熔断器状态）
    router: Arc<ProviderRouter>,
    /// 数据库连接
    _db: Arc<Database>,
    /// 非流式请求超时
    non_streaming_timeout: Duration,
}

impl LayeredForwarder {
    /// 创建新的分层转发器
    pub fn new(router: Arc<ProviderRouter>, db: Arc<Database>, non_streaming_timeout: u64) -> Self {
        Self {
            router,
            _db: db,
            non_streaming_timeout: Duration::from_secs(non_streaming_timeout),
        }
    }

    /// 分层轮询转发请求
    ///
    /// 核心逻辑：
    /// 1. 按priority分组（层级）
    /// 2. 每个层级内按base_url分组
    /// 3. 多轮轮询，每轮每个base_url尝试不同key
    /// 4. 本层级失败后进入下一层级
    pub async fn forward_with_layered_retry(
        &self,
        app_type: &AppType,
        endpoint: &str,
        body: Value,
        headers: axum::http::HeaderMap,
        providers: Vec<Provider>,
    ) -> Result<ForwardResult, ForwardError> {
        if providers.is_empty() {
            return Err(ForwardError {
                error: ProxyError::NoAvailableProvider,
                provider: None,
            });
        }

        let adapter = get_adapter(app_type);
        let app_type_str = app_type.as_str();

        // 步骤1: 按priority分组（使用BTreeMap自动排序）
        let mut by_priority: BTreeMap<usize, Vec<Provider>> = BTreeMap::new();
        for p in providers.into_iter() {
            let priority = p.sort_index.unwrap_or(999999);
            by_priority.entry(priority).or_default().push(p);
        }

        log::debug!("[{}] 分层轮询: {} 个层级", app_type_str, by_priority.len());

        let mut last_error = None;
        let mut last_provider = None;

        // 步骤2: 逐层级尝试
        for (priority, providers_in_level) in by_priority.into_iter() {
            if providers_in_level.is_empty() {
                continue;
            }

            log::debug!(
                "[{}] 尝试层级 {} ({} 个供应商)",
                app_type_str,
                priority,
                providers_in_level.len()
            );

            // 步骤3: 按base_url分组
            let groups = self.group_by_base_url(&providers_in_level, adapter.as_ref());

            log::debug!(
                "[{}] 层级 {} 分为 {} 个base_url组",
                app_type_str,
                priority,
                groups.len()
            );

            // 步骤4: 多轮轮询（每轮每个base_url尝试不同key）
            let max_rounds = self.calculate_max_rounds(&groups);

            for round in 0..max_rounds {
                for (base_url, provider_list) in &groups {
                    if round >= provider_list.len() {
                        continue;
                    }

                    // 轮内选择: 第round个provider (循环)
                    let provider = &provider_list[round];

                    // 检查熔断器
                    let permit = self
                        .router
                        .allow_provider_request(&provider.id, app_type_str)
                        .await;

                    if !permit.allowed {
                        log::debug!("[{}] Provider {} 被熔断器拒绝", app_type_str, provider.name);
                        continue;
                    }

                    log::debug!(
                        "[{}] 层级 {} 第 {} 轮 - 使用Provider: {} (base_url: {})",
                        app_type_str,
                        priority,
                        round + 1,
                        provider.name,
                        base_url
                    );

                    // 尝试转发
                    match self
                        .try_forward(
                            provider,
                            endpoint,
                            &body,
                            &headers,
                            adapter.as_ref(),
                            permit.used_half_open_permit,
                            app_type_str,
                        )
                        .await
                    {
                        Ok(result) => {
                            log::info!(
                                "[{}] 层级 {} 第 {} 轮成功 - Provider: {}",
                                app_type_str,
                                priority,
                                round + 1,
                                provider.name
                            );
                            return Ok(result);
                        }
                        Err(e) => {
                            // 保存错误信息（转为String避免clone）
                            last_error = Some(e.error);
                            last_provider = e.provider.clone();

                            // 判断是否可重试
                            if self.is_retryable(&last_error.as_ref().unwrap()) {
                                log::debug!(
                                    "[{}] Provider {} 失败（可重试），切换下一个",
                                    app_type_str,
                                    provider.name
                                );
                                continue;
                            } else {
                                log::error!(
                                    "[{}] Provider {} 失败（不可重试）: {}",
                                    app_type_str,
                                    provider.name,
                                    last_error.as_ref().unwrap()
                                );
                                return Err(ForwardError {
                                    error: last_error.unwrap(),
                                    provider: last_provider,
                                });
                            }
                        }
                    }
                }
            }

            log::warn!(
                "[{}] 层级 {} 已用尽，切换到下一层级",
                app_type_str,
                priority
            );
        }

        // 所有层级都失败
        log::error!("[{}] 所有层级均失败", app_type_str);

        Err(ForwardError {
            error: last_error.unwrap_or(ProxyError::MaxRetriesExceeded),
            provider: last_provider,
        })
    }

    /// 按base_url分组Provider
    fn group_by_base_url(
        &self,
        providers: &[Provider],
        adapter: &dyn ProviderAdapter,
    ) -> HashMap<String, Vec<Provider>> {
        let mut groups: HashMap<String, Vec<Provider>> = HashMap::new();

        for provider in providers {
            let base_url = self.extract_base_url_key(provider, adapter);
            groups.entry(base_url).or_default().push(provider.clone());
        }

        groups
    }

    /// 提取Provider的base_url作为分组key
    fn extract_base_url_key(&self, provider: &Provider, adapter: &dyn ProviderAdapter) -> String {
        if let Ok(base_url) = adapter.extract_base_url(provider) {
            base_url.trim().trim_end_matches('/').to_string()
        } else {
            // 回退：使用provider_id作为唯一key
            format!("provider:{}", provider.id)
        }
    }

    /// 计算最大轮询轮数
    fn calculate_max_rounds(&self, groups: &HashMap<String, Vec<Provider>>) -> usize {
        groups.values().map(|list| list.len()).max().unwrap_or(1)
    }

    /// 尝试转发单个请求
    async fn try_forward(
        &self,
        provider: &Provider,
        endpoint: &str,
        body: &Value,
        headers: &axum::http::HeaderMap,
        adapter: &dyn ProviderAdapter,
        used_half_open_permit: bool,
        app_type_str: &str,
    ) -> Result<ForwardResult, ForwardError> {
        let start = Instant::now();

        // 提取模型名称用于日志记录
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // 执行HTTP转发
        let result = self
            .forward_http(provider, endpoint, body, headers, adapter)
            .await;

        let latency = start.elapsed().as_millis() as u64;

        match result {
            Ok(response) => {
                // 记录成功到熔断器
                let _ = self
                    .router
                    .record_result(
                        &provider.id,
                        app_type_str,
                        used_half_open_permit,
                        true,
                        None,
                    )
                    .await;

                // 【新增】记录成功到文件日志
                let status_code = response.status().as_u16();
                get_file_logger().log_success(
                    app_type_str,
                    status_code,
                    &provider.name,
                    latency,
                    model,
                );

                log::debug!(
                    "[{}] Provider {} 成功，延迟 {}ms",
                    app_type_str,
                    provider.name,
                    latency
                );

                Ok(ForwardResult {
                    response: ProxyResponse::Reqwest(response),
                    provider: provider.clone(),
                    claude_api_format: None,
                })
            }
            Err(error) => {
                // 记录失败到熔断器
                let _ = self
                    .router
                    .record_result(
                        &provider.id,
                        app_type_str,
                        used_half_open_permit,
                        false,
                        Some(error.to_string()),
                    )
                    .await;

                // 【新增】记录失败到文件日志
                let status_code = super::error_mapper::map_proxy_error_to_status(&error);
                let error_detail = super::error_mapper::get_error_message(&error);
                get_file_logger().log_error(
                    app_type_str,
                    status_code,
                    &provider.name,
                    latency,
                    model,
                    &error_detail,
                );

                log::debug!(
                    "[{}] Provider {} 失败，延迟 {}ms: {}",
                    app_type_str,
                    provider.name,
                    latency,
                    error
                );

                Err(ForwardError {
                    error,
                    provider: Some(provider.clone()),
                })
            }
        }
    }

    /// 执行实际的HTTP转发
    async fn forward_http(
        &self,
        provider: &Provider,
        endpoint: &str,
        body: &Value,
        headers: &axum::http::HeaderMap,
        adapter: &dyn ProviderAdapter,
    ) -> Result<Response, ProxyError> {
        // 1. 提取 base_url
        let base_url = adapter.extract_base_url(provider)?;

        // 2. 检查是否需要格式转换
        let needs_transform = adapter.needs_transform(provider);

        let effective_endpoint =
            if needs_transform && adapter.name() == "Claude" && endpoint == "/v1/messages" {
                "/v1/chat/completions"
            } else {
                endpoint
            };

        // 3. 构建完整URL
        let url = adapter.build_url(&base_url, effective_endpoint);

        // 4. 应用模型映射
        let (mapped_body, _, _) = model_mapper::apply_model_mapping(body.clone(), provider);

        // 5. 转换请求体（如果需要）
        let request_body = if needs_transform {
            adapter.transform_request(mapped_body, provider)?
        } else {
            mapped_body
        };

        // 6. 过滤私有参数
        let filtered_body = filter_private_params_with_whitelist(request_body, &[]);

        // 7. 获取HTTP客户端并构建请求
        let client = super::http_client::get();
        let mut request = client.post(&url);

        // 8. 设置超时
        if !self.non_streaming_timeout.is_zero() {
            request = request.timeout(self.non_streaming_timeout);
        }

        // 9. 添加headers（过滤黑名单）
        for (key, value) in headers {
            if HEADER_BLACKLIST
                .iter()
                .any(|h| key.as_str().eq_ignore_ascii_case(h))
            {
                continue;
            }
            request = request.header(key, value);
        }

        // 10. 处理 anthropic-beta Header（仅 Claude）
        if adapter.name() == "Claude" {
            const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
            let beta_value = if let Some(beta) = headers.get("anthropic-beta") {
                if let Ok(beta_str) = beta.to_str() {
                    if beta_str.contains(CLAUDE_CODE_BETA) {
                        beta_str.to_string()
                    } else {
                        format!("{CLAUDE_CODE_BETA},{beta_str}")
                    }
                } else {
                    CLAUDE_CODE_BETA.to_string()
                }
            } else {
                CLAUDE_CODE_BETA.to_string()
            };
            request = request.header("anthropic-beta", &beta_value);
        }

        // 11. 客户端 IP 透传
        if let Some(xff) = headers.get("x-forwarded-for") {
            if let Ok(xff_str) = xff.to_str() {
                request = request.header("x-forwarded-for", xff_str);
            }
        }
        if let Some(real_ip) = headers.get("x-real-ip") {
            if let Ok(real_ip_str) = real_ip.to_str() {
                request = request.header("x-real-ip", real_ip_str);
            }
        }

        // 12. 禁用压缩
        request = request.header("accept-encoding", "identity");

        // 13. 添加认证头
        if let Some(auth) = adapter.extract_auth(provider) {
            for (name, value) in adapter.get_auth_headers(&auth) {
                request = request.header(name, value);
            }
        }

        // 14. anthropic-version 统一处理（仅 Claude）
        if adapter.name() == "Claude" {
            let version_str = headers
                .get("anthropic-version")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("2023-06-01");
            request = request.header("anthropic-version", version_str);
        }

        // 15. 发送请求
        let response = request.json(&filtered_body).send().await.map_err(|e| {
            if e.is_timeout() {
                ProxyError::Timeout(format!("请求超时: {e}"))
            } else if e.is_connect() {
                ProxyError::ForwardFailed(format!("连接失败: {e}"))
            } else {
                ProxyError::ForwardFailed(e.to_string())
            }
        })?;

        // 16. 检查响应状态
        let status = response.status();

        if status.is_success() {
            Ok(response)
        } else {
            let status_code = status.as_u16();
            let body_text = response.text().await.ok();

            Err(ProxyError::UpstreamError {
                status: status_code,
                body: body_text,
            })
        }
    }

    /// 判断错误是否可重试
    fn is_retryable(&self, error: &ProxyError) -> bool {
        matches!(
            error,
            ProxyError::Timeout(_)
                | ProxyError::ForwardFailed(_)
                | ProxyError::ProviderUnhealthy(_)
                | ProxyError::UpstreamError { .. }
                | ProxyError::ConfigError(_)
                | ProxyError::TransformError(_)
                | ProxyError::AuthError(_)
                | ProxyError::StreamIdleTimeout(_)
        )
    }
}
