//! Web 控制台命令网关。
//!
//! 将 `/api/invoke/:command` 映射到与对应 `#[tauri::command]` **完全相同**的后端逻辑：
//! 直接调用 `ProviderService` / `ProxyService` / `Database`，对含 AppHandle 副作用的命令
//! 复用其核心逻辑并跳过事件/托盘（Web 面板通过 react-query 轮询刷新）。
//!
//! 参数按前端 Tauri 习惯以 camelCase 传入（providerId/appType/...）。
//! 未实现的命令返回明确错误，前端对应功能报错但界面不崩溃。

use crate::app_config::AppType;
use crate::provider::{Provider, UniversalProvider};
use crate::proxy::load_balancer::LoadBalanceStrategy;
use crate::proxy::types::{AppProxyConfig, GlobalProxyConfig, ProxyConfig};
use crate::services::usage_stats::LogFilters;
use crate::services::{ProviderService, ProviderSortUpdate};
use crate::store::AppState;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::str::FromStr;
use std::sync::Arc;

// 合并官方上游后新增功能域网关所需类型
use crate::app_config::{McpApps, McpServer};
use crate::deeplink::{
    import_mcp_from_deeplink, import_prompt_from_deeplink, import_provider_from_deeplink,
    import_skill_from_deeplink, parse_deeplink_url, DeepLinkImportRequest,
};
use crate::prompt::Prompt;
use crate::provider::ClaudeDesktopMode;
use crate::proxy::providers::codex_oauth_auth::CodexOAuthManager;
use crate::proxy::providers::copilot_auth::{CopilotAuthError, CopilotAuthManager};
use crate::proxy::types::{LogConfig, OptimizerConfig, RectifierConfig};
use crate::proxy::CircuitBreakerConfig;
use crate::services::env_checker::EnvConflict;
use crate::services::skill::{
    DiscoverableSkill, ImportSkillSelection, SkillRepo, SkillService, SkillStorageLocation,
};
use crate::services::stream_check::StreamCheckConfig;
use crate::services::subscription::{query_codex_quota, CredentialStatus, SubscriptionQuota};
use crate::services::usage_stats::UsageSummaryByApp;
use crate::services::{McpService, PromptService, SpeedtestService};
use crate::session_manager;
use std::sync::OnceLock;

/// 提取并反序列化指定参数键（camelCase）。
fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    let v = args.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| format!("参数 {key} 无效: {e}"))
}

/// 将结果序列化为 JSON 信封 data。
fn ok<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// 进程级 GitHub Copilot 认证管理器（Web 控制台专用）。
///
/// GUI/代理使用 Tauri 托管的 `CopilotAuthState` 单例，Web 面板无 AppHandle 无法触达，
/// 故在面板进程内复用同一磁盘凭据目录构造一个进程级单例，读写同一份 `copilot_auth.json`
/// （与 GUI 共享磁盘多账号 store），跨调用持久化内存 token/models 缓存。
fn copilot_manager() -> &'static CopilotAuthManager {
    static MANAGER: OnceLock<CopilotAuthManager> = OnceLock::new();
    MANAGER.get_or_init(|| CopilotAuthManager::new(crate::config::get_app_config_dir()))
}

/// 进程级 `CopilotAuthState`（Web 控制台专用）。
///
/// 供需要 `State<CopilotAuthState>` 的核心逻辑（如 `query_provider_usage_inner`）在无
/// AppHandle 的面板进程内复用。与 `copilot_manager()` 一样读写同一磁盘凭据目录。
fn copilot_state() -> &'static crate::commands::CopilotAuthState {
    use crate::commands::CopilotAuthState;
    static STATE: OnceLock<CopilotAuthState> = OnceLock::new();
    STATE.get_or_init(|| {
        CopilotAuthState(Arc::new(tokio::sync::RwLock::new(CopilotAuthManager::new(
            crate::config::get_app_config_dir(),
        ))))
    })
}

