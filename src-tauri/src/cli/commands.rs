//! CLI命令执行逻辑
//!
//! 实现所有子命令的具体执行逻辑

use super::output;
use crate::{app_config::AppType, database::Database, provider::Provider};
use serde_json::json;
use std::str::FromStr;
use std::sync::Arc;

// ============================================================================
// Provider 命令实现
// ============================================================================

pub async fn provider_list(app: &str, verbose: bool) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let providers = db
        .get_all_providers(app_type.as_str())
        .map_err(|e| e.to_string())?;

    if providers.is_empty() {
        output::warning(&format!("没有找到 {} 的供应商", app));
        return Ok(());
    }

    let current_id = db
        .get_current_provider(app_type.as_str())
        .map_err(|e| e.to_string())?;

    output::section(&format!("{} 供应商列表", app.to_uppercase()));

    if verbose {
        for (id, provider) in providers {
            let is_current = current_id.as_ref().map(|c| c == &id).unwrap_or(false);
            let marker = if is_current { " [当前]" } else { "" };

            println!(
                "\n{} {}{}",
                output::status_indicator(true),
                provider.name,
                marker
            );
            output::key_value(vec![
                ("ID", id.clone()),
                (
                    "权重",
                    format!("{} (频率: 1/{})", provider.weight, provider.weight),
                ),
                (
                    "类别",
                    provider.category.unwrap_or_else(|| "未分类".to_string()),
                ),
                (
                    "故障转移",
                    if provider.in_failover_queue {
                        "是"
                    } else {
                        "否"
                    }
                    .to_string(),
                ),
                ("创建时间", format_timestamp(provider.created_at)),
            ]);

            if let Some(notes) = provider.notes {
                println!("  备注: {}", notes);
            }
        }
    } else {
        let headers = vec!["ID", "名称", "权重", "当前", "故障转移"];
        let rows: Vec<Vec<String>> = providers
            .iter()
            .map(|(id, p)| {
                let is_current = current_id.as_ref().map(|c| c == id).unwrap_or(false);
                vec![
                    id.clone(),
                    p.name.clone(),
                    p.weight.to_string(),
                    if is_current { "✓" } else { "" }.to_string(),
                    if p.in_failover_queue { "✓" } else { "" }.to_string(),
                ]
            })
            .collect();

        output::table(headers, rows);
    }

    Ok(())
}

