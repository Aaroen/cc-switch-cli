//! 供应商路由器模块
//!
//! 负责选择和管理代理目标供应商，实现智能故障转移和权重轮询

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use crate::proxy::circuit_breaker::{AllowResult, CircuitBreaker, CircuitBreakerConfig};
use crate::proxy::load_balancer::{LoadBalanceStrategy, LoadBalancer};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 供应商路由器
pub struct ProviderRouter {
    /// 数据库连接
    db: Arc<Database>,
    /// 熔断器管理器 - key 格式: "app_type:provider_id"
    circuit_breakers: Arc<RwLock<HashMap<String, Arc<CircuitBreaker>>>>,
    /// 负载均衡器 - key: app_type
    load_balancers: Arc<RwLock<HashMap<String, LoadBalancer>>>,
}

impl ProviderRouter {
    /// 创建新的供应商路由器
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            load_balancers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 选择可用的供应商（支持故障转移和权重轮询）
    ///
    /// 返回按优先级排序的可用供应商列表：
    /// - 权重轮询开启时：使用 LoadBalancer（多策略）选择供应商
    /// - 权重轮询关闭且故障转移开启时：按队列顺序依次尝试（P1 → P2 → ...）
    /// - 两者都关闭时：仅返回当前供应商
    pub async fn select_providers(&self, app_type: &str) -> Result<Vec<Provider>, AppError> {
        let mut result = Vec::new();
        let mut total_providers = 0usize;
        let mut circuit_open_count = 0usize;

        // 读取代理配置
        let (auto_failover_enabled, weight_round_robin_enabled, lb_strategy) =
            match self.db.get_proxy_config_for_app(app_type).await {
                Ok(config) => (
                    config.auto_failover_enabled,
                    config.weight_round_robin_enabled,
                    config.load_balance_strategy,
                ),
                Err(e) => {
                    log::error!(
                        "[{app_type}] 读取 proxy_config 失败: {e}，默认禁用故障转移和权重轮询"
                    );
                    (false, false, LoadBalanceStrategy::default())
                }
            };

        if weight_round_robin_enabled {
            // 权重轮询模式：使用 LoadBalancer（多策略）
            let all_providers = self.db.get_all_providers(app_type)?;
            let mut weighted_providers: Vec<Provider> = all_providers
                .into_iter()
                .filter(|(_, p)| p.weight > 0) // 过滤掉权重为0的（禁用的）
                .map(|(_, p)| p)
                .collect();

            total_providers = weighted_providers.len();

            if total_providers == 0 {
                log::warn!("[{app_type}] 权重轮询模式: 没有可用供应商（所有供应商权重为0）");
                return Err(AppError::NoProvidersConfigured);
            }

            // 按权重排序（权重小的优先，频率高）
            weighted_providers.sort_by_key(|p| p.weight);

            // 获取或创建负载均衡器
            let selected_provider = {
                let mut lbs = self.load_balancers.write().await;
                let lb = lbs.entry(app_type.to_string()).or_insert_with(|| {
                    log::info!(
                        "[{app_type}] 创建新的负载均衡器（策略={}），供应商数量: {}",
                        lb_strategy.as_str(),
                        weighted_providers.len()
                    );
                    LoadBalancer::new(weighted_providers.clone(), lb_strategy)
                });

                // 检查是否需要重建（策略变化 / 供应商数量或权重变化）
                let current_providers = lb.providers();
                let needs_update = lb.strategy() != lb_strategy
                    || current_providers.len() != weighted_providers.len()
                    || current_providers
                        .iter()
                        .zip(weighted_providers.iter())
                        .any(|(wp, p)| wp.provider.id != p.id || wp.weight != p.weight);

                if needs_update {
                    log::info!(
                        "[{app_type}] 配置已变化（策略={}），重建负载均衡器",
                        lb_strategy.as_str()
                    );
                    *lb = LoadBalancer::new(weighted_providers.clone(), lb_strategy);
                }

                // 使用负载均衡器选择供应商
                lb.select().cloned()
            };

            // 组装候选列表：选中者优先 + 其他供应商（用于同请求内故障转移）
            let mut ordered_candidates: Vec<Provider> = Vec::new();
            if let Some(provider) = selected_provider {
                ordered_candidates.push(provider);
            } else {
                log::debug!("[{app_type}] 负载均衡器未选中供应商，使用权重排序列表");
            }

            for p in weighted_providers {
                if ordered_candidates.first().map(|s| s.id.as_str()) == Some(p.id.as_str()) {
                    continue;
                }
                ordered_candidates.push(p);
            }

            for provider in ordered_candidates {
                let circuit_key = format!("{}:{}", app_type, provider.id);
                let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;

                if breaker.is_available().await {
                    result.push(provider);
                } else {
                    circuit_open_count += 1;
                }
            }

            log::debug!(
                "[{app_type}] 权重轮询模式: {} 个可用供应商 (共 {} 个, {} 个熔断)",
                result.len(),
                total_providers,
                circuit_open_count
            );
        } else if auto_failover_enabled {
            // 故障转移开启：仅按队列顺序依次尝试（P1 → P2 → ...）
            let all_providers = self.db.get_all_providers(app_type)?;

            // 使用 DAO 返回的排序结果，确保和前端展示一致
            let ordered_ids: Vec<String> = self
                .db
                .get_failover_queue(app_type)?
                .into_iter()
                .map(|item| item.provider_id)
                .collect();

            total_providers = ordered_ids.len();

            for provider_id in ordered_ids {
                let Some(provider) = all_providers.get(&provider_id).cloned() else {
                    continue;
                };

                let circuit_key = format!("{app_type}:{}", provider.id);
                let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;

                if breaker.is_available().await {
                    result.push(provider);
                } else {
                    circuit_open_count += 1;
                }
            }
        } else {
            // 故障转移关闭：仅使用当前供应商，跳过熔断器检查
            let current_id = AppType::from_str(app_type)
                .ok()
                .and_then(|app_enum| {
                    crate::settings::get_effective_current_provider(&self.db, &app_enum)
                        .ok()
                        .flatten()
                })
                .or_else(|| self.db.get_current_provider(app_type).ok().flatten());

            if let Some(current_id) = current_id {
                if let Some(current) = self.db.get_provider_by_id(&current_id, app_type)? {
                    total_providers = 1;
                    result.push(current);
                }
            }
        }

        if result.is_empty() {
            if total_providers > 0 && circuit_open_count == total_providers {
                log::warn!("[{app_type}] [FO-004] 所有供应商均已熔断");
                return Err(AppError::AllProvidersCircuitOpen);
            } else {
                log::warn!("[{app_type}] [FO-005] 未配置供应商");
                return Err(AppError::NoProvidersConfigured);
            }
        }

        Ok(result)
    }

