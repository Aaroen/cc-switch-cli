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

/// 提取并反序列化指定参数键（camelCase）。
fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    let v = args.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| format!("参数 {key} 无效: {e}"))
}

/// 将结果序列化为 JSON 信封 data。
fn ok<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
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
        "get_usage_data_sources" => ok(
            crate::services::session_usage::get_data_source_breakdown(&app.db)
                .map_err(|e| e.to_string())?,
        ),

        // ==================== 供应商 CRUD / 导入 ====================
        "add_provider" => {
            let app_str: String = arg(&args, "app")?;
            let provider: Provider = arg(&args, "provider")?;
            let add_to_live: Option<bool> = args.get("addToLive").and_then(|v| v.as_bool());
            let app_type = AppType::from_str(&app_str).map_err(|e| e.to_string())?;
            ok(
                ProviderService::add(app.as_ref(), app_type, provider, add_to_live.unwrap_or(true))
                    .map_err(|e| e.to_string())?,
            )
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
            ok(ProviderService::update_sort_order(app.as_ref(), app_type, updates)
                .map_err(|e| e.to_string())?)
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
        "get_claude_common_config_snippet" => {
            ok(app.db.get_config_snippet("claude").map_err(|e| e.to_string())?)
        }
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
            ok(crate::database::Database::rename_backup(&old_filename, &new_name)
                .map_err(|e| e.to_string())?)
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

        // ==================== 启动期 / 桌面专属（Web 降级）====================
        "get_init_error" => ok(Value::Null), // 浏览器无后端初始化错误事件
        "update_tray_menu" => ok(true),      // 无托盘：no-op，避免变更后置回调报错

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

    let (app_enabled, auto_failover_enabled) = match app.db.get_proxy_config_for_app(&app_type).await
    {
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
        crate::services::OmoService::write_config_to_file(app.as_ref(), &crate::services::omo::STANDARD)
            .map_err(|e| e.to_string())?;
    }
    if app_type == "omo-slim"
        && app
            .db
            .get_current_omo_provider("opencode", "omo-slim")
            .map_err(|e| e.to_string())?
            .is_some()
    {
        crate::services::OmoService::write_config_to_file(app.as_ref(), &crate::services::omo::SLIM)
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
        let mut proxy_config = app
            .db
            .get_proxy_config()
            .await
            .map_err(|e| e.to_string())?;
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