pub async fn provider_add(
    app: &str,
    name: &str,
    key: Option<String>,
    url: Option<String>,
    file: Option<String>,
) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    // 从文件或参数构建配置
    let settings_config = if let Some(file_path) = file {
        // 从文件读取配置
        let content =
            std::fs::read_to_string(&file_path).map_err(|e| format!("读取配置文件失败: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("解析配置文件失败: {}", e))?
    } else {
        // 从参数构建配置
        let mut config = json!({});

        if let Some(api_key) = key {
            match app_type {
                AppType::Claude => {
                    config["env"] = json!({
                        "ANTHROPIC_API_KEY": api_key
                    });
                }
                AppType::Codex => {
                    config["env"] = json!({
                        "OPENAI_API_KEY": api_key
                    });
                }
                AppType::Gemini => {
                    config["env"] = json!({
                        "GEMINI_API_KEY": api_key
                    });
                }
            }
        }

        if let Some(base_url) = url {
            match app_type {
                AppType::Claude => {
                    let env = config.get_mut("env").and_then(|v| v.as_object_mut());
                    if let Some(env) = env {
                        env.insert("ANTHROPIC_BASE_URL".to_string(), json!(base_url));
                    } else {
                        config["env"] = json!({
                            "ANTHROPIC_BASE_URL": base_url
                        });
                    }
                }
                AppType::Codex => {
                    config["base_url"] = json!(base_url);
                }
                AppType::Gemini => {
                    let env = config.get_mut("env").and_then(|v| v.as_object_mut());
                    if let Some(env) = env {
                        env.insert("GEMINI_API_BASE_URL".to_string(), json!(base_url));
                    } else {
                        config["env"] = json!({
                            "GEMINI_API_BASE_URL": base_url
                        });
                    }
                }
            }
        }

        config
    };

    // 生成ID
    let id = uuid::Uuid::new_v4().to_string();

    // 创建Provider
    let provider = Provider {
        id: id.clone(),
        name: name.to_string(),
        settings_config,
        website_url: None,
        category: Some(app.to_string()),
        created_at: Some(chrono::Utc::now().timestamp_millis()),
        sort_index: None,
        notes: None,
        meta: None,
        icon: None,
        icon_color: None,
        in_failover_queue: false,
        weight: 1, // 默认权重
    };

    // 保存到数据库
    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    output::success(&format!("供应商 '{}' 已添加 (ID: {})", name, id));
    output::hint("提示: 使用 'cc-switch provider switch' 切换到此供应商");

    Ok(())
}

pub async fn provider_remove(app: &str, id: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    // 检查是否存在
    let provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    // 确认删除
    if !output::confirm(&format!("确认删除供应商 '{}'?", provider.name)) {
        output::info("已取消");
        return Ok(());
    }

    db.delete_provider(app_type.as_str(), id)
        .map_err(|e| e.to_string())?;

    output::success(&format!("供应商 '{}' 已删除", provider.name));

    Ok(())
}

pub async fn provider_switch(app: &str, id: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    // 检查是否存在
    let provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    db.set_current_provider(app_type.as_str(), id)
        .map_err(|e| e.to_string())?;

    output::success(&format!("已切换到供应商: {}", provider.name));

    Ok(())
}

pub async fn provider_set_weight(app: &str, id: &str, weight: u32) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    // 检查权重范围
    if weight > 10 {
        return Err("权重必须在0-10范围内".to_string());
    }

    // 检查供应商是否存在
    let provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    db.update_provider_weight(app_type.as_str(), id, weight)
        .map_err(|e| e.to_string())?;

    output::success(&format!(
        "供应商 '{}' 权重已设置为 {}",
        provider.name, weight
    ));

    if weight == 0 {
        output::warning("权重为0，此供应商已禁用");
    } else {
        output::info(&format!("频率: 每{}轮使用一次", weight));
    }

    Ok(())
}

pub async fn provider_set_model_mapping(
    app: &str,
    id: &str,
    from: &str,
    to: &str,
) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    let mapping = provider
        .settings_config
        .get_mut("model_mapping")
        .and_then(|v| v.as_object_mut())
        .map(|m| {
            m.insert(from.to_string(), serde_json::Value::String(to.to_string()));
            m
        });

    if mapping.is_none() {
        provider.settings_config["model_mapping"] = serde_json::json!({
            from: to
        });
    }

    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    output::success(&format!(
        "供应商 '{}' 模型映射已设置: {} → {}",
        provider.name, from, to
    ));

    Ok(())
}

pub async fn provider_set_env(app: &str, id: &str, key: &str, value: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    let env = provider
        .settings_config
        .get_mut("env")
        .and_then(|v| v.as_object_mut());

    if let Some(env) = env {
        env.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    } else {
        provider.settings_config["env"] = serde_json::json!({
            key: value
        });
    }

    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    output::success(&format!(
        "供应商 '{}' env 已设置: {} = {}",
        provider.name, key, value
    ));

    Ok(())
}

pub async fn provider_show(app: &str, id: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    output::section(&format!("供应商详情: {}", provider.name));

    output::key_value(vec![
        ("ID", provider.id.clone()),
        ("名称", provider.name.clone()),
        (
            "权重",
            format!("{} (频率: 1/{})", provider.weight, provider.weight),
        ),
        (
            "类别",
            provider.category.unwrap_or_else(|| "未分类".to_string()),
        ),
        (
            "故障转移",
            if provider.in_failover_queue {
                "是"
            } else {
                "否"
            }
            .to_string(),
        ),
        ("创建时间", format_timestamp(provider.created_at)),
    ]);

    if let Some(notes) = provider.notes {
        println!("\n备注:\n{}", notes);
    }

    println!("\n配置:");
    output::json(&provider.settings_config);

    Ok(())
}