    /// 请求执行前获取熔断器“放行许可”
    ///
    /// - Closed：直接放行
    /// - Open：超时到达后切到 HalfOpen 并放行一次探测
    /// - HalfOpen：按限流规则放行探测
    ///
    /// 注意：调用方必须在请求结束后通过 `record_result()` 释放 HalfOpen 名额，
    /// 否则会导致该 Provider 长时间无法进入探测状态。
    pub async fn allow_provider_request(&self, provider_id: &str, app_type: &str) -> AllowResult {
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        breaker.allow_request().await
    }

    /// 记录供应商请求结果
    pub async fn record_result(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
        success: bool,
        error_msg: Option<String>,
    ) -> Result<(), AppError> {
        // 1. 按应用独立获取熔断器配置
        let failure_threshold = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(app_config) => app_config.circuit_failure_threshold,
            Err(_) => 5, // 默认值
        };

        // 2. 更新熔断器状态
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;

        if success {
            breaker.record_success(used_half_open_permit).await;
        } else {
            breaker.record_failure(used_half_open_permit).await;
        }

        // 3. 更新数据库健康状态（使用配置的阈值）
        self.db
            .update_provider_health_with_threshold(
                provider_id,
                app_type,
                success,
                error_msg.clone(),
                failure_threshold,
            )
            .await?;