pub async fn dispatch(app: &Arc<AppState>, command: &str, args: Value) -> Result<Value, String> {
    match command {
        // ==================== 代理状态 / 控制 ====================
        "get_proxy_status" => ok(app.proxy_service.get_status().await?),
        "is_proxy_running" => ok(app.proxy_service.is_running().await),
        "is_live_takeover_active" => ok(app.proxy_service.is_takeover_active().await?),
        "get_proxy_takeover_status" => ok(app.proxy_service.get_takeover_status().await?),
        "start_proxy_server" => ok(app.proxy_service.start().await?),
        "stop_proxy_with_restore" => ok(app.proxy_service.stop_with_restore().await?),
        "set_proxy_takeover_for_app" => {
            let app_type: String = arg(&args, "appType")?;
            let enabled: bool = arg(&args, "enabled")?;
            app.proxy_service
                .set_takeover_for_app(&app_type, enabled)
                .await?;
            ok(Value::Null)
        }

        // ==================== 代理配置 ====================
        "get_proxy_config" => ok(app.proxy_service.get_config().await?),
        "update_proxy_config" => {
            let config: ProxyConfig = arg(&args, "config")?;
            app.proxy_service.update_config(&config).await?;
            ok(Value::Null)
        }
        "get_global_proxy_config" => ok(app
            .db
            .get_global_proxy_config()
            .await
            .map_err(|e| e.to_string())?),
        "get_proxy_config_for_app" => {
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_proxy_config_for_app(&app_type)
                .await
                .map_err(|e| e.to_string())?)
        }
        "update_proxy_config_for_app" => {
            // 注意：load_balance_strategy 不经此通道写入（避免回写覆盖），由专用命令处理
            let config: AppProxyConfig = arg(&args, "config")?;
            app.db
                .update_proxy_config_for_app(config)
                .await
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }

        // ==================== 负载均衡策略（本 fork 新增）====================
        "get_load_balance_strategy" => {
            let app_type: String = arg(&args, "appType")?;
            let s = app
                .db
                .get_load_balance_strategy(&app_type)
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            ok(s.as_str().to_string())
        }
        "set_load_balance_strategy" => {
            let app_type: String = arg(&args, "appType")?;
            let strategy: String = arg(&args, "strategy")?;
            AppType::from_str(&app_type).map_err(|e| e.to_string())?;
            let parsed = strategy.parse::<LoadBalanceStrategy>().map_err(|_| {
                format!("无效的负载均衡策略: {strategy}（frequency / weighted_random / hard_round_robin）")
            })?;
            app.db
                .set_load_balance_strategy(&app_type, parsed)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }

        // ==================== 供应商 ====================
        "get_providers" => {
            let app_str: String = arg(&args, "app")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(ProviderService::list(app.as_ref(), app_type).map_err(|e| e.to_string())?)
        }
        "get_current_provider" => {
            let app_str: String = arg(&args, "app")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(ProviderService::current(app.as_ref(), app_type).map_err(|e| e.to_string())?)
        }
        "update_provider_weight" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let weight: u32 = arg(&args, "weight")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            app.db
                .update_provider_weight(app_type.as_str(), &id, weight)
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "switch_proxy_provider" => {
            let app_type: String = arg(&args, "appType")?;
            let provider_id: String = arg(&args, "providerId")?;
            // 复用命令逻辑：禁止接管模式下切到官方供应商
            let provider = app
                .db
                .get_provider_by_id(&provider_id, &app_type)
                .map_err(|e| format!("读取供应商失败: {e}"))?
                .ok_or_else(|| format!("供应商不存在: {provider_id}"))?;
            if provider.category.as_deref() == Some("official") {
                return Err("代理接管模式下不能切换到官方供应商".to_string());
            }
            app.proxy_service
                .switch_proxy_target(&app_type, &provider_id)
                .await?;
            ok(Value::Null)
        }

        // ==================== 故障转移 / 熔断器 ====================
        "get_provider_health" => {
            let provider_id: String = arg(&args, "providerId")?;
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_provider_health(&provider_id, &app_type)
                .await
                .map_err(|e| e.to_string())?)
        }
        "get_circuit_breaker_stats" => ok(Value::Null), // 与现有命令一致：当前为占位
        "get_failover_queue" => {
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_failover_queue(&app_type)
                .map_err(|e| e.to_string())?)
        }
        "get_available_providers_for_failover" => {
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_available_providers_for_failover(&app_type)
                .map_err(|e| e.to_string())?)
        }
        "add_to_failover_queue" => {
            let app_type: String = arg(&args, "appType")?;
            let provider_id: String = arg(&args, "providerId")?;
            app.db
                .add_to_failover_queue(&app_type, &provider_id)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "remove_from_failover_queue" => {
            let app_type: String = arg(&args, "appType")?;
            let provider_id: String = arg(&args, "providerId")?;
            app.db
                .remove_from_failover_queue(&app_type, &provider_id)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "get_auto_failover_enabled" => {
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_proxy_config_for_app(&app_type)
                .await
                .map(|c| c.auto_failover_enabled)
                .map_err(|e| e.to_string())?)
        }
        "set_auto_failover_enabled" => set_auto_failover_enabled(app, args).await,
        "reset_circuit_breaker" => reset_circuit_breaker(app, args).await,

        // ==================== 使用统计仪表盘 ====================
        "get_usage_summary" => {
            let start_date: Option<i64> = arg(&args, "startDate")?;
            let end_date: Option<i64> = arg(&args, "endDate")?;
            let app_type: Option<String> = arg(&args, "appType")?;
            ok(app
                .db
                .get_usage_summary(start_date, end_date, app_type.as_deref())
                .map_err(|e| e.to_string())?)
        }
        "get_usage_date_bounds" => {
            ok(app.db.get_usage_date_bounds().map_err(|e| e.to_string())?)
        }
        "get_usage_trends" => {
            let start_date: Option<i64> = arg(&args, "startDate")?;
            let end_date: Option<i64> = arg(&args, "endDate")?;
            let app_type: Option<String> = arg(&args, "appType")?;
            ok(app
                .db
                .get_daily_trends(start_date, end_date, app_type.as_deref())
                .map_err(|e| e.to_string())?)
        }
        "get_provider_stats" => {
            let start_date: Option<i64> = arg(&args, "startDate")?;
            let end_date: Option<i64> = arg(&args, "endDate")?;
            let app_type: Option<String> = arg(&args, "appType")?;
            ok(app
                .db
                .get_provider_stats(start_date, end_date, app_type.as_deref())
                .map_err(|e| e.to_string())?)
        }
        "get_model_stats" => {
            let start_date: Option<i64> = arg(&args, "startDate")?;
            let end_date: Option<i64> = arg(&args, "endDate")?;
            let app_type: Option<String> = arg(&args, "appType")?;
            ok(app
                .db
                .get_model_stats(start_date, end_date, app_type.as_deref())
                .map_err(|e| e.to_string())?)
        }
        // 按供应商查询脚本型用量（供应商卡片/托盘）。复用 #[tauri::command]
        // queryProviderUsage 的核心逻辑，跳过 usage_cache 写入 / emit / 托盘刷新
        // （均依赖 AppHandle，面板经 react-query 轮询刷新）。
        "queryProviderUsage" => {
            let provider_id: String = arg(&args, "providerId")?;
            let app_str: String = arg(&args, "app")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(crate::commands::query_provider_usage_inner(
                app.as_ref(),
                copilot_state(),
                app_type,
                &provider_id,
            )
            .await?)
        }
        // 用量脚本测试（供应商表单的“测试脚本”按钮）。与 #[tauri::command]
        // testUsageScript 调用同一 ProviderService 核心逻辑，无 AppHandle 依赖。
        "testUsageScript" => {
            let provider_id: String = arg(&args, "providerId")?;
            let app_str: String = arg(&args, "app")?;
            let script_code: String = arg(&args, "scriptCode")?;
            let timeout: Option<u64> = arg(&args, "timeout")?;
            let api_key: Option<String> = arg(&args, "apiKey")?;
            let base_url: Option<String> = arg(&args, "baseUrl")?;
            let access_token: Option<String> = arg(&args, "accessToken")?;
            let user_id: Option<String> = arg(&args, "userId")?;
            let template_type: Option<String> = arg(&args, "templateType")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(ProviderService::test_usage_script(
                app.as_ref(),
                app_type,
                &provider_id,
                &script_code,
                timeout.unwrap_or(10),
                api_key.as_deref(),
                base_url.as_deref(),
                access_token.as_deref(),
                user_id.as_deref(),
                template_type.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?)
        }
        "get_request_logs" => {
            let filters: LogFilters = arg(&args, "filters")?;
            let page: u32 = arg(&args, "page")?;
            let page_size: u32 = arg(&args, "pageSize")?;
            ok(app
                .db
                .get_request_logs(&filters, page, page_size)
                .map_err(|e| e.to_string())?)
        }
        "get_request_detail" => {
            let request_id: String = arg(&args, "requestId")?;
            ok(app
                .db
                .get_request_detail(&request_id)
                .map_err(|e| e.to_string())?)
        }
        "get_model_pricing" => get_model_pricing(app),
        "update_model_pricing" => {
            let model_id: String = arg(&args, "modelId")?;
            let display_name: String = arg(&args, "displayName")?;
            let input_cost: String = arg(&args, "inputCost")?;
            let output_cost: String = arg(&args, "outputCost")?;
            let cache_read_cost: String = arg(&args, "cacheReadCost")?;
            let cache_creation_cost: String = arg(&args, "cacheCreationCost")?;
            let conn = app
                .db
                .conn
                .lock()
                .map_err(|e| format!("Mutex lock failed: {e}"))?;
            conn.execute(
                "INSERT OR REPLACE INTO model_pricing (
                    model_id, display_name, input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    model_id,
                    display_name,
                    input_cost,
                    output_cost,
                    cache_read_cost,
                    cache_creation_cost
                ],
            )
            .map_err(|e| format!("更新模型定价失败: {e}"))?;
            ok(Value::Null)
        }
        "delete_model_pricing" => {
            let model_id: String = arg(&args, "modelId")?;
            let conn = app
                .db
                .conn
                .lock()
                .map_err(|e| format!("Mutex lock failed: {e}"))?;
            conn.execute(
                "DELETE FROM model_pricing WHERE model_id = ?1",
                rusqlite::params![model_id],
            )
            .map_err(|e| format!("删除模型定价失败: {e}"))?;
            ok(Value::Null)
        }
        "check_provider_limits" => {
            let provider_id: String = arg(&args, "providerId")?;
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .check_provider_limits(&provider_id, &app_type)
                .map_err(|e| e.to_string())?)
        }
        "sync_session_usage" => {
            let mut result = crate::services::session_usage::sync_claude_session_logs(&app.db)
                .map_err(|e| e.to_string())?;
            match crate::services::session_usage_codex::sync_codex_usage(&app.db) {
                Ok(codex_result) => {
                    result.imported += codex_result.imported;
                    result.skipped += codex_result.skipped;
                    result.files_scanned += codex_result.files_scanned;
                    result.errors.extend(codex_result.errors);
                }
                Err(e) => result.errors.push(format!("Codex 同步失败: {e}")),
            }
            match crate::services::session_usage_gemini::sync_gemini_usage(&app.db) {
                Ok(gemini_result) => {
                    result.imported += gemini_result.imported;
                    result.skipped += gemini_result.skipped;
                    result.files_scanned += gemini_result.files_scanned;
                    result.errors.extend(gemini_result.errors);
                }
                Err(e) => result.errors.push(format!("Gemini 同步失败: {e}")),
            }
            ok(result)
        }
        "get_usage_data_sources" => ok(crate::services::session_usage::get_data_source_breakdown(
            &app.db,
        )
        .map_err(|e| e.to_string())?),

        // ==================== 供应商 CRUD / 导入 ====================
        "add_provider" => {
            let app_str: String = arg(&args, "app")?;
            let provider: Provider = arg(&args, "provider")?;
            let add_to_live: Option<bool> = args.get("addToLive").and_then(|v| v.as_bool());
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(ProviderService::add(
                app.as_ref(),
                app_type,
                provider,
                add_to_live.unwrap_or(true),
            )
            .map_err(|e| e.to_string())?)
        }
        "update_provider" => {
            let app_str: String = arg(&args, "app")?;
            let provider: Provider = arg(&args, "provider")?;
            let original_id: Option<String> = args
                .get("originalId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(
                ProviderService::update(app.as_ref(), app_type, original_id.as_deref(), provider)
                    .map_err(|e| e.to_string())?,
            )
        }
        "delete_provider" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ProviderService::delete(app.as_ref(), app_type, &id).map_err(|e| e.to_string())?;
            ok(true)
        }
        "remove_provider_from_live_config" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ProviderService::remove_from_live_config(app.as_ref(), app_type, &id)
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "switch_provider" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(ProviderService::switch(app.as_ref(), app_type, &id).map_err(|e| e.to_string())?)
        }
        "update_providers_sort_order" => {
            let app_str: String = arg(&args, "app")?;
            let updates: Vec<ProviderSortUpdate> = arg(&args, "updates")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(
                ProviderService::update_sort_order(app.as_ref(), app_type, updates)
                    .map_err(|e| e.to_string())?,
            )
        }
        "import_default_config" => import_default_config(app, args).await,
        "import_opencode_providers_from_live" => ok(
            crate::services::provider::import_opencode_providers_from_live(app.as_ref())
                .map_err(|e| e.to_string())?,
        ),
        "get_opencode_live_provider_ids" => ok(crate::opencode_config::get_providers()
            .map(|providers| providers.keys().cloned().collect::<Vec<String>>())
            .map_err(|e| e.to_string())?),
        "import_openclaw_providers_from_live" => ok(
            crate::services::provider::import_openclaw_providers_from_live(app.as_ref())
                .map_err(|e| e.to_string())?,
        ),
        "get_openclaw_live_provider_ids" => ok(crate::openclaw_config::get_providers()
            .map(|providers| providers.keys().cloned().collect::<Vec<String>>())
            .map_err(|e| e.to_string())?),
        "get_universal_providers" => {
            ok(ProviderService::list_universal(app.as_ref()).map_err(|e| e.to_string())?)
        }
        "get_universal_provider" => {
            let id: String = arg(&args, "id")?;
            ok(ProviderService::get_universal(app.as_ref(), &id).map_err(|e| e.to_string())?)
        }
        "upsert_universal_provider" => {
            let provider: UniversalProvider = arg(&args, "provider")?;
            ok(ProviderService::upsert_universal(app.as_ref(), provider)
                .map_err(|e| e.to_string())?)
        }
        "delete_universal_provider" => {
            let id: String = arg(&args, "id")?;
            ok(ProviderService::delete_universal(app.as_ref(), &id).map_err(|e| e.to_string())?)
        }
        "sync_universal_provider" => {
            let id: String = arg(&args, "id")?;
            ok(ProviderService::sync_universal_to_apps(app.as_ref(), &id)
                .map_err(|e| e.to_string())?)
        }

        // ==================== 全局出站代理 / 计费 / 配置片段 ====================
        "get_global_proxy_url" => ok(app.db.get_global_proxy_url().map_err(|e| e.to_string())?),
        "set_global_proxy_url" => {
            let url: String = arg(&args, "url")?;
            let url_opt = if url.trim().is_empty() {
                None
            } else {
                Some(url.as_str())
            };
            crate::proxy::http_client::validate_proxy(url_opt)?;
            app.db
                .set_global_proxy_url(url_opt)
                .map_err(|e| e.to_string())?;
            crate::proxy::http_client::apply_proxy(url_opt)?;
            ok(Value::Null)
        }
        "test_proxy_url" => {
            let url: String = arg(&args, "url")?;
            ok(crate::commands::test_proxy_url(url).await?)
        }
        "get_upstream_proxy_status" => ok(crate::commands::get_upstream_proxy_status()),
        "scan_local_proxies" => ok(crate::commands::scan_local_proxies().await),
        "update_global_proxy_config" => update_global_proxy_config(app, args).await,
        "get_default_cost_multiplier" => {
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_default_cost_multiplier(&app_type)
                .await
                .map_err(|e| e.to_string())?)
        }
        "set_default_cost_multiplier" => {
            let app_type: String = arg(&args, "appType")?;
            let value: String = arg(&args, "value")?;
            app.db
                .set_default_cost_multiplier(&app_type, &value)
                .await
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "get_pricing_model_source" => {
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_pricing_model_source(&app_type)
                .await
                .map_err(|e| e.to_string())?)
        }
        "set_pricing_model_source" => {
            let app_type: String = arg(&args, "appType")?;
            let value: String = arg(&args, "value")?;
            app.db
                .set_pricing_model_source(&app_type, &value)
                .await
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "get_claude_common_config_snippet" => ok(app
            .db
            .get_config_snippet("claude")
            .map_err(|e| e.to_string())?),
        "set_claude_common_config_snippet" => {
            let snippet: String = arg(&args, "snippet")?;
            let is_cleared = snippet.trim().is_empty();
            if !is_cleared {
                serde_json::from_str::<Value>(&snippet)
                    .map_err(|e| format!("无效的 JSON 格式: {e}"))?;
            }
            let value = if is_cleared { None } else { Some(snippet) };
            app.db
                .set_config_snippet("claude", value)
                .map_err(|e| e.to_string())?;
            app.db
                .set_config_snippet_cleared("claude", is_cleared)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "get_common_config_snippet" => {
            let app_type: String = arg(&args, "appType")?;
            ok(app
                .db
                .get_config_snippet(&app_type)
                .map_err(|e| e.to_string())?)
        }
        "set_common_config_snippet" => set_common_config_snippet(app, args).await,
        "extract_common_config_snippet" => {
            let app_type: String = arg(&args, "appType")?;
            let app_enum = AppType::from_str(&app_type).map_err(|e| e.to_string())?;
            let settings_config: Option<String> = match args.get("settingsConfig") {
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
                _ => None,
            };
            if let Some(settings_config) = settings_config {
                let settings: Value = serde_json::from_str(&settings_config)
                    .map_err(|e| format!("无效的 JSON 格式: {e}"))?;
                ok(
                    ProviderService::extract_common_config_snippet_from_settings(
                        app_enum, &settings,
                    )
                    .map_err(|e| e.to_string())?,
                )
            } else {
                ok(
                    ProviderService::extract_common_config_snippet(app.as_ref(), app_enum)
                        .map_err(|e| e.to_string())?,
                )
            }
        }

        // ==================== 应用设置 / WebDAV 云备份 / 本地备份 / 导入导出 ====================
        "get_settings" => ok(crate::settings::get_settings_for_frontend()),
        "save_settings" => {
            let incoming: crate::settings::AppSettings = arg(&args, "settings")?;
            let existing = crate::settings::get_settings();
            let merged = merge_settings_for_save_web(incoming, &existing);
            crate::settings::update_settings(merged).map_err(|e| e.to_string())?;
            ok(true)
        }
        "webdav_test_connection" => {
            let settings: crate::settings::WebDavSyncSettings = arg(&args, "settings")?;
            let preserve_empty: bool = args
                .get("preserveEmptyPassword")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let resolved = resolve_webdav_password(
                settings,
                crate::settings::get_webdav_sync_settings(),
                preserve_empty,
            );
            crate::services::webdav_sync::check_connection(&resolved)
                .await
                .map_err(|e| e.to_string())?;
            ok(serde_json::json!({ "success": true, "message": "WebDAV connection ok" }))
        }
        "webdav_sync_save_settings" => {
            let settings: crate::settings::WebDavSyncSettings = arg(&args, "settings")?;
            let password_touched: bool = args
                .get("passwordTouched")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let existing = crate::settings::get_webdav_sync_settings();
            let mut sync_settings =
                resolve_webdav_password(settings, existing.clone(), !password_touched);
            if let Some(existing_settings) = existing {
                sync_settings.status = existing_settings.status;
            }
            sync_settings.normalize();
            sync_settings.validate().map_err(|e| e.to_string())?;
            crate::settings::set_webdav_sync_settings(Some(sync_settings))
                .map_err(|e| e.to_string())?;
            ok(serde_json::json!({ "success": true }))
        }
        "webdav_sync_fetch_remote_info" => {
            let settings = require_enabled_webdav_settings_web()?;
            let info = crate::services::webdav_sync::fetch_remote_info(&settings)
                .await
                .map_err(|e| e.to_string())?;
            ok(info.unwrap_or(serde_json::json!({ "empty": true })))
        }
        "webdav_sync_upload" => {
            let db = app.db.clone();
            let mut settings = require_enabled_webdav_settings_web()?;
            let result = crate::services::webdav_sync::run_with_sync_lock(
                crate::services::webdav_sync::upload(&db, &mut settings),
            )
            .await;
            match result {
                Ok(value) => ok(value),
                Err(err) => {
                    persist_webdav_sync_error(&mut settings, &err.to_string(), "manual");
                    Err(err.to_string())
                }
            }
        }
        "webdav_sync_download" => {
            let db = app.db.clone();
            let mut settings = require_enabled_webdav_settings_web()?;
            let _auto_sync_suppression =
                crate::services::webdav_auto_sync::AutoSyncSuppressionGuard::new();
            let sync_result = crate::services::webdav_sync::run_with_sync_lock(
                crate::services::webdav_sync::download(&db, &mut settings),
            )
            .await;
            let mut result = match sync_result {
                Ok(value) => value,
                Err(err) => {
                    persist_webdav_sync_error(&mut settings, &err.to_string(), "manual");
                    return Err(err.to_string());
                }
            };
            let post_warning = {
                let post_state = crate::store::AppState::new(db.clone());
                match ProviderService::sync_current_to_live(&post_state)
                    .and_then(|_| crate::settings::reload_settings())
                {
                    Ok(()) => None,
                    Err(e) => {
                        log::warn!("[WebPanel] 下载后同步状态失败: {e}");
                        Some(format!("Post-operation synchronization failed: {e}"))
                    }
                }
            };
            if let (Some(msg), Some(obj)) = (post_warning, result.as_object_mut()) {
                obj.insert("warning".to_string(), Value::String(msg));
            }
            ok(result)
        }
        "sync_current_providers_live" => {
            let post_state = crate::store::AppState::new(app.db.clone());
            ProviderService::sync_current_to_live(&post_state).map_err(|e| e.to_string())?;
            ok(serde_json::json!({ "success": true, "message": "Live configuration synchronized" }))
        }
        "create_db_backup" => {
            let path = app
                .db
                .backup_database_file()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Database file not found, backup skipped".to_string())?;
            ok(path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default())
        }
        "list_db_backups" => {
            ok(crate::database::Database::list_backups().map_err(|e| e.to_string())?)
        }
        "restore_db_backup" => {
            let filename: String = arg(&args, "filename")?;
            ok(app
                .db
                .restore_from_backup(&filename)
                .map_err(|e| e.to_string())?)
        }
        "rename_db_backup" => {
            let old_filename: String = arg(&args, "oldFilename")?;
            let new_name: String = arg(&args, "newName")?;
            ok(
                crate::database::Database::rename_backup(&old_filename, &new_name)
                    .map_err(|e| e.to_string())?,
            )
        }
        "delete_db_backup" => {
            let filename: String = arg(&args, "filename")?;
            crate::database::Database::delete_backup(&filename).map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "export_config_to_file" => {
            let file_path: String = arg(&args, "filePath")?;
            let target_path = std::path::PathBuf::from(&file_path);
            app.db.export_sql(&target_path).map_err(|e| e.to_string())?;
            ok(serde_json::json!({
                "success": true,
                "message": "SQL exported successfully",
                "filePath": file_path
            }))
        }
        "import_config_from_file" => {
            let file_path: String = arg(&args, "filePath")?;
            let path_buf = std::path::PathBuf::from(&file_path);
            let backup_id = app.db.import_sql(&path_buf).map_err(|e| e.to_string())?;
            let post_warning = {
                let post_state = crate::store::AppState::new(app.db.clone());
                match ProviderService::sync_current_to_live(&post_state)
                    .and_then(|_| crate::settings::reload_settings())
                {
                    Ok(()) => None,
                    Err(e) => {
                        log::warn!("[WebPanel] 导入后同步状态失败: {e}");
                        Some(format!("Post-operation synchronization failed: {e}"))
                    }
                }
            };
            let mut payload = serde_json::json!({
                "success": true,
                "message": "SQL imported successfully",
                "backupId": backup_id
            });
            if let (Some(msg), Some(obj)) = (post_warning, payload.as_object_mut()) {
                obj.insert("warning".to_string(), Value::String(msg));
            }
            ok(payload)
        }

        // ==================== 浏览器文件 I/O（内容下载/上传，无服务端路径）====================
        // Web 控制台专用：导出返回 SQL 文本由浏览器另存为，导入接收上传的 SQL 文本。
        "export_config_string" => {
            let content = app.db.export_sql_string().map_err(|e| e.to_string())?;
            ok(serde_json::json!({ "content": content }))
        }
        "import_config_string" => {
            let content: String = arg(&args, "content")?;
            let backup_id = app
                .db
                .import_sql_string(&content)
                .map_err(|e| e.to_string())?;
            let post_warning = {
                let post_state = crate::store::AppState::new(app.db.clone());
                match ProviderService::sync_current_to_live(&post_state)
                    .and_then(|_| crate::settings::reload_settings())
                {
                    Ok(()) => None,
                    Err(e) => {
                        log::warn!("[WebPanel] 导入后同步状态失败: {e}");
                        Some(format!("Post-operation synchronization failed: {e}"))
                    }
                }
            };
            let mut payload = serde_json::json!({
                "success": true,
                "message": "SQL imported successfully",
                "backupId": backup_id
            });
            if let (Some(msg), Some(obj)) = (post_warning, payload.as_object_mut()) {
                obj.insert("warning".to_string(), Value::String(msg));
            }
            ok(payload)
        }

        // ==================== 启动期 / 桌面专属（Web 降级）====================
        "get_init_error" => ok(Value::Null), // 浏览器无后端初始化错误事件
        "update_tray_menu" => ok(true),      // 无托盘：no-op，避免变更后置回调报错

        // ==================== auth ====================

        // ==================== copilot ====================
        "copilot_start_device_flow" => {
            let github_domain: Option<String> = arg(&args, "githubDomain").unwrap_or(None);
            ok(copilot_manager()
                .start_device_flow(github_domain.as_deref())
                .await
                .map_err(|e| e.to_string())?)
        }
        "copilot_poll_for_auth" => {
            let device_code: String = arg(&args, "deviceCode")?;
            let github_domain: Option<String> = arg(&args, "githubDomain").unwrap_or(None);
            match copilot_manager()
                .poll_for_token(&device_code, github_domain.as_deref())
                .await
            {
                Ok(Some(_account)) => ok(true),
                Ok(None) => ok(false),
                Err(CopilotAuthError::AuthorizationPending) => ok(false),
                Err(e) => Err(e.to_string()),
            }
        }
        "copilot_poll_for_account" => {
            let device_code: String = arg(&args, "deviceCode")?;
            let github_domain: Option<String> = arg(&args, "githubDomain").unwrap_or(None);
            match copilot_manager()
                .poll_for_token(&device_code, github_domain.as_deref())
                .await
            {
                Ok(account) => ok(account),
                Err(CopilotAuthError::AuthorizationPending) => ok(Value::Null),
                Err(e) => Err(e.to_string()),
            }
        }
        "copilot_list_accounts" => ok(copilot_manager().list_accounts().await),
        "copilot_remove_account" => {
            let account_id: String = arg(&args, "accountId")?;
            copilot_manager()
                .remove_account(&account_id)
                .await
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "copilot_set_default_account" => {
            let account_id: String = arg(&args, "accountId")?;
            copilot_manager()
                .set_default_account(&account_id)
                .await
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "copilot_get_auth_status" => ok(copilot_manager().get_status().await),
        "copilot_is_authenticated" => ok(copilot_manager().is_authenticated().await),
        "copilot_logout" => {
            copilot_manager()
                .clear_auth()
                .await
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "copilot_get_token" => ok(copilot_manager()
            .get_valid_token()
            .await
            .map_err(|e| e.to_string())?),
        "copilot_get_token_for_account" => {
            let account_id: String = arg(&args, "accountId")?;
            ok(copilot_manager()
                .get_valid_token_for_account(&account_id)
                .await
                .map_err(|e| e.to_string())?)
        }
        "copilot_get_models" => ok(copilot_manager()
            .fetch_models()
            .await
            .map_err(|e| e.to_string())?),
        "copilot_get_models_for_account" => {
            let account_id: String = arg(&args, "accountId")?;
            ok(copilot_manager()
                .fetch_models_for_account(&account_id)
                .await
                .map_err(|e| e.to_string())?)
        }
        "copilot_get_usage" => ok(copilot_manager()
            .fetch_usage()
            .await
            .map_err(|e| e.to_string())?),
        "copilot_get_usage_for_account" => {
            let account_id: String = arg(&args, "accountId")?;
            ok(copilot_manager()
                .fetch_usage_for_account(&account_id)
                .await
                .map_err(|e| e.to_string())?)
        }

        // ==================== skill ====================
        "get_installed_skills" => {
            ok(SkillService::get_all_installed(&app.db).map_err(|e| e.to_string())?)
        }
        "get_skill_backups" => ok(SkillService::list_backups().map_err(|e| e.to_string())?),
        "delete_skill_backup" => {
            let backup_id: String = arg(&args, "backupId")?;
            SkillService::delete_backup(&backup_id).map_err(|e| e.to_string())?;
            ok(true)
        }
        "install_skill_unified" => {
            let skill: DiscoverableSkill = arg(&args, "skill")?;
            let current_app: String = arg(&args, "currentApp")?;
            let app_type = AppType::from_str(&current_app).map_err(|e| e.to_string())?;
            ok(SkillService::new()
                .install(&app.db, &skill, &app_type)
                .await
                .map_err(|e| e.to_string())?)
        }
        "uninstall_skill_unified" => {
            let id: String = arg(&args, "id")?;
            ok(SkillService::uninstall(&app.db, &id).map_err(|e| e.to_string())?)
        }
        "restore_skill_backup" => {
            let backup_id: String = arg(&args, "backupId")?;
            let current_app: String = arg(&args, "currentApp")?;
            let app_type = AppType::from_str(&current_app).map_err(|e| e.to_string())?;
            ok(
                SkillService::restore_from_backup(&app.db, &backup_id, &app_type)
                    .map_err(|e| e.to_string())?,
            )
        }
        "toggle_skill_app" => {
            let id: String = arg(&args, "id")?;
            let app_str: String = arg(&args, "app")?;
            let enabled: bool = arg(&args, "enabled")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            SkillService::toggle_app(&app.db, &id, &app_type, enabled)
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "scan_unmanaged_skills" => {
            ok(SkillService::scan_unmanaged(&app.db).map_err(|e| e.to_string())?)
        }
        "import_skills_from_apps" => {
            let imports: Vec<ImportSkillSelection> = arg(&args, "imports")?;
            ok(SkillService::import_from_apps(&app.db, imports).map_err(|e| e.to_string())?)
        }
        "discover_available_skills" => {
            let repos = app.db.get_skill_repos().map_err(|e| e.to_string())?;
            ok(SkillService::new()
                .discover_available(repos)
                .await
                .map_err(|e| e.to_string())?)
        }
        "check_skill_updates" => ok(SkillService::new()
            .check_updates(&app.db)
            .await
            .map_err(|e| e.to_string())?),
        "update_skill" => {
            let id: String = arg(&args, "id")?;
            ok(SkillService::new()
                .update_skill(&app.db, &id)
                .await
                .map_err(|e| e.to_string())?)
        }
        "migrate_skill_storage" => {
            let target: SkillStorageLocation = arg(&args, "target")?;
            ok(SkillService::migrate_storage(&app.db, target).map_err(|e| e.to_string())?)
        }
        "search_skills_sh" => {
            let query: String = arg(&args, "query")?;
            let limit: usize = arg(&args, "limit")?;
            let offset: usize = arg(&args, "offset")?;
            ok(SkillService::search_skills_sh(&query, limit, offset)
                .await
                .map_err(|e| e.to_string())?)
        }
        "get_skills" => {
            let repos = app.db.get_skill_repos().map_err(|e| e.to_string())?;
            ok(SkillService::new()
                .list_skills(repos, &app.db)
                .await
                .map_err(|e| e.to_string())?)
        }
        "get_skills_for_app" => {
            // 新版本不再区分应用：验证 app 参数后统一返回所有技能
            let app_str: String = arg(&args, "app")?;
            AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            let repos = app.db.get_skill_repos().map_err(|e| e.to_string())?;
            ok(SkillService::new()
                .list_skills(repos, &app.db)
                .await
                .map_err(|e| e.to_string())?)
        }
        "install_skill" => {
            // 兼容旧 API：固定 claude，通过 directory 在发现列表中匹配后安装
            let directory: String = arg(&args, "directory")?;
            let app_type = AppType::from_str("claude").map_err(|e| e.to_string())?;
            let repos = app.db.get_skill_repos().map_err(|e| e.to_string())?;
            let svc = SkillService::new();
            let skills = svc
                .discover_available(repos)
                .await
                .map_err(|e| e.to_string())?;
            let skill = skills
                .into_iter()
                .find(|s| {
                    let install_name = std::path::Path::new(&s.directory)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| s.directory.clone());
                    install_name.eq_ignore_ascii_case(&directory)
                        || s.directory.eq_ignore_ascii_case(&directory)
                })
                .ok_or_else(|| format!("未找到可安装的 Skill: {directory}"))?;
            svc.install(&app.db, &skill, &app_type)
                .await
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "install_skill_for_app" => {
            // 兼容旧 API：通过 directory 在发现列表中匹配后安装到指定应用
            let app_str: String = arg(&args, "app")?;
            let directory: String = arg(&args, "directory")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            let repos = app.db.get_skill_repos().map_err(|e| e.to_string())?;
            let svc = SkillService::new();
            let skills = svc
                .discover_available(repos)
                .await
                .map_err(|e| e.to_string())?;
            let skill = skills
                .into_iter()
                .find(|s| {
                    let install_name = std::path::Path::new(&s.directory)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| s.directory.clone());
                    install_name.eq_ignore_ascii_case(&directory)
                        || s.directory.eq_ignore_ascii_case(&directory)
                })
                .ok_or_else(|| format!("未找到可安装的 Skill: {directory}"))?;
            svc.install(&app.db, &skill, &app_type)
                .await
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "uninstall_skill" => {
            // 兼容旧 API：固定 claude，通过 directory 找到已安装 skill id 后卸载
            let directory: String = arg(&args, "directory")?;
            let skills = SkillService::get_all_installed(&app.db).map_err(|e| e.to_string())?;
            let skill = skills
                .into_iter()
                .find(|s| s.directory.eq_ignore_ascii_case(&directory))
                .ok_or_else(|| format!("未找到已安装的 Skill: {directory}"))?;
            ok(SkillService::uninstall(&app.db, &skill.id).map_err(|e| e.to_string())?)
        }
        "uninstall_skill_for_app" => {
            // 兼容旧 API：验证 app 参数后，通过 directory 找到已安装 skill id 后卸载
            let app_str: String = arg(&args, "app")?;
            let directory: String = arg(&args, "directory")?;
            AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            let skills = SkillService::get_all_installed(&app.db).map_err(|e| e.to_string())?;
            let skill = skills
                .into_iter()
                .find(|s| s.directory.eq_ignore_ascii_case(&directory))
                .ok_or_else(|| format!("未找到已安装的 Skill: {directory}"))?;
            ok(SkillService::uninstall(&app.db, &skill.id).map_err(|e| e.to_string())?)
        }
        "get_skill_repos" => ok(app.db.get_skill_repos().map_err(|e| e.to_string())?),
        "add_skill_repo" => {
            let repo: SkillRepo = arg(&args, "repo")?;
            app.db.save_skill_repo(&repo).map_err(|e| e.to_string())?;
            ok(true)
        }
        "remove_skill_repo" => {
            let owner: String = arg(&args, "owner")?;
            let name: String = arg(&args, "name")?;
            app.db
                .delete_skill_repo(&owner, &name)
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "install_skills_from_zip" => {
            let file_path: String = arg(&args, "filePath")?;
            let current_app: String = arg(&args, "currentApp")?;
            let app_type = AppType::from_str(&current_app).map_err(|e| e.to_string())?;
            let path = std::path::Path::new(&file_path);
            ok(SkillService::install_from_zip(&app.db, path, &app_type)
                .map_err(|e| e.to_string())?)
        }

        // ==================== mcp ====================
        "get_claude_mcp_status" => {
            ok(crate::claude_mcp::get_mcp_status().map_err(|e| e.to_string())?)
        }
        "read_claude_mcp_config" => {
            ok(crate::claude_mcp::read_mcp_json().map_err(|e| e.to_string())?)
        }
        "upsert_claude_mcp_server" => {
            let id: String = arg(&args, "id")?;
            let spec: Value = arg(&args, "spec")?;
            ok(crate::claude_mcp::upsert_mcp_server(&id, spec).map_err(|e| e.to_string())?)
        }
        "delete_claude_mcp_server" => {
            let id: String = arg(&args, "id")?;
            ok(crate::claude_mcp::delete_mcp_server(&id).map_err(|e| e.to_string())?)
        }
        "validate_mcp_command" => {
            let cmd: String = arg(&args, "cmd")?;
            ok(crate::claude_mcp::validate_command_in_path(&cmd).map_err(|e| e.to_string())?)
        }
        "get_mcp_config" => {
            #[allow(deprecated)]
            {
                let app_str: String = arg(&args, "app")?;
                let app_ty = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
                let config_path = crate::config::get_app_config_path()
                    .to_string_lossy()
                    .to_string();
                let servers = McpService::get_servers(app, app_ty).map_err(|e| e.to_string())?;
                ok(serde_json::json!({
                    "config_path": config_path,
                    "servers": servers,
                }))
            }
        }
        "upsert_mcp_server_in_config" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let spec: Value = arg(&args, "spec")?;
            let sync_other_side: Option<bool> = args.get("syncOtherSide").and_then(|v| v.as_bool());
            let app_ty = AppType::from_str(&app_str).map_err(|e| e.to_string())?;

            let existing_server = {
                let servers = app.db.get_all_mcp_servers().map_err(|e| e.to_string())?;
                servers.get(&id).cloned()
            };

            let mut new_server = if let Some(mut existing) = existing_server {
                existing.server = spec.clone();
                existing.apps.set_enabled_for(&app_ty, true);
                existing
            } else {
                let mut apps = McpApps::default();
                apps.set_enabled_for(&app_ty, true);
                let name = spec
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&id)
                    .to_string();
                McpServer {
                    id: id.clone(),
                    name,
                    server: spec,
                    apps,
                    description: None,
                    homepage: None,
                    docs: None,
                    tags: Vec::new(),
                }
            };

            if sync_other_side.unwrap_or(false) {
                new_server.apps.claude = true;
                new_server.apps.codex = true;
                new_server.apps.gemini = true;
                new_server.apps.opencode = true;
            }

            McpService::upsert_server(app, new_server).map_err(|e| e.to_string())?;
            ok(true)
        }
        "delete_mcp_server_in_config" => {
            let id: String = arg(&args, "id")?;
            ok(McpService::delete_server(app, &id).map_err(|e| e.to_string())?)
        }
        "set_mcp_enabled" => {
            #[allow(deprecated)]
            {
                let app_str: String = arg(&args, "app")?;
                let id: String = arg(&args, "id")?;
                let enabled: bool = arg(&args, "enabled")?;
                let app_ty = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
                ok(
                    McpService::set_enabled(app, app_ty, &id, enabled)
                        .map_err(|e| e.to_string())?,
                )
            }
        }
        "get_mcp_servers" => ok(McpService::get_all_servers(app).map_err(|e| e.to_string())?),
        "upsert_mcp_server" => {
            let server: McpServer = arg(&args, "server")?;
            McpService::upsert_server(app, server).map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "delete_mcp_server" => {
            let id: String = arg(&args, "id")?;
            ok(McpService::delete_server(app, &id).map_err(|e| e.to_string())?)
        }
        "toggle_mcp_app" => {
            let server_id: String = arg(&args, "serverId")?;
            let app_str: String = arg(&args, "app")?;
            let enabled: bool = arg(&args, "enabled")?;
            let app_ty = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            McpService::toggle_app(app, &server_id, app_ty, enabled).map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "import_mcp_from_apps" => {
            let mut total = 0usize;
            total += McpService::import_from_claude(app).unwrap_or(0);
            total += McpService::import_from_codex(app).unwrap_or(0);
            total += McpService::import_from_gemini(app).unwrap_or(0);
            total += McpService::import_from_opencode(app).unwrap_or(0);
            total += McpService::import_from_hermes(app).unwrap_or(0);
            ok(total)
        }

        // ==================== hermes ====================
        "get_hermes_live_provider_ids" => ok(crate::hermes_config::get_providers()
            .map(|providers| providers.keys().cloned().collect::<Vec<String>>())
            .map_err(|e| e.to_string())?),
        "get_hermes_model_config" => {
            ok(crate::hermes_config::get_model_config().map_err(|e| e.to_string())?)
        }
        "get_hermes_memory" => {
            let kind: crate::hermes_config::MemoryKind = arg(&args, "kind")?;
            ok(crate::hermes_config::read_memory(kind).map_err(|e| e.to_string())?)
        }
        "set_hermes_memory" => {
            let kind: crate::hermes_config::MemoryKind = arg(&args, "kind")?;
            let content: String = arg(&args, "content")?;
            crate::hermes_config::write_memory(kind, &content).map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "get_hermes_memory_limits" => {
            ok(crate::hermes_config::read_memory_limits().map_err(|e| e.to_string())?)
        }
        "set_hermes_memory_enabled" => {
            let kind: crate::hermes_config::MemoryKind = arg(&args, "kind")?;
            let enabled: bool = arg(&args, "enabled")?;
            ok(crate::hermes_config::set_memory_enabled(kind, enabled)
                .map_err(|e| e.to_string())?)
        }
        "import_hermes_providers_from_live" => ok(
            crate::services::provider::import_hermes_providers_from_live(app.as_ref())
                .map_err(|e| e.to_string())?,
        ),

        // ==================== openclaw ====================
        "get_openclaw_agents_defaults" => {
            ok(crate::openclaw_config::get_agents_defaults().map_err(|e| e.to_string())?)
        }
        "get_openclaw_default_model" => {
            ok(crate::openclaw_config::get_default_model().map_err(|e| e.to_string())?)
        }
        "get_openclaw_env" => {
            ok(crate::openclaw_config::get_env_config().map_err(|e| e.to_string())?)
        }
        "get_openclaw_live_provider" => {
            let provider_id: String = arg(&args, "providerId")?;
            ok(crate::openclaw_config::get_provider(&provider_id).map_err(|e| e.to_string())?)
        }
        "get_openclaw_model_catalog" => {
            ok(crate::openclaw_config::get_model_catalog().map_err(|e| e.to_string())?)
        }
        "get_openclaw_tools" => {
            ok(crate::openclaw_config::get_tools_config().map_err(|e| e.to_string())?)
        }
        "scan_openclaw_config_health" => {
            ok(crate::openclaw_config::scan_openclaw_config_health().map_err(|e| e.to_string())?)
        }
        "set_openclaw_agents_defaults" => {
            let defaults: crate::openclaw_config::OpenClawAgentsDefaults = arg(&args, "defaults")?;
            ok(
                crate::openclaw_config::set_agents_defaults(&defaults)
                    .map_err(|e| e.to_string())?,
            )
        }
        "set_openclaw_default_model" => {
            let model: crate::openclaw_config::OpenClawDefaultModel = arg(&args, "model")?;
            ok(crate::openclaw_config::set_default_model(&model).map_err(|e| e.to_string())?)
        }
        "set_openclaw_env" => {
            let env: crate::openclaw_config::OpenClawEnvConfig = arg(&args, "env")?;
            ok(crate::openclaw_config::set_env_config(&env).map_err(|e| e.to_string())?)
        }
        "set_openclaw_model_catalog" => {
            let catalog: std::collections::HashMap<
                String,
                crate::openclaw_config::OpenClawModelCatalogEntry,
            > = arg(&args, "catalog")?;
            ok(crate::openclaw_config::set_model_catalog(&catalog).map_err(|e| e.to_string())?)
        }
        "set_openclaw_tools" => {
            let tools: crate::openclaw_config::OpenClawToolsConfig = arg(&args, "tools")?;
            ok(crate::openclaw_config::set_tools_config(&tools).map_err(|e| e.to_string())?)
        }

        // ==================== omo ====================
        "read_omo_local_file" => ok(crate::services::OmoService::read_local_file(
            &crate::services::omo::STANDARD,
        )
        .map_err(|e| e.to_string())?),
        "read_omo_slim_local_file" => ok(crate::services::OmoService::read_local_file(
            &crate::services::omo::SLIM,
        )
        .map_err(|e| e.to_string())?),
        "get_current_omo_provider_id" => {
            let provider = app
                .db
                .get_current_omo_provider("opencode", "omo")
                .map_err(|e| e.to_string())?;
            ok(provider.map(|p| p.id).unwrap_or_default())
        }
        "get_current_omo_slim_provider_id" => {
            let provider = app
                .db
                .get_current_omo_provider("opencode", "omo-slim")
                .map_err(|e| e.to_string())?;
            ok(provider.map(|p| p.id).unwrap_or_default())
        }
        "disable_current_omo" => {
            let providers = app
                .db
                .get_all_providers("opencode")
                .map_err(|e| e.to_string())?;
            for (id, p) in &providers {
                if p.category.as_deref() == Some("omo") {
                    app.db
                        .clear_omo_provider_current("opencode", id, "omo")
                        .map_err(|e| e.to_string())?;
                }
            }
            crate::services::OmoService::delete_config_file(&crate::services::omo::STANDARD)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "disable_current_omo_slim" => {
            let providers = app
                .db
                .get_all_providers("opencode")
                .map_err(|e| e.to_string())?;
            for (id, p) in &providers {
                if p.category.as_deref() == Some("omo-slim") {
                    app.db
                        .clear_omo_provider_current("opencode", id, "omo-slim")
                        .map_err(|e| e.to_string())?;
                }
            }
            crate::services::OmoService::delete_config_file(&crate::services::omo::SLIM)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }

        // ==================== session ====================
        "list_sessions" => ok(session_manager::scan_sessions()),
        "get_session_messages" => {
            let provider_id: String = arg(&args, "providerId")?;
            let source_path: String = arg(&args, "sourcePath")?;
            ok(session_manager::load_messages(&provider_id, &source_path)?)
        }
        "delete_session" => {
            let provider_id: String = arg(&args, "providerId")?;
            let session_id: String = arg(&args, "sessionId")?;
            let source_path: String = arg(&args, "sourcePath")?;
            ok(session_manager::delete_session(
                &provider_id,
                &session_id,
                &source_path,
            )?)
        }
        "delete_sessions" => {
            let items: Vec<session_manager::DeleteSessionRequest> = arg(&args, "items")?;
            ok(session_manager::delete_sessions(&items))
        }

        // ==================== prompt ====================
        "get_prompts" => {
            let app_str: String = arg(&args, "app")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(PromptService::get_prompts(app.as_ref(), app_type).map_err(|e| e.to_string())?)
        }
        "upsert_prompt" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let prompt: Prompt = arg(&args, "prompt")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            PromptService::upsert_prompt(app.as_ref(), app_type, &id, prompt)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "delete_prompt" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            PromptService::delete_prompt(app.as_ref(), app_type, &id).map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "enable_prompt" => {
            let app_str: String = arg(&args, "app")?;
            let id: String = arg(&args, "id")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            PromptService::enable_prompt(app.as_ref(), app_type, &id).map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "import_prompt_from_file" => {
            let app_str: String = arg(&args, "app")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(PromptService::import_from_file(app.as_ref(), app_type)
                .map_err(|e| e.to_string())?)
        }
        "get_current_prompt_file_content" => {
            let app_str: String = arg(&args, "app")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(PromptService::get_current_file_content(app_type).map_err(|e| e.to_string())?)
        }

        // ==================== workspace_memory ====================
        "list_daily_memory_files" => ok(crate::commands::list_daily_memory_files().await?),
        "read_daily_memory_file" => {
            let filename: String = arg(&args, "filename")?;
            ok(crate::commands::read_daily_memory_file(filename).await?)
        }
        "write_daily_memory_file" => {
            let filename: String = arg(&args, "filename")?;
            let content: String = arg(&args, "content")?;
            crate::commands::write_daily_memory_file(filename, content).await?;
            ok(Value::Null)
        }
        "delete_daily_memory_file" => {
            let filename: String = arg(&args, "filename")?;
            crate::commands::delete_daily_memory_file(filename).await?;
            ok(Value::Null)
        }
        "search_daily_memory_files" => {
            let query: String = arg(&args, "query")?;
            ok(crate::commands::search_daily_memory_files(query).await?)
        }
        "read_workspace_file" => {
            let filename: String = arg(&args, "filename")?;
            ok(crate::commands::read_workspace_file(filename).await?)
        }
        "write_workspace_file" => {
            let filename: String = arg(&args, "filename")?;
            let content: String = arg(&args, "content")?;
            crate::commands::write_workspace_file(filename, content).await?;
            ok(Value::Null)
        }

        // ==================== endpoints_deeplink ====================
        "get_custom_endpoints" => {
            let app_str: String = arg(&args, "app")?;
            let provider_id: String = arg(&args, "providerId")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(
                ProviderService::get_custom_endpoints(app.as_ref(), app_type, &provider_id)
                    .map_err(|e| e.to_string())?,
            )
        }
        "add_custom_endpoint" => {
            let app_str: String = arg(&args, "app")?;
            let provider_id: String = arg(&args, "providerId")?;
            let url: String = arg(&args, "url")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ProviderService::add_custom_endpoint(app.as_ref(), app_type, &provider_id, url)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "remove_custom_endpoint" => {
            let app_str: String = arg(&args, "app")?;
            let provider_id: String = arg(&args, "providerId")?;
            let url: String = arg(&args, "url")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ProviderService::remove_custom_endpoint(app.as_ref(), app_type, &provider_id, url)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "update_endpoint_last_used" => {
            let app_str: String = arg(&args, "app")?;
            let provider_id: String = arg(&args, "providerId")?;
            let url: String = arg(&args, "url")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ProviderService::update_endpoint_last_used(app.as_ref(), app_type, &provider_id, url)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "parse_deeplink" => {
            let url: String = arg(&args, "url")?;
            ok(parse_deeplink_url(&url).map_err(|e| e.to_string())?)
        }
        "merge_deeplink_config" => {
            let request: DeepLinkImportRequest = arg(&args, "request")?;
            ok(crate::deeplink::parse_and_merge_config(&request).map_err(|e| e.to_string())?)
        }
        "import_from_deeplink_unified" => {
            let request: DeepLinkImportRequest = arg(&args, "request")?;
            match request.resource.as_str() {
                "provider" => {
                    let provider_id = import_provider_from_deeplink(app.as_ref(), request)
                        .map_err(|e| e.to_string())?;
                    ok(serde_json::json!({ "type": "provider", "id": provider_id }))
                }
                "prompt" => {
                    let prompt_id = import_prompt_from_deeplink(app.as_ref(), request)
                        .map_err(|e| e.to_string())?;
                    ok(serde_json::json!({ "type": "prompt", "id": prompt_id }))
                }
                "mcp" => {
                    let result = import_mcp_from_deeplink(app.as_ref(), request)
                        .map_err(|e| e.to_string())?;
                    ok(serde_json::json!({
                        "type": "mcp",
                        "importedCount": result.imported_count,
                        "importedIds": result.imported_ids,
                        "failed": result.failed
                    }))
                }
                "skill" => {
                    let skill_key = import_skill_from_deeplink(app.as_ref(), request)
                        .map_err(|e| e.to_string())?;
                    ok(serde_json::json!({ "type": "skill", "key": skill_key }))
                }
                other => Err(format!("Unsupported resource type: {other}")),
            }
        }

        // ==================== proxy_config_misc ====================
        "get_circuit_breaker_config" => ok(app
            .db
            .get_circuit_breaker_config()
            .await
            .map_err(|e| e.to_string())?),
        "update_circuit_breaker_config" => {
            let config: CircuitBreakerConfig = arg(&args, "config")?;
            app.db
                .update_circuit_breaker_config(&config)
                .await
                .map_err(|e| e.to_string())?;
            app.proxy_service
                .update_circuit_breaker_configs(config)
                .await?;
            ok(Value::Null)
        }
        "get_rectifier_config" => ok(app.db.get_rectifier_config().map_err(|e| e.to_string())?),
        "set_rectifier_config" => {
            let config: RectifierConfig = arg(&args, "config")?;
            app.db
                .set_rectifier_config(&config)
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "get_optimizer_config" => ok(app.db.get_optimizer_config().map_err(|e| e.to_string())?),
        "set_optimizer_config" => {
            let config: OptimizerConfig = arg(&args, "config")?;
            match config.cache_ttl.as_str() {
                "5m" | "1h" => {}
                other => {
                    return Err(format!(
                        "Invalid cache_ttl value: '{other}'. Allowed values: '5m', '1h'"
                    ))
                }
            }
            app.db
                .set_optimizer_config(&config)
                .map_err(|e| e.to_string())?;
            ok(true)
        }
        "get_log_config" => ok(app.db.get_log_config().map_err(|e| e.to_string())?),
        "set_log_config" => {
            let config: LogConfig = arg(&args, "config")?;
            app.db.set_log_config(&config).map_err(|e| e.to_string())?;
            log::set_max_level(config.to_level_filter());
            log::info!(
                "日志配置已更新: enabled={}, level={}",
                config.enabled,
                config.level
            );
            ok(true)
        }
        "get_stream_check_config" => ok(app
            .db
            .get_stream_check_config()
            .map_err(|e| e.to_string())?),
        "save_stream_check_config" => {
            let config: StreamCheckConfig = arg(&args, "config")?;
            app.db
                .save_stream_check_config(&config)
                .map_err(|e| e.to_string())?;
            ok(Value::Null)
        }
        "stream_check_provider" | "stream_check_all_providers" => Err(
            "流式健康检查依赖桌面端 Copilot 认证状态，Web 控制台暂不支持（请使用桌面端）"
                .to_string(),
        ),

        // ==================== env ====================
        "check_env_conflicts" => {
            let app_str: String = arg(&args, "app")?;
            ok(crate::services::env_checker::check_env_conflicts(&app_str)?)
        }
        "delete_env_vars" => {
            let conflicts: Vec<EnvConflict> = arg(&args, "conflicts")?;
            ok(crate::services::env_manager::delete_env_vars(conflicts)?)
        }
        "restore_env_backup" => {
            let backup_path: String = arg(&args, "backupPath")?;
            crate::services::env_manager::restore_from_backup(backup_path)?;
            ok(Value::Null)
        }

        // ==================== claude_desktop_plugin ====================
        "apply_claude_plugin_config" => {
            let official: bool = arg(&args, "official")?;
            let applied = if official {
                crate::claude_plugin::clear_claude_config().map_err(|e| e.to_string())?
            } else {
                crate::claude_plugin::write_claude_config().map_err(|e| e.to_string())?
            };
            ok(applied)
        }
        "apply_claude_onboarding_skip" => {
            ok(crate::claude_mcp::set_has_completed_onboarding().map_err(|e| e.to_string())?)
        }
        "clear_claude_onboarding_skip" => {
            ok(crate::claude_mcp::clear_has_completed_onboarding().map_err(|e| e.to_string())?)
        }
        "get_claude_code_config_path" => ok(crate::config::get_claude_settings_path()
            .to_string_lossy()
            .to_string()),
        "get_claude_desktop_status" => {
            let proxy_running = app.proxy_service.is_running().await;
            ok(
                crate::claude_desktop_config::get_status(app.db.as_ref(), proxy_running)
                    .map_err(|e| e.to_string())?,
            )
        }
        "get_claude_desktop_default_routes" => {
            ok(crate::claude_desktop_config::default_proxy_routes())
        }
        "ensure_claude_desktop_official_provider" => ok(app
            .db
            .ensure_official_seed_by_id(
                crate::database::CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
                AppType::ClaudeDesktop,
            )
            .map_err(|e| e.to_string())?),
        "import_claude_desktop_providers_from_claude" => {
            // 复用 commands::provider::import_claude_desktop_providers_from_claude 核心逻辑（纯 db）。
            // 私有助手 claude_provider_models_are_claude_safe 不可跨模块引用，故用公开的
            // is_claude_safe_model_id 内联等价复刻；suggested_claude_desktop_routes 为 pub(crate) 可直接调用。
            let claude_providers = app
                .db
                .get_all_providers(AppType::Claude.as_str())
                .map_err(|e| e.to_string())?;
            let existing_ids = app
                .db
                .get_provider_ids(AppType::ClaudeDesktop.as_str())
                .map_err(|e| e.to_string())?;

            let mut imported = 0usize;
            for provider in claude_providers.values() {
                if existing_ids.contains(&provider.id) {
                    continue;
                }

                let mut desktop_provider = provider.clone();
                desktop_provider.in_failover_queue = false;
                let meta = desktop_provider.meta.get_or_insert_with(Default::default);

                let models_claude_safe = match provider
                    .settings_config
                    .get("env")
                    .and_then(|value| value.as_object())
                {
                    None => true,
                    Some(env) => [
                        "ANTHROPIC_MODEL",
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                        "ANTHROPIC_DEFAULT_SONNET_MODEL",
                        "ANTHROPIC_DEFAULT_OPUS_MODEL",
                    ]
                    .into_iter()
                    .filter_map(|key| env.get(key).and_then(|value| value.as_str()))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .all(crate::claude_desktop_config::is_claude_safe_model_id),
                };

                if crate::claude_desktop_config::is_compatible_direct_provider(provider)
                    && models_claude_safe
                {
                    meta.claude_desktop_mode = Some(ClaudeDesktopMode::Direct);
                } else if let Some(routes) =
                    crate::commands::suggested_claude_desktop_routes(provider)
                {
                    meta.claude_desktop_mode = Some(ClaudeDesktopMode::Proxy);
                    meta.claude_desktop_model_routes = routes;
                } else {
                    continue;
                }

                app.db
                    .save_provider(AppType::ClaudeDesktop.as_str(), &desktop_provider)
                    .map_err(|e| e.to_string())?;
                imported += 1;
            }

            if let Err(e) = app.db.ensure_official_seed_by_id(
                crate::database::CLAUDE_DESKTOP_OFFICIAL_PROVIDER_ID,
                AppType::ClaudeDesktop,
            ) {
                log::warn!("Failed to ensure claude-desktop-official seed during import: {e}");
            }

            ok(imported)
        }

        // ==================== quota_balance_models ====================
        "get_balance" => {
            let base_url: String = arg(&args, "baseUrl")?;
            let api_key: String = arg(&args, "apiKey")?;
            ok(crate::services::balance::get_balance(&base_url, &api_key).await?)
        }
        "get_coding_plan_quota" => {
            let base_url: String = arg(&args, "baseUrl")?;
            let api_key: String = arg(&args, "apiKey")?;
            ok(crate::services::coding_plan::get_coding_plan_quota(&base_url, &api_key).await?)
        }
        "get_subscription_quota" => {
            // 命令本体仅为发 usage-cache-updated 事件 / 刷新托盘才取 AppHandle+State，
            // Web 面板靠轮询刷新，这里只调用同一核心逻辑并跳过事件/托盘副作用。
            let tool: String = arg(&args, "tool")?;
            ok(crate::services::subscription::get_subscription_quota(&tool).await?)
        }
        "fetch_models_for_config" => {
            let base_url: String = arg(&args, "baseUrl")?;
            let api_key: String = arg(&args, "apiKey")?;
            let is_full_url: Option<bool> = arg(&args, "isFullUrl")?;
            let models_url: Option<String> = arg(&args, "modelsUrl")?;
            ok(crate::services::model_fetch::fetch_models(
                &base_url,
                &api_key,
                is_full_url.unwrap_or(false),
                models_url.as_deref(),
            )
            .await?)
        }
        "test_api_endpoints" => {
            let urls: Vec<String> = arg(&args, "urls")?;
            let timeout_secs: Option<u64> = arg(&args, "timeoutSecs")?;
            ok(SpeedtestService::test_endpoints(urls, timeout_secs)
                .await
                .map_err(|e| e.to_string())?)
        }
        "get_codex_oauth_quota" => {
            // 命令本体取 State<CodexOAuthState>，Web 网关无 AppHandle/State：
            // 复用全局 get_app_config_dir() 构造 CodexOAuthManager（与 lib.rs 初始化一致），
            // 调用同一 query_codex_quota 协议路径。
            let account_id: Option<String> = arg(&args, "accountId")?;
            let manager = CodexOAuthManager::new(crate::config::get_app_config_dir());
            let resolved = match account_id {
                Some(id) => Some(id),
                None => manager.default_account_id().await,
            };
            let Some(id) = resolved else {
                return ok(SubscriptionQuota::not_found("codex_oauth"));
            };
            let quota = match manager.get_valid_token_for_account(&id).await {
                Ok(token) => query_codex_quota(
                    token.as_str(),
                    Some(id.as_str()),
                    "codex_oauth",
                    "Codex OAuth access token expired or rejected. Please re-login via cc-switch.",
                )
                .await,
                Err(e) => SubscriptionQuota::error(
                    "codex_oauth",
                    CredentialStatus::Expired,
                    format!("Codex OAuth token unavailable: {e}"),
                ),
            };
            ok(quota)
        }
        "get_codex_oauth_models" => {
            // 同 get_codex_oauth_quota：无 State，经全局 config dir 构造 CodexOAuthManager。
            let account_id: Option<String> = arg(&args, "accountId")?;
            let manager = CodexOAuthManager::new(crate::config::get_app_config_dir());
            let resolved = match account_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                Some(id) => Some(id.to_string()),
                None => manager.default_account_id().await,
            };
            let Some(id) = resolved else {
                return Err("No ChatGPT account available".to_string());
            };
            let token = manager
                .get_valid_token_for_account(&id)
                .await
                .map_err(|e| format!("Codex OAuth token unavailable: {e}"))?;
            ok(crate::services::codex_oauth_models::fetch_models_with_token(&token, &id).await?)
        }

        // ==================== tool_misc ====================
        "get_tool_versions" => {
            let tools: Option<Vec<String>> = arg(&args, "tools")?;
            let wsl_shell_by_tool: Option<
                std::collections::HashMap<String, crate::commands::WslShellPreferenceInput>,
            > = arg(&args, "wslShellByTool")?;
            ok(crate::commands::get_tool_versions(tools, wsl_shell_by_tool).await?)
        }
        "probe_tool_installations" => {
            let tools: Vec<String> = arg(&args, "tools")?;
            ok(crate::commands::probe_tool_installations(tools).await?)
        }
        "run_tool_lifecycle_action" => {
            let tools: Vec<String> = arg(&args, "tools")?;
            let action: String = arg(&args, "action")?;
            let wsl_shell_by_tool: Option<
                std::collections::HashMap<String, crate::commands::WslShellPreferenceInput>,
            > = arg(&args, "wslShellByTool")?;
            crate::commands::run_tool_lifecycle_action(tools, action, wsl_shell_by_tool).await?;
            ok(Value::Null)
        }
        "get_usage_summary_by_app" => {
            let start_date: Option<i64> = arg(&args, "startDate")?;
            let end_date: Option<i64> = arg(&args, "endDate")?;
            let result: Vec<UsageSummaryByApp> = app
                .db
                .get_usage_summary_by_app(start_date, end_date)
                .map_err(|e| e.to_string())?;
            ok(result)
        }
        "read_live_provider_settings" => {
            let app_str: String = arg(&args, "app")?;
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(ProviderService::read_live_settings(app_type).map_err(|e| e.to_string())?)
        }
        "get_config_dir" => {
            let app_str: String = arg(&args, "app")?;
            let dir = match AppType::from_str(&app_str).map_err(|e| e.to_string())? {
                AppType::Claude => crate::config::get_claude_config_dir(),
                AppType::ClaudeDesktop => crate::claude_desktop_config::get_config_library_path()
                    .map_err(|e| e.to_string())?,
                AppType::Codex => crate::codex_config::get_codex_config_dir(),
                AppType::Gemini => crate::gemini_config::get_gemini_dir(),
                AppType::OpenCode => crate::opencode_config::get_opencode_dir(),
                AppType::OpenClaw => crate::openclaw_config::get_openclaw_dir(),
                AppType::Hermes => crate::hermes_config::get_hermes_dir(),
            };
            ok(dir.to_string_lossy().to_string())
        }
        "is_portable_mode" => {
            let exe_path =
                std::env::current_exe().map_err(|e| format!("获取可执行路径失败: {e}"))?;
            let portable = exe_path
                .parent()
                .map(|dir| dir.join("portable.ini").is_file())
                .unwrap_or(false);
            ok(portable)
        }
        "get_app_config_path" => ok(crate::config::get_app_config_path()
            .to_string_lossy()
            .to_string()),
        "get_app_config_dir_override" => ok(crate::app_store::get_app_config_dir_override()
            .map(|p| p.to_string_lossy().to_string())),

        // 未实现命令：明确报错（前端对应功能提示错误，但界面不崩溃）
        other => Err(format!("Web 控制台暂未支持的命令: {other}")),
    }
}

/// 复用 `commands::failover::set_auto_failover_enabled` 核心逻辑（跳过事件/托盘刷新）。
async fn set_auto_failover_enabled(app: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let app_type: String = arg(&args, "appType")?;
    let enabled: bool = arg(&args, "enabled")?;

    // 开启时强一致：确保队列非空并切到 P1
    let p1_provider_id = if enabled {
        let mut queue = app
            .db
            .get_failover_queue(&app_type)
            .map_err(|e| e.to_string())?;
        if queue.is_empty() {
            let app_enum =
                AppType::from_str(&app_type).map_err(|_| format!("无效的应用类型: {app_type}"))?;
            let current_id = crate::settings::get_effective_current_provider(&app.db, &app_enum)
                .map_err(|e| e.to_string())?;
            let Some(current_id) = current_id else {
                return Err("故障转移队列为空，且未设置当前供应商，无法开启故障转移".to_string());
            };
            app.db
                .add_to_failover_queue(&app_type, &current_id)
                .map_err(|e| e.to_string())?;
            queue = app
                .db
                .get_failover_queue(&app_type)
                .map_err(|e| e.to_string())?;
        }
        queue
            .first()
            .map(|item| item.provider_id.clone())
            .ok_or_else(|| "故障转移队列为空，无法开启故障转移".to_string())?
    } else {
        String::new()
    };

    let mut config = app
        .db
        .get_proxy_config_for_app(&app_type)
        .await
        .map_err(|e| e.to_string())?;
    config.auto_failover_enabled = enabled;
    app.db
        .update_proxy_config_for_app(config)
        .await
        .map_err(|e| e.to_string())?;

    if enabled {
        app.proxy_service
            .switch_proxy_target(&app_type, &p1_provider_id)
            .await?;
    }
    ok(Value::Null)
}

/// 复用 `commands::proxy::reset_circuit_breaker` 核心逻辑（AppHandle 传 None，跳过事件）。
async fn reset_circuit_breaker(app: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let provider_id: String = arg(&args, "providerId")?;
    let app_type: String = arg(&args, "appType")?;

    app.db
        .update_provider_health(&provider_id, &app_type, true, None)
        .await
        .map_err(|e| e.to_string())?;
    app.proxy_service
        .reset_provider_circuit_breaker(&provider_id, &app_type)
        .await?;

    let (app_enabled, auto_failover_enabled) =
        match app.db.get_proxy_config_for_app(&app_type).await {
            Ok(config) => (config.enabled, config.auto_failover_enabled),
            Err(_) => (false, false),
        };

    if app_enabled && auto_failover_enabled && app.proxy_service.is_running().await {
        if let Ok(Some(current_id)) = app.db.get_current_provider(&app_type) {
            let queue = app
                .db
                .get_failover_queue(&app_type)
                .map_err(|e| e.to_string())?;
            let restored_order = queue
                .iter()
                .find(|item| item.provider_id == provider_id)
                .and_then(|item| item.sort_index);
            let current_order = queue
                .iter()
                .find(|item| item.provider_id == current_id)
                .and_then(|item| item.sort_index);
            if let (Some(restored), Some(current)) = (restored_order, current_order) {
                if restored < current {
                    let provider_name = app
                        .db
                        .get_all_providers(&app_type)
                        .ok()
                        .and_then(|providers| providers.get(&provider_id).map(|p| p.name.clone()))
                        .unwrap_or_else(|| provider_id.clone());
                    let switch_manager =
                        crate::proxy::failover_switch::FailoverSwitchManager::new(app.db.clone());
                    // Web 面板无 AppHandle：传 None，事件由前端轮询弥补
                    if let Err(e) = switch_manager
                        .try_switch(None, &app_type, &provider_id, &provider_name)
                        .await
                    {
                        log::error!("[WebPanel] 自动切换失败: {e}");
                    }
                }
            }
        }
    }
    ok(Value::Null)
}

// ==================== 各域复用的辅助函数 ====================

/// 复用 `commands::usage::get_model_pricing` 核心逻辑。
/// `ModelPricingInfo` 定义在私有模块 `commands::usage`，无法跨模块引用，
/// 故直接以前端期望的 camelCase 键构造 JSON（与该结构体 serde 输出一致）。
fn get_model_pricing(app: &Arc<AppState>) -> Result<Value, String> {
    app.db
        .ensure_model_pricing_seeded()
        .map_err(|e| e.to_string())?;

    let conn = app
        .db
        .conn
        .lock()
        .map_err(|e| format!("Mutex lock failed: {e}"))?;

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='model_pricing'",
            [],
            |row| row.get::<_, i64>(0).map(|count| count > 0),
        )
        .unwrap_or(false);
    if !table_exists {
        return ok(Vec::<Value>::new());
    }

    let mut stmt = conn
        .prepare(
            "SELECT model_id, display_name, input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing
             ORDER BY display_name",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "modelId": row.get::<_, String>(0)?,
                "displayName": row.get::<_, String>(1)?,
                "inputCostPerMillion": row.get::<_, String>(2)?,
                "outputCostPerMillion": row.get::<_, String>(3)?,
                "cacheReadCostPerMillion": row.get::<_, String>(4)?,
                "cacheCreationCostPerMillion": row.get::<_, String>(5)?,
            }))
        })
        .map_err(|e| e.to_string())?;

    let mut pricing: Vec<Value> = Vec::new();
    for row in rows {
        pricing.push(row.map_err(|e| e.to_string())?);
    }
    ok(pricing)
}

/// 复用 `commands::provider::import_default_config_internal` 核心逻辑。
async fn import_default_config(app: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let app_str: String = arg(&args, "app")?;
    let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;

    let imported = ProviderService::import_default_config(app.as_ref(), app_type.clone())
        .map_err(|e| e.to_string())?;

    if imported {
        if app
            .db
            .should_auto_extract_config_snippet(app_type.as_str())
            .map_err(|e| e.to_string())?
        {
            if let Ok(snippet) =
                ProviderService::extract_common_config_snippet(app.as_ref(), app_type.clone())
            {
                if !snippet.is_empty() && snippet != "{}" {
                    let _ = app.db.set_config_snippet(app_type.as_str(), Some(snippet));
                    let _ = app.db.set_config_snippet_cleared(app_type.as_str(), false);
                }
            }
        }

        ProviderService::migrate_legacy_common_config_usage_if_needed(app.as_ref(), app_type)
            .map_err(|e| e.to_string())?;
    }

    ok(imported)
}

/// 复用 `commands::config::set_common_config_snippet` 核心逻辑。
async fn set_common_config_snippet(app: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let app_type: String = arg(&args, "appType")?;
    let snippet: String = arg(&args, "snippet")?;
    let is_cleared = snippet.trim().is_empty();

    if !is_cleared {
        match app_type.as_str() {
            "claude" | "gemini" | "omo" | "omo-slim" => {
                serde_json::from_str::<Value>(&snippet)
                    .map_err(|e| format!("无效的 JSON 格式: {e}"))?;
            }
            "codex" => {
                snippet
                    .parse::<toml_edit::DocumentMut>()
                    .map_err(|e| format!("无效的 TOML 格式: {e}"))?;
            }
            _ => {}
        }
    }

    let old_snippet = app
        .db
        .get_config_snippet(&app_type)
        .map_err(|e| e.to_string())?;
    let value = if is_cleared { None } else { Some(snippet) };

    if matches!(app_type.as_str(), "claude" | "codex" | "gemini") {
        if let Some(legacy_snippet) = old_snippet
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let app_enum = AppType::from_str(&app_type).map_err(|e| e.to_string())?;
            ProviderService::migrate_legacy_common_config_usage(
                app.as_ref(),
                app_enum,
                legacy_snippet,
            )
            .map_err(|e| e.to_string())?;
        }
    }

    app.db
        .set_config_snippet(&app_type, value)
        .map_err(|e| e.to_string())?;
    app.db
        .set_config_snippet_cleared(&app_type, is_cleared)
        .map_err(|e| e.to_string())?;

    if matches!(app_type.as_str(), "claude" | "codex" | "gemini") {
        let app_enum = AppType::from_str(&app_type).map_err(|e| e.to_string())?;
        ProviderService::sync_current_provider_for_app(app.as_ref(), app_enum)
            .map_err(|e| e.to_string())?;
    }

    if app_type == "omo"
        && app
            .db
            .get_current_omo_provider("opencode", "omo")
            .map_err(|e| e.to_string())?
            .is_some()
    {
        crate::services::OmoService::write_config_to_file(
            app.as_ref(),
            &crate::services::omo::STANDARD,
        )
        .map_err(|e| e.to_string())?;
    }
    if app_type == "omo-slim"
        && app
            .db
            .get_current_omo_provider("opencode", "omo-slim")
            .map_err(|e| e.to_string())?
            .is_some()
    {
        crate::services::OmoService::write_config_to_file(
            app.as_ref(),
            &crate::services::omo::SLIM,
        )
        .map_err(|e| e.to_string())?;
    }

    ok(Value::Null)
}

/// 更新全局代理配置；地址/端口变化且代理运行中时经 ProxyService::update_config 重绑定，避免脑裂。
///
/// 顺序要点：必须在写入新全局配置之前调用 update_config（它读取 DB 旧值作为 previous 判定是否重启）。
async fn update_global_proxy_config(app: &Arc<AppState>, args: Value) -> Result<Value, String> {
    let config: GlobalProxyConfig = arg(&args, "config")?;

    let previous = app
        .db
        .get_global_proxy_config()
        .await
        .map_err(|e| e.to_string())?;

    let address_or_port_changed = previous.listen_address != config.listen_address
        || previous.listen_port != config.listen_port;

    if address_or_port_changed && app.proxy_service.is_running().await {
        let mut proxy_config = app.db.get_proxy_config().await.map_err(|e| e.to_string())?;
        proxy_config.listen_address = config.listen_address.clone();
        proxy_config.listen_port = config.listen_port;
        proxy_config.enable_logging = config.enable_logging;
        app.proxy_service.update_config(&proxy_config).await?;
    }

    app.db
        .update_global_proxy_config(config)
        .await
        .map_err(|e| e.to_string())?;

    ok(Value::Null)
}

/// 复用 `commands::settings::merge_settings_for_save` 合并语义：空密码代表“保持现有”。
fn merge_settings_for_save_web(
    mut incoming: crate::settings::AppSettings,
    existing: &crate::settings::AppSettings,
) -> crate::settings::AppSettings {
    match (&mut incoming.webdav_sync, &existing.webdav_sync) {
        (None, _) => {
            incoming.webdav_sync = existing.webdav_sync.clone();
        }
        (Some(incoming_sync), Some(existing_sync))
            if incoming_sync.password.is_empty() && !existing_sync.password.is_empty() =>
        {
            incoming_sync.password = existing_sync.password.clone();
        }
        _ => {}
    }
    incoming
}

/// 复用 `commands::webdav_sync::resolve_password_for_request`：空密码且要求保留时回填现有密码。
fn resolve_webdav_password(
    mut incoming: crate::settings::WebDavSyncSettings,
    existing: Option<crate::settings::WebDavSyncSettings>,
    preserve_empty_password: bool,
) -> crate::settings::WebDavSyncSettings {
    if let Some(existing_settings) = existing {
        if preserve_empty_password && incoming.password.is_empty() {
            incoming.password = existing_settings.password;
        }
    }
    incoming
}

/// 要求 WebDAV 已配置且已启用。
fn require_enabled_webdav_settings_web() -> Result<crate::settings::WebDavSyncSettings, String> {
    let settings = crate::settings::get_webdav_sync_settings()
        .ok_or_else(|| "未配置 WebDAV 同步".to_string())?;
    if !settings.enabled {
        return Err("WebDAV 同步未启用".to_string());
    }
    Ok(settings)
}

/// 将同步错误写入状态字段（不覆盖凭据）。
fn persist_webdav_sync_error(
    settings: &mut crate::settings::WebDavSyncSettings,
    error: &str,
    source: &str,
) {
    settings.status.last_error = Some(error.to_string());
    settings.status.last_error_source = Some(source.to_string());
    let _ = crate::settings::update_webdav_sync_status(settings.status.clone());
}