pub async fn provider_test(_app: &str, _id: &str) -> Result<(), String> {
    output::info("功能开发中...");
    // TODO: 实现供应商连接测试
    Ok(())
}

// ============================================================================
// Config 命令实现
// ============================================================================

pub async fn config_show(app: Option<String>) -> Result<(), String> {
    let db = get_database()?;

    if let Some(app_str) = app {
        let app_type = parse_app_type(&app_str)?;

        output::section(&format!("{} 配置", app_str.to_uppercase()));

        // 显示当前供应商
        if let Ok(Some(current_id)) = db.get_current_provider(app_type.as_str()) {
            if let Ok(Some(provider)) = db.get_provider_by_id(&current_id, app_type.as_str()) {
                output::key_value(vec![("当前供应商", provider.name.clone())]);
            }
        }

        // 显示代理配置
        if let Ok(app_config) = db.get_proxy_config_for_app(app_type.as_str()).await {
            // 获取全局配置以读取 host 和 port
            if let Ok(global_config) = db.get_global_proxy_config().await {
                println!("\n代理配置:");
                output::key_value(vec![
                    (
                        "启用",
                        if app_config.enabled { "是" } else { "否" }.to_string(),
                    ),
                    (
                        "监听地址",
                        format!(
                            "{}:{}",
                            global_config.listen_address, global_config.listen_port
                        ),
                    ),
                    (
                        "非流式超时",
                        format!("{}秒", app_config.non_streaming_timeout),
                    ),
                    (
                        "流式超时",
                        format!("{}秒", app_config.streaming_idle_timeout),
                    ),
                ]);
            }
        }

        // 显示熔断器配置
        if let Ok(cb_config) = db.get_circuit_breaker_config().await {
            println!("\n熔断器配置:");
            output::key_value(vec![
                ("失败阈值", cb_config.failure_threshold.to_string()),
                ("成功阈值", cb_config.success_threshold.to_string()),
                ("超时时间", format!("{}秒", cb_config.timeout_seconds)),
                ("最小请求数", cb_config.min_requests.to_string()),
            ]);
        }
    } else {
        // 显示全局配置
        output::section("全局配置");

        if let Ok(state) = db.get_global_proxy_state() {
            let desc = match state {
                crate::database::GlobalProxyState::Unset => {
                    "auto（未设置，启动时可继承环境变量代理）".to_string()
                }
                crate::database::GlobalProxyState::Direct => {
                    "direct（显式直连，忽略环境变量代理）".to_string()
                }
                crate::database::GlobalProxyState::Proxy(url) => url,
            };
            output::key_value(vec![("上游代理", desc)]);
        }

        // 显示所有应用的启用状态
        println!("\n应用启用状态:");
        for app in ["claude", "codex", "gemini"] {
            if let Ok(config) = db.get_proxy_config_for_app(app).await {
                println!(
                    "  {} {}: {}",
                    output::status_indicator(config.enabled),
                    app,
                    if config.enabled { "启用" } else { "禁用" }
                );
            }
        }
    }

    Ok(())
}