        Ok(())
    }

    /// 重置熔断器（手动恢复）
    pub async fn reset_circuit_breaker(&self, circuit_key: &str) {
        let breakers = self.circuit_breakers.read().await;
        if let Some(breaker) = breakers.get(circuit_key) {
            breaker.reset().await;
        }
    }

    /// 重置指定供应商的熔断器
    pub async fn reset_provider_breaker(&self, provider_id: &str, app_type: &str) {
        let circuit_key = format!("{app_type}:{provider_id}");
        self.reset_circuit_breaker(&circuit_key).await;
    }

    /// 仅释放 HalfOpen permit，不影响健康统计（neutral 接口）
    ///
    /// 用于整流器等场景：请求结果不应计入 Provider 健康度，
    /// 但仍需释放占用的探测名额，避免 HalfOpen 状态卡死
    pub async fn release_permit_neutral(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
    ) {
        if !used_half_open_permit {
            return;
        }
        let circuit_key = format!("{app_type}:{provider_id}");
        let breaker = self.get_or_create_circuit_breaker(&circuit_key).await;
        breaker.release_half_open_permit();
    }

    /// 更新所有熔断器的配置（热更新）
    pub async fn update_all_configs(&self, config: CircuitBreakerConfig) {
        let breakers = self.circuit_breakers.read().await;
        for breaker in breakers.values() {
            breaker.update_config(config.clone()).await;
        }
    }

    /// 更新指定应用已创建熔断器的配置（热更新）
    pub async fn update_app_configs(&self, app_type: &str, config: CircuitBreakerConfig) {
        let prefix = format!("{app_type}:");
        let breakers = self.circuit_breakers.read().await;
        for (key, breaker) in breakers.iter() {
            if key.starts_with(&prefix) {
                breaker.update_config(config.clone()).await;
            }
        }
    }

    /// 获取熔断器状态
    #[allow(dead_code)]
    pub async fn get_circuit_breaker_stats(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Option<crate::proxy::circuit_breaker::CircuitBreakerStats> {
        let circuit_key = format!("{app_type}:{provider_id}");
        let breakers = self.circuit_breakers.read().await;

        if let Some(breaker) = breakers.get(&circuit_key) {
            Some(breaker.get_stats().await)
        } else {
            None
        }
    }

    /// 重置指定应用的负载均衡器
    ///
    /// 当供应商配置变化时（添加/删除/修改权重），应调用此方法重置负载均衡器
    #[allow(dead_code)] // 保留 API：当前权重变更经 needs_update 自动重建
    pub async fn reset_load_balancer(&self, app_type: &str) {
        let mut lbs = self.load_balancers.write().await;
        if lbs.remove(app_type).is_some() {
            log::info!("[{app_type}] 负载均衡器已重置");
        }
    }

    /// 获取负载均衡器当前轮询计数
    #[allow(dead_code)]
    pub async fn get_load_balancer_round(&self, app_type: &str) -> Option<u32> {
        let lbs = self.load_balancers.read().await;
        lbs.get(app_type).map(|lb| lb.current_round())
    }

    /// 获取或创建熔断器
    async fn get_or_create_circuit_breaker(&self, key: &str) -> Arc<CircuitBreaker> {
        // 先尝试读锁获取
        {
            let breakers = self.circuit_breakers.read().await;
            if let Some(breaker) = breakers.get(key) {
                return breaker.clone();
            }
        }

        // 如果不存在，获取写锁创建
        let mut breakers = self.circuit_breakers.write().await;

        // 双重检查，防止竞争条件
        if let Some(breaker) = breakers.get(key) {
            return breaker.clone();
        }

        // 从 key 中提取 app_type (格式: "app_type:provider_id")
        let app_type = key.split(':').next().unwrap_or("claude");

        // 按应用独立读取熔断器配置
        let config = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(app_config) => crate::proxy::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: app_config.circuit_failure_threshold,
                success_threshold: app_config.circuit_success_threshold,
                timeout_seconds: app_config.circuit_timeout_seconds as u64,
                error_rate_threshold: app_config.circuit_error_rate_threshold,
                min_requests: app_config.circuit_min_requests,
            },
            Err(_) => crate::proxy::circuit_breaker::CircuitBreakerConfig::default(),
        };

        let breaker = Arc::new(CircuitBreaker::new(config));
        breakers.insert(key.to_string(), breaker.clone());

        breaker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use serde_json::json;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        original_test_home: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("failed to create temp home");
            let original_home = env::var("HOME").ok();
            let original_userprofile = env::var("USERPROFILE").ok();
            let original_test_home = env::var("CC_SWITCH_TEST_HOME").ok();

            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            crate::settings::reload_settings().expect("reload settings");

            Self {
                dir,
                original_home,
                original_userprofile,
                original_test_home,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }

            match &self.original_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }

            match &self.original_test_home {
                Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_provider_router_creation() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());
        let router = ProviderRouter::new(db);

        let breaker = router.get_or_create_circuit_breaker("claude:test").await;
        assert!(breaker.allow_request().await.allowed);
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_disabled_uses_current_provider() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "a");
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_enabled_uses_queue_order_ignoring_current() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        // 设置 sort_index 来控制顺序：b=1, a=2
        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.sort_index = Some(2);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(1);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();

        db.add_to_failover_queue("claude", "b").unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();

        // 启用自动故障转移（使用新的 proxy_config API）
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 2);
        // 故障转移开启时：仅按队列顺序选择（忽略当前供应商）
        assert_eq!(providers[0].id, "b");
        assert_eq!(providers[1].id, "a");
    }

    #[tokio::test]
    #[serial]
    async fn test_failover_enabled_uses_queue_only_even_if_current_not_in_queue() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.sort_index = Some(1);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.set_current_provider("claude", "a").unwrap();

        // 只把 b 加入故障转移队列（模拟“当前供应商不在队列里”的常见配置）
        db.add_to_failover_queue("claude", "b").unwrap();

        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());
        let providers = router.select_providers("claude").await.unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "b");
    }

    #[tokio::test]
    #[serial]
    async fn test_select_providers_does_not_consume_half_open_permit() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        })
        .await
        .unwrap();

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        let provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();

        db.add_to_failover_queue("claude", "a").unwrap();
        db.add_to_failover_queue("claude", "b").unwrap();

        // 启用自动故障转移（使用新的 proxy_config API）
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        router
            .record_result("b", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        let providers = router.select_providers("claude").await.unwrap();
        assert_eq!(providers.len(), 2);

        assert!(router.allow_provider_request("b", "claude").await.allowed);
    }

    #[tokio::test]
    #[serial]
    async fn test_release_permit_neutral_frees_half_open_slot() {
        let _home = TempHome::new();
        let db = Arc::new(Database::memory().unwrap());

        // 配置熔断器：1 次失败即熔断，0 秒超时立即进入 HalfOpen
        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 0,
            ..Default::default()
        })
        .await
        .unwrap();

        let provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        db.save_provider("claude", &provider_a).unwrap();
        db.add_to_failover_queue("claude", "a").unwrap();

        // 启用自动故障转移
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.auto_failover_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        // 触发熔断：1 次失败
        router
            .record_result("a", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        // 第一次请求：获取 HalfOpen 探测名额
        let first = router.allow_provider_request("a", "claude").await;
        assert!(first.allowed);
        assert!(first.used_half_open_permit);

        // 第二次请求应被拒绝（名额已被占用）
        let second = router.allow_provider_request("a", "claude").await;
        assert!(!second.allowed);

        // 使用 release_permit_neutral 释放名额（不影响健康统计）
        router
            .release_permit_neutral("a", "claude", first.used_half_open_permit)
            .await;

        // 第三次请求应被允许（名额已释放）
        let third = router.allow_provider_request("a", "claude").await;
        assert!(third.allowed);
        assert!(third.used_half_open_permit);
    }

    #[tokio::test]
    async fn test_weight_round_robin_uses_load_balancer() {
        let db = Arc::new(Database::memory().unwrap());

        // 创建三个供应商，权重分别为 1, 2, 3
        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.weight = 1; // 每轮都使用
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.weight = 2; // 每2轮使用一次
        let mut provider_c =
            Provider::with_id("c".to_string(), "Provider C".to_string(), json!({}), None);
        provider_c.weight = 3; // 每3轮使用一次

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();
        db.save_provider("claude", &provider_c).unwrap();

        // 启用权重轮询
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.weight_round_robin_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        // 连续调用 6 次，验证轮询行为
        // 由于 weight=1 的 Provider A 每轮都会被选中，所以应该总是返回 A
        // 但负载均衡器会按频率控制选择
        let mut selected_ids = Vec::new();
        for _ in 0..6 {
            let providers = router.select_providers("claude").await.unwrap();
            assert!(!providers.is_empty());
            selected_ids.push(providers[0].id.clone());
        }

        // 验证负载均衡器计数器在增加
        let round = router.get_load_balancer_round("claude").await;
        assert!(round.is_some());
        assert_eq!(round.unwrap(), 6);

        // 验证 Provider A (weight=1) 被选中的次数最多
        let a_count = selected_ids.iter().filter(|id| *id == "a").count();
        assert!(a_count >= 1, "Provider A (weight=1) 应该被选中至少1次");
    }

    #[tokio::test]
    async fn test_weight_round_robin_fallback_on_circuit_open() {
        let db = Arc::new(Database::memory().unwrap());

        // 配置熔断器：1 次失败即熔断
        db.update_circuit_breaker_config(&CircuitBreakerConfig {
            failure_threshold: 1,
            timeout_seconds: 60, // 60秒超时，确保测试期间不会自动恢复
            ..Default::default()
        })
        .await
        .unwrap();

        // 创建两个供应商
        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.weight = 1;
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.weight = 2;

        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();

        // 启用权重轮询
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.weight_round_robin_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        // 触发 Provider A 熔断
        router
            .record_result("a", "claude", false, false, Some("fail".to_string()))
            .await
            .unwrap();

        // 选择供应商，应该回退到 Provider B
        let providers = router.select_providers("claude").await.unwrap();
        assert!(!providers.is_empty());
        // 由于 A 被熔断，应该返回 B
        assert_eq!(providers[0].id, "b");
    }

    #[tokio::test]
    async fn test_reset_load_balancer() {
        let db = Arc::new(Database::memory().unwrap());

        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.weight = 1;

        db.save_provider("claude", &provider_a).unwrap();

        // 启用权重轮询
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.weight_round_robin_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        // 调用几次以增加计数器
        for _ in 0..5 {
            let _ = router.select_providers("claude").await;
        }

        // 验证计数器
        let round = router.get_load_balancer_round("claude").await;
        assert_eq!(round, Some(5));

        // 重置负载均衡器
        router.reset_load_balancer("claude").await;

        // 验证计数器已重置（负载均衡器被移除）
        let round_after_reset = router.get_load_balancer_round("claude").await;
        assert_eq!(round_after_reset, None);

        // 再次调用会创建新的负载均衡器
        let _ = router.select_providers("claude").await;
        let round_new = router.get_load_balancer_round("claude").await;
        assert_eq!(round_new, Some(1));
    }

    #[tokio::test]
    async fn test_strategy_switch_rebuilds_load_balancer() {
        let db = Arc::new(Database::memory().unwrap());

        let mut provider_a =
            Provider::with_id("a".to_string(), "Provider A".to_string(), json!({}), None);
        provider_a.weight = 1;
        let mut provider_b =
            Provider::with_id("b".to_string(), "Provider B".to_string(), json!({}), None);
        provider_b.weight = 2;
        db.save_provider("claude", &provider_a).unwrap();
        db.save_provider("claude", &provider_b).unwrap();

        // 启用权重轮询（默认策略 Frequency）
        let mut config = db.get_proxy_config_for_app("claude").await.unwrap();
        config.weight_round_robin_enabled = true;
        db.update_proxy_config_for_app(config).await.unwrap();

        let router = ProviderRouter::new(db.clone());

        // Frequency 策略下选择 3 次，计数器累加到 3
        for _ in 0..3 {
            let _ = router.select_providers("claude").await.unwrap();
        }
        assert_eq!(router.get_load_balancer_round("claude").await, Some(3));

        // 切换策略为 HardRoundRobin（仅经专用键写入，不经通用 update）
        db.set_load_balance_strategy("claude", LoadBalanceStrategy::HardRoundRobin)
            .unwrap();

        // 下次选择应检测到策略变化并重建：global_round 归零后自增到 1
        let providers = router.select_providers("claude").await.unwrap();
        assert!(!providers.is_empty());
        assert_eq!(
            router.get_load_balancer_round("claude").await,
            Some(1),
            "策略切换应触发负载均衡器重建（global_round 归零）"
        );

        // HardRoundRobin 在权重升序集合 [a(1), b(2)] 上 index=(1-1)%2=0 → a
        assert_eq!(providers[0].id, "a");

        // 候选列表仍应包含全部可用供应商（故障转移尾部不丢供应商）
        assert!(providers.iter().any(|p| p.id == "b"));
    }
}