pub async fn config_set(key: &str, value: &str, app: Option<String>) -> Result<(), String> {
    let db = get_database()?;

    match key {
        "global_proxy" => {
            let proxy_url = if value.is_empty() { None } else { Some(value) };

            db.set_global_proxy_url(proxy_url)
                .map_err(|e| e.to_string())?;

            output::success("全局代理已更新");
        }
        _ => {
            // 应用特定配置
            if let Some(app_str) = app {
                let app_type = parse_app_type(&app_str)?;

                match key {
                    "port" => {
                        let port = value
                            .parse::<u16>()
                            .map_err(|_| "无效的端口号".to_string())?;

                        let mut global_config = db
                            .get_global_proxy_config()
                            .await
                            .map_err(|e| e.to_string())?;

                        global_config.listen_port = port;

                        db.update_global_proxy_config(global_config)
                            .await
                            .map_err(|e| e.to_string())?;

                        output::success(&format!("端口已更新为 {}", port));
                    }
                    "enabled" => {
                        let enabled = value
                            .parse::<bool>()
                            .map_err(|_| "无效的布尔值（使用 true/false）".to_string())?;

                        let mut config = db
                            .get_proxy_config_for_app(app_type.as_str())
                            .await
                            .map_err(|e| e.to_string())?;

                        config.enabled = enabled;

                        db.update_proxy_config_for_app(config)
                            .await
                            .map_err(|e| e.to_string())?;

                        output::success(&format!(
                            "{} 代理已{}",
                            app_str,
                            if enabled { "启用" } else { "禁用" }
                        ));
                    }
                    _ => {
                        return Err(format!("未知配置项: {}", key));
                    }
                }
            } else {
                return Err("应用特定配置需要指定 --app 参数".to_string());
            }
        }
    }

    Ok(())
}

pub async fn config_export(_output_path: &str) -> Result<(), String> {
    output::info("导出配置功能开发中...");
    // TODO: 实现配置导出
    Ok(())
}

pub async fn config_import(_input_path: &str) -> Result<(), String> {
    output::info("导入配置功能开发中...");
    // TODO: 实现配置导入
    Ok(())
}

pub async fn config_proxy(app: Option<String>) -> Result<(), String> {
    let db = get_database()?;

    if let Some(app_str) = app {
        let app_type = parse_app_type(&app_str)?;
        let app_config = db
            .get_proxy_config_for_app(app_type.as_str())
            .await
            .map_err(|e| e.to_string())?;

        // 获取全局配置以读取 host 和 port
        let global_config = db
            .get_global_proxy_config()
            .await
            .map_err(|e| e.to_string())?;

        output::section(&format!("{} 代理配置", app_str.to_uppercase()));
        output::key_value(vec![
            (
                "启用",
                if app_config.enabled { "是" } else { "否" }.to_string(),
            ),
            (
                "监听地址",
                format!(
                    "{}:{}",
                    global_config.listen_address, global_config.listen_port
                ),
            ),
            (
                "非流式超时",
                format!("{}秒", app_config.non_streaming_timeout),
            ),
            (
                "流式首字节超时",
                format!("{}秒", app_config.streaming_first_byte_timeout),
            ),
            (
                "流式空闲超时",
                format!("{}秒", app_config.streaming_idle_timeout),
            ),
        ]);
    } else {
        // 显示所有应用的代理配置
        output::section("代理配置");

        // 获取全局配置
        let global_config = db
            .get_global_proxy_config()
            .await
            .map_err(|e| e.to_string())?;

        for app in ["claude", "codex", "gemini"] {
            if let Ok(config) = db.get_proxy_config_for_app(app).await {
                println!(
                    "\n{} {}:",
                    output::status_indicator(config.enabled),
                    app.to_uppercase()
                );
                output::key_value(vec![
                    (
                        "  地址",
                        format!(
                            "{}:{}",
                            global_config.listen_address, global_config.listen_port
                        ),
                    ),
                    ("  超时", format!("{}s", config.non_streaming_timeout)),
                ]);
            }
        }
    }

    Ok(())
}

pub async fn config_loadbalance(app: &str, enabled: Option<bool>) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut config = db
        .get_proxy_config_for_app(app_type.as_str())
        .await
        .map_err(|e| e.to_string())?;

    if let Some(enable) = enabled {
        // 设置权重轮询开关
        config.weight_round_robin_enabled = enable;

        db.update_proxy_config_for_app(config.clone())
            .await
            .map_err(|e| e.to_string())?;

        output::success(&format!(
            "{} 权重轮询已{}",
            app.to_uppercase(),
            if enable { "启用" } else { "禁用" }
        ));

        if enable {
            output::info("权重轮询模式：按供应商权重分配请求");
            output::hint("使用 'csc provider weight' 设置供应商权重");
        }
    } else {
        // 显示当前状态
        output::section(&format!("{} 权重轮询配置", app.to_uppercase()));

        let status = if config.weight_round_robin_enabled {
            "启用"
        } else {
            "禁用"
        };

        output::key_value(vec![
            ("状态", status.to_string()),
            (
                "自动故障转移",
                if config.auto_failover_enabled {
                    "启用"
                } else {
                    "禁用"
                }
                .to_string(),
            ),
        ]);

        // 显示供应商权重列表
        let providers = db
            .get_all_providers(app_type.as_str())
            .map_err(|e| e.to_string())?;

        if !providers.is_empty() {
            println!("\n供应商权重:");
            let headers = vec!["ID", "名称", "权重", "频率"];
            let rows: Vec<Vec<String>> = providers
                .iter()
                .map(|(id, p)| {
                    let freq = if p.weight == 0 {
                        "禁用".to_string()
                    } else {
                        format!("1/{}", p.weight)
                    };
                    vec![id.clone(), p.name.clone(), p.weight.to_string(), freq]
                })
                .collect();

            output::table(headers, rows);
        }

        output::hint(&format!(
            "使用 'csc config lb --app {} --enabled true' 启用权重轮询",
            app
        ));
    }

    Ok(())
}

// ============================================================================
// Failover 命令实现
// ============================================================================

pub async fn failover_queue(app: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let providers = db
        .get_all_providers(app_type.as_str())
        .map_err(|e| e.to_string())?;

    let queue: Vec<_> = providers
        .iter()
        .filter(|(_, p)| p.in_failover_queue)
        .collect();

    if queue.is_empty() {
        output::warning(&format!("{} 没有供应商在故障转移队列中", app));
        output::hint("使用 'cc-switch failover add' 添加供应商到队列");
        return Ok(());
    }

    output::section(&format!("{} 故障转移队列", app.to_uppercase()));

    let headers = vec!["顺序", "ID", "名称", "权重"];
    let rows: Vec<Vec<String>> = queue
        .iter()
        .enumerate()
        .map(|(idx, (id, p))| {
            vec![
                (idx + 1).to_string(),
                id.to_string(),
                p.name.clone(),
                p.weight.to_string(),
            ]
        })
        .collect();

    output::table(headers, rows);

    // 显示自动故障转移状态
    if let Ok(app_config) = db.get_proxy_config_for_app(app_type.as_str()).await {
        println!(
            "\n自动故障转移: {}",
            if app_config.auto_failover_enabled {
                "启用"
            } else {
                "禁用"
            }
        );
    }

    Ok(())
}

pub async fn failover_add(app: &str, id: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    // 检查供应商是否存在
    let mut provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    if provider.in_failover_queue {
        output::warning(&format!("供应商 '{}' 已在队列中", provider.name));
        return Ok(());
    }

    provider.in_failover_queue = true;

    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    output::success(&format!("供应商 '{}' 已添加到故障转移队列", provider.name));

    Ok(())
}

pub async fn failover_remove(app: &str, id: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    if !provider.in_failover_queue {
        output::warning(&format!("供应商 '{}' 不在队列中", provider.name));
        return Ok(());
    }

    provider.in_failover_queue = false;

    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    output::success(&format!("供应商 '{}' 已从故障转移队列移除", provider.name));

    Ok(())
}

pub async fn failover_toggle(app: &str, enabled: bool) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut config = db
        .get_proxy_config_for_app(app_type.as_str())
        .await
        .map_err(|e| e.to_string())?;

    config.auto_failover_enabled = enabled;

    db.update_proxy_config_for_app(config)
        .await
        .map_err(|e| e.to_string())?;

    output::success(&format!(
        "{} 自动故障转移已{}",
        app,
        if enabled { "启用" } else { "禁用" }
    ));

    Ok(())
}

pub async fn failover_circuit_breaker(_app: &str, _id: Option<String>) -> Result<(), String> {
    output::info("熔断器状态查看功能开发中...");
    // TODO: 实现熔断器状态查看
    Ok(())
}

pub async fn failover_reset(_app: &str, _id: &str) -> Result<(), String> {
    output::info("熔断器重置功能开发中...");
    // TODO: 实现熔断器重置
    Ok(())
}

// ============================================================================
// Stats 命令实现
// ============================================================================

pub async fn stats_summary(_days: u32, _app: Option<String>) -> Result<(), String> {
    output::info("统计摘要功能开发中...");
    // TODO: 实现统计摘要
    Ok(())
}

pub async fn stats_provider(_app: &str, _id: Option<String>, _days: u32) -> Result<(), String> {
    output::info("供应商统计功能开发中...");
    // TODO: 实现供应商统计
    Ok(())
}

pub async fn stats_model(_days: u32) -> Result<(), String> {
    output::info("模型统计功能开发中...");
    // TODO: 实现模型统计
    Ok(())
}

pub async fn stats_logs(
    _limit: u32,
    _app: Option<String>,
    _provider: Option<String>,
) -> Result<(), String> {
    output::info("请求日志功能开发中...");
    // TODO: 实现请求日志
    Ok(())
}

// ============================================================================
// MCP 命令实现（简化版）
// ============================================================================

pub async fn mcp_list(_app: Option<String>) -> Result<(), String> {
    output::info("MCP服务器列表功能开发中...");
    Ok(())
}

pub async fn mcp_add(
    _name: &str,
    _command: &str,
    _args: Vec<String>,
    _enabled: Vec<String>,
) -> Result<(), String> {
    output::info("MCP服务器添加功能开发中...");
    Ok(())
}

pub async fn mcp_remove(_name: &str) -> Result<(), String> {
    output::info("MCP服务器删除功能开发中...");
    Ok(())
}

pub async fn mcp_toggle(_name: &str, _app: &str, _enabled: bool) -> Result<(), String> {
    output::info("MCP服务器切换功能开发中...");
    Ok(())
}

// ============================================================================
// Prompt 命令实现（简化版）
// ============================================================================

pub async fn prompt_list(_app: Option<String>) -> Result<(), String> {
    output::info("提示词列表功能开发中...");
    Ok(())
}

pub async fn prompt_add(_name: &str, _content: &str, _app: &str) -> Result<(), String> {
    output::info("提示词添加功能开发中...");
    Ok(())
}

pub async fn prompt_remove(_name: &str, _app: &str) -> Result<(), String> {
    output::info("提示词删除功能开发中...");
    Ok(())
}

pub async fn prompt_show(_name: &str, _app: &str) -> Result<(), String> {
    output::info("提示词查看功能开发中...");
    Ok(())
}

// ============================================================================
// Skill 命令实现（简化版）
// ============================================================================

pub async fn skill_list(_app: Option<String>) -> Result<(), String> {
    output::info("技能列表功能开发中...");
    Ok(())
}

pub async fn skill_install(_id: &str, _apps: Vec<String>) -> Result<(), String> {
    output::info("技能安装功能开发中...");
    Ok(())
}

pub async fn skill_uninstall(_id: &str, _app: Option<String>) -> Result<(), String> {
    output::info("技能卸载功能开发中...");
    Ok(())
}

pub async fn skill_discover() -> Result<(), String> {
    output::info("技能发现功能开发中...");
    Ok(())
}

// ============================================================================
// 辅助函数
// ============================================================================

fn get_database() -> Result<Arc<Database>, String> {
    Database::init()
        .map(Arc::new)
        .map_err(|e| format!("数据库初始化失败: {}", e))
}

fn parse_app_type(app: &str) -> Result<AppType, String> {
    AppType::from_str(app).map_err(|e| format!("无效的应用类型 '{}': {}", app, e))
}

fn format_timestamp(ts: Option<i64>) -> String {
    ts.map(|t| {
        chrono::DateTime::from_timestamp_millis(t)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "无效时间".to_string())
    })
    .unwrap_or_else(|| "未知".to_string())
}
