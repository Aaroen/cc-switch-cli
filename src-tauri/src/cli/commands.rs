//! CLI命令执行逻辑
//!
//! 实现所有子命令的具体执行逻辑

use super::output;
use crate::{app_config::AppType, database::Database, provider::Provider};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Read;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderExportBundleV1 {
    /// 文件格式版本
    version: u32,
    /// 应用类型（claude/codex/gemini）
    app: String,
    /// 导出时间（毫秒时间戳）
    exported_at_ms: i64,
    /// 导出时的 current provider id（若导出的集合包含 current）
    current: Option<String>,
    /// 供应商列表
    providers: Vec<Provider>,
}

fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("api_key")
        || k.contains("apikey")
        || k.contains("key")
        || k.contains("token")
        || k.contains("secret")
        || k.contains("password")
        || k.contains("auth")
}

fn redact_json_value(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, vv) in map.iter_mut() {
                if is_secret_key(k) {
                    *vv = serde_json::Value::String("***".to_string());
                } else {
                    redact_json_value(vv);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for vv in arr.iter_mut() {
                redact_json_value(vv);
            }
        }
        _ => {}
    }
}

fn redact_provider(mut p: Provider) -> Provider {
    redact_json_value(&mut p.settings_config);
    p
}

fn deep_merge_json(dst: &mut serde_json::Value, src: serde_json::Value) {
    match (dst, src) {
        (serde_json::Value::Object(dst_map), serde_json::Value::Object(src_map)) => {
            for (k, v) in src_map {
                match dst_map.get_mut(&k) {
                    Some(existing) => deep_merge_json(existing, v),
                    None => {
                        dst_map.insert(k, v);
                    }
                }
            }
        }
        (dst_any, src_any) => {
            *dst_any = src_any;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonPathSegment {
    Key(String),
    Index(usize),
}

fn parse_json_path(path: &str) -> Result<Vec<JsonPathSegment>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("JSON 路径不能为空".to_string());
    }

    trimmed
        .split('.')
        .map(|segment| {
            let segment = segment.trim();
            if segment.is_empty() {
                return Err(format!("JSON 路径无效: {}", path));
            }

            if segment.chars().all(|ch| ch.is_ascii_digit()) {
                segment
                    .parse::<usize>()
                    .map(JsonPathSegment::Index)
                    .map_err(|_| format!("JSON 路径数组索引无效: {}", segment))
            } else {
                Ok(JsonPathSegment::Key(segment.to_string()))
            }
        })
        .collect()
}

fn json_container_for(next: &JsonPathSegment) -> Value {
    match next {
        JsonPathSegment::Key(_) => Value::Object(Map::new()),
        JsonPathSegment::Index(_) => Value::Array(Vec::new()),
    }
}

fn get_json_path_value<'a>(
    current: &'a Value,
    segments: &[JsonPathSegment],
    full_path: &str,
) -> Result<&'a Value, String> {
    if segments.is_empty() {
        return Ok(current);
    }

    match &segments[0] {
        JsonPathSegment::Key(key) => match current {
            Value::Object(map) => map
                .get(key)
                .ok_or_else(|| format!("未找到超参数路径: {}", full_path))
                .and_then(|next| get_json_path_value(next, &segments[1..], full_path)),
            _ => Err(format!(
                "超参数路径 '{}' 在键 '{}' 之前命中了非对象节点",
                full_path, key
            )),
        },
        JsonPathSegment::Index(index) => match current {
            Value::Array(items) => items
                .get(*index)
                .ok_or_else(|| format!("未找到超参数路径: {}", full_path))
                .and_then(|next| get_json_path_value(next, &segments[1..], full_path)),
            _ => Err(format!(
                "超参数路径 '{}' 在索引 '{}' 之前命中了非数组节点",
                full_path, index
            )),
        },
    }
}

fn set_json_path_value(
    current: &mut Value,
    segments: &[JsonPathSegment],
    new_value: Value,
    full_path: &str,
) -> Result<(), String> {
    if segments.is_empty() {
        *current = new_value;
        return Ok(());
    }

    match &segments[0] {
        JsonPathSegment::Key(key) => {
            if current.is_null() {
                *current = Value::Object(Map::new());
            }

            match current {
                Value::Object(map) => {
                    if segments.len() == 1 {
                        map.insert(key.clone(), new_value);
                        Ok(())
                    } else {
                        let next = map
                            .entry(key.clone())
                            .or_insert_with(|| json_container_for(&segments[1]));
                        if next.is_null() {
                            *next = json_container_for(&segments[1]);
                        }
                        set_json_path_value(next, &segments[1..], new_value, full_path)
                    }
                }
                _ => Err(format!(
                    "超参数路径 '{}' 在键 '{}' 之前命中了非对象节点",
                    full_path, key
                )),
            }
        }
        JsonPathSegment::Index(index) => {
            if current.is_null() {
                *current = Value::Array(Vec::new());
            }

            match current {
                Value::Array(items) => {
                    while items.len() <= *index {
                        items.push(Value::Null);
                    }

                    if segments.len() == 1 {
                        items[*index] = new_value;
                        Ok(())
                    } else {
                        if items[*index].is_null() {
                            items[*index] = json_container_for(&segments[1]);
                        }
                        set_json_path_value(
                            &mut items[*index],
                            &segments[1..],
                            new_value,
                            full_path,
                        )
                    }
                }
                _ => Err(format!(
                    "超参数路径 '{}' 在索引 '{}' 之前命中了非数组节点",
                    full_path, index
                )),
            }
        }
    }
}

fn remove_json_path_value(
    current: &mut Value,
    segments: &[JsonPathSegment],
    full_path: &str,
) -> Result<Value, String> {
    if segments.is_empty() {
        return Err("JSON 路径不能为空".to_string());
    }

    match &segments[0] {
        JsonPathSegment::Key(key) => match current {
            Value::Object(map) => {
                if segments.len() == 1 {
                    map.remove(key)
                        .ok_or_else(|| format!("未找到超参数路径: {}", full_path))
                } else {
                    let next = map
                        .get_mut(key)
                        .ok_or_else(|| format!("未找到超参数路径: {}", full_path))?;
                    remove_json_path_value(next, &segments[1..], full_path)
                }
            }
            _ => Err(format!(
                "超参数路径 '{}' 在键 '{}' 之前命中了非对象节点",
                full_path, key
            )),
        },
        JsonPathSegment::Index(index) => match current {
            Value::Array(items) => {
                if *index >= items.len() {
                    return Err(format!("未找到超参数路径: {}", full_path));
                }

                if segments.len() == 1 {
                    Ok(items.remove(*index))
                } else {
                    remove_json_path_value(&mut items[*index], &segments[1..], full_path)
                }
            }
            _ => Err(format!(
                "超参数路径 '{}' 在索引 '{}' 之前命中了非数组节点",
                full_path, index
            )),
        },
    }
}

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
                AppType::OpenCode => {
                    let options = config.get_mut("options").and_then(|v| v.as_object_mut());
                    if let Some(options) = options {
                        options.insert("apiKey".to_string(), json!(api_key));
                    } else {
                        config["options"] = json!({
                            "apiKey": api_key
                        });
                    }
                }
                AppType::OpenClaw => {
                    config["apiKey"] = json!(api_key);
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
                AppType::OpenCode => {
                    let options = config.get_mut("options").and_then(|v| v.as_object_mut());
                    if let Some(options) = options {
                        options.insert("baseURL".to_string(), json!(base_url));
                    } else {
                        config["options"] = json!({
                            "baseURL": base_url
                        });
                    }
                }
                AppType::OpenClaw => {
                    config["baseUrl"] = json!(base_url);
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
    output::hint("提示: 使用 'ccs provider switch' 切换到此供应商");

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
    if weight > 100 {
        return Err("权重必须在0-100范围内".to_string());
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

pub async fn provider_hyperparams_show(
    app: &str,
    id: &str,
    path: Option<&str>,
) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    output::section(&format!("供应商超参数: {}", provider.name));
    output::key_value(vec![
        ("应用", app.to_string()),
        ("供应商ID", provider.id.clone()),
        (
            "路径",
            path.map(str::to_string)
                .unwrap_or_else(|| "settings_config".to_string()),
        ),
    ]);

    println!();

    if let Some(path) = path {
        let segments = parse_json_path(path)?;
        let value = get_json_path_value(&provider.settings_config, &segments, path)?;
        output::json(value);
    } else {
        output::json(&provider.settings_config);
    }

    Ok(())
}

pub async fn provider_hyperparams_set(
    app: &str,
    id: &str,
    path: &str,
    json_value: Option<&str>,
    string_value: Option<&str>,
) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    let segments = parse_json_path(path)?;
    let new_value = match (json_value, string_value) {
        (Some(raw), None) => {
            serde_json::from_str(raw).map_err(|e| format!("超参数 JSON 解析失败: {}", e))?
        }
        (None, Some(raw)) => Value::String(raw.to_string()),
        _ => {
            return Err("请使用 --json 或 --value 之一设置超参数".to_string());
        }
    };

    set_json_path_value(&mut provider.settings_config, &segments, new_value, path)?;

    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    let saved_value = get_json_path_value(&provider.settings_config, &segments, path)?;

    output::success(&format!(
        "供应商 '{}' 超参数已更新: {}",
        provider.name, path
    ));
    output::json(saved_value);

    Ok(())
}

pub async fn provider_hyperparams_remove(app: &str, id: &str, path: &str) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    let segments = parse_json_path(path)?;
    let removed = remove_json_path_value(&mut provider.settings_config, &segments, path)?;

    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    output::success(&format!(
        "供应商 '{}' 超参数已删除: {}",
        provider.name, path
    ));
    output::json(&removed);

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

pub async fn provider_export(
    app: &str,
    output_path: &str,
    id: Option<&str>,
    redact: bool,
) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let current_id = db
        .get_current_provider(app_type.as_str())
        .map_err(|e| e.to_string())?;

    let mut providers: Vec<Provider> = if let Some(provider_id) = id {
        let p = db
            .get_provider_by_id(provider_id, app_type.as_str())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("供应商不存在: {}", provider_id))?;
        vec![p]
    } else {
        db.get_all_providers(app_type.as_str())
            .map_err(|e| e.to_string())?
            .into_values()
            .collect()
    };

    if providers.is_empty() {
        output::warning(&format!("没有可导出的供应商（app={}），将导出空列表", app));
    }

    if redact && !providers.is_empty() {
        providers = providers.into_iter().map(redact_provider).collect();
        output::warning("已启用脱敏导出：密钥字段将被替换为 \"***\"（导入后不可直接使用）");
    } else if !redact && !providers.is_empty() {
        output::warning("导出文件将包含密钥/令牌等敏感信息，请妥善保管");
    }

    let mut current = current_id.clone();
    if let Some(ref cid) = current {
        let included = providers.iter().any(|p| &p.id == cid);
        if !included {
            current = None;
        }
    }

    let bundle = ProviderExportBundleV1 {
        version: 1,
        app: app.to_string(),
        exported_at_ms: chrono::Utc::now().timestamp_millis(),
        current,
        providers,
    };

    let content =
        serde_json::to_string_pretty(&bundle).map_err(|e| format!("序列化失败: {}", e))?;

    if output_path == "-" {
        println!("{content}");
        return Ok(());
    }

    let out = std::path::Path::new(output_path);
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}: {}", parent.display(), e))?;
        }
    }

    std::fs::write(out, content).map_err(|e| format!("写入文件失败 {}: {}", out.display(), e))?;

    output::success(&format!(
        "已导出 {} 个 {} 供应商到: {}",
        bundle.providers.len(),
        app,
        out.display()
    ));
    Ok(())
}

pub async fn provider_import(
    app: &str,
    input_path: &str,
    overwrite: bool,
    new_ids: bool,
    set_current: bool,
) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    let mut content = String::new();
    if input_path == "-" {
        std::io::stdin()
            .read_to_string(&mut content)
            .map_err(|e| format!("读取 stdin 失败: {}", e))?;
    } else {
        content = std::fs::read_to_string(input_path)
            .map_err(|e| format!("读取文件失败 {}: {}", input_path, e))?;
    }

    let mut bundle: ProviderExportBundleV1 =
        serde_json::from_str(&content).map_err(|e| format!("解析导入文件失败: {}", e))?;

    if bundle.version != 1 {
        return Err(format!(
            "不支持的导入文件版本: {}（仅支持 version=1）",
            bundle.version
        ));
    }
    if bundle.app != app {
        return Err(format!(
            "导入文件 app 不匹配：文件为 '{}'，命令为 '{}'（请使用正确的 --app）",
            bundle.app, app
        ));
    }

    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;

    for p in bundle.providers.iter_mut() {
        if p.weight > 100 {
            return Err(format!(
                "供应商 '{}' 权重超出范围(0-100): {}",
                p.name, p.weight
            ));
        }

        if new_ids {
            let old = p.id.clone();
            let new_id = uuid::Uuid::new_v4().to_string();
            p.id = new_id.clone();
            id_map.insert(old, new_id);
        }

        let existing = db
            .get_provider_by_id(&p.id, app_type.as_str())
            .map_err(|e| e.to_string())?;

        match existing {
            Some(_existing_provider) => {
                if !overwrite {
                    skipped += 1;
                    continue;
                }
                db.save_provider(app_type.as_str(), p)
                    .map_err(|e| e.to_string())?;

                // endpoints 只做“增量合并”，避免重复与破坏性删除
                if let Some(import_meta) = &p.meta {
                    let existed_urls = db
                        .list_custom_endpoint_urls(app_type.as_str(), &p.id)
                        .unwrap_or_default();
                    for (url, _) in import_meta.custom_endpoints.iter() {
                        if existed_urls.contains(url) {
                            continue;
                        }
                        let _ = db.add_custom_endpoint(app_type.as_str(), &p.id, url);
                    }
                }

                updated += 1;
            }
            None => {
                db.save_provider(app_type.as_str(), p)
                    .map_err(|e| e.to_string())?;
                added += 1;
            }
        }
    }

    if set_current {
        if let Some(cur) = bundle.current.take() {
            let target = if new_ids {
                id_map.get(&cur).cloned()
            } else {
                Some(cur)
            };
            if let Some(id) = target {
                let existed = db
                    .get_provider_by_id(&id, app_type.as_str())
                    .map_err(|e| e.to_string())?
                    .is_some();
                if existed {
                    db.set_current_provider(app_type.as_str(), &id)
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }

    output::success(&format!(
        "导入完成（app={}）：新增 {}，更新 {}，跳过 {}",
        app, added, updated, skipped
    ));
    Ok(())
}

pub async fn provider_update(
    app: &str,
    id: &str,
    file: Option<&str>,
    key: Option<String>,
    url: Option<String>,
    replace: bool,
    name: Option<String>,
    notes: Option<String>,
) -> Result<(), String> {
    let db = get_database()?;
    let app_type = parse_app_type(app)?;

    if file.is_none() && key.is_none() && url.is_none() && name.is_none() && notes.is_none() {
        return Err(
            "未提供任何更新内容：请使用 --file/--key/--url/--name/--notes 之一".to_string(),
        );
    }

    let mut provider = db
        .get_provider_by_id(id, app_type.as_str())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("供应商不存在: {}", id))?;

    // 1) 文件配置：merge 或 replace settings_config
    if let Some(path) = file {
        let mut content = String::new();
        if path == "-" {
            std::io::stdin()
                .read_to_string(&mut content)
                .map_err(|e| format!("读取 stdin 失败: {}", e))?;
        } else {
            content = std::fs::read_to_string(path)
                .map_err(|e| format!("读取文件失败 {}: {}", path, e))?;
        }

        let incoming: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("解析配置文件失败: {}", e))?;

        if replace {
            provider.settings_config = incoming;
        } else {
            deep_merge_json(&mut provider.settings_config, incoming);
        }
    }

    // 2) key/url：按 app 类型写入 settings_config（覆盖写入）
    if let Some(api_key) = key {
        match app_type {
            AppType::Claude => {
                let env = provider
                    .settings_config
                    .as_object_mut()
                    .and_then(|o| o.get_mut("env"))
                    .and_then(|v| v.as_object_mut());
                if let Some(env) = env {
                    env.insert("ANTHROPIC_API_KEY".to_string(), json!(api_key));
                } else {
                    provider.settings_config["env"] = json!({ "ANTHROPIC_API_KEY": api_key });
                }
            }
            AppType::Codex => {
                let env = provider
                    .settings_config
                    .as_object_mut()
                    .and_then(|o| o.get_mut("env"))
                    .and_then(|v| v.as_object_mut());
                if let Some(env) = env {
                    env.insert("OPENAI_API_KEY".to_string(), json!(api_key));
                } else {
                    provider.settings_config["env"] = json!({ "OPENAI_API_KEY": api_key });
                }
            }
            AppType::Gemini => {
                let env = provider
                    .settings_config
                    .as_object_mut()
                    .and_then(|o| o.get_mut("env"))
                    .and_then(|v| v.as_object_mut());
                if let Some(env) = env {
                    env.insert("GEMINI_API_KEY".to_string(), json!(api_key));
                } else {
                    provider.settings_config["env"] = json!({ "GEMINI_API_KEY": api_key });
                }
            }
            AppType::OpenCode => {
                let options = provider
                    .settings_config
                    .as_object_mut()
                    .and_then(|o| o.get_mut("options"))
                    .and_then(|v| v.as_object_mut());
                if let Some(options) = options {
                    options.insert("apiKey".to_string(), json!(api_key));
                } else {
                    provider.settings_config["options"] = json!({ "apiKey": api_key });
                }
            }
            AppType::OpenClaw => {
                provider.settings_config["apiKey"] = json!(api_key);
            }
        }
    }

    if let Some(base_url) = url {
        match app_type {
            AppType::Claude => {
                let env = provider
                    .settings_config
                    .as_object_mut()
                    .and_then(|o| o.get_mut("env"))
                    .and_then(|v| v.as_object_mut());
                if let Some(env) = env {
                    env.insert("ANTHROPIC_BASE_URL".to_string(), json!(base_url));
                } else {
                    provider.settings_config["env"] = json!({ "ANTHROPIC_BASE_URL": base_url });
                }
            }
            AppType::Codex => {
                provider.settings_config["base_url"] = json!(base_url);
            }
            AppType::Gemini => {
                let env = provider
                    .settings_config
                    .as_object_mut()
                    .and_then(|o| o.get_mut("env"))
                    .and_then(|v| v.as_object_mut());
                if let Some(env) = env {
                    env.insert("GEMINI_API_BASE_URL".to_string(), json!(base_url));
                } else {
                    provider.settings_config["env"] = json!({ "GEMINI_API_BASE_URL": base_url });
                }
            }
            AppType::OpenCode => {
                let options = provider
                    .settings_config
                    .as_object_mut()
                    .and_then(|o| o.get_mut("options"))
                    .and_then(|v| v.as_object_mut());
                if let Some(options) = options {
                    options.insert("baseURL".to_string(), json!(base_url));
                } else {
                    provider.settings_config["options"] = json!({ "baseURL": base_url });
                }
            }
            AppType::OpenClaw => {
                provider.settings_config["baseUrl"] = json!(base_url);
            }
        }
    }

    // 3) name/notes
    if let Some(new_name) = name {
        provider.name = new_name;
    }
    if notes.is_some() {
        provider.notes = notes;
    }

    db.save_provider(app_type.as_str(), &provider)
        .map_err(|e| e.to_string())?;

    output::success(&format!(
        "供应商 '{}' 已更新 (ID: {})",
        provider.name, provider.id
    ));
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
            output::hint("使用 'ccs provider weight' 设置供应商权重");
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
            "使用 'ccs config lb --app {} --enabled true' 启用权重轮询",
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
        output::hint("使用 'ccs failover add' 添加供应商到队列");
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

#[cfg(test)]
mod tests {
    use super::{
        get_json_path_value, parse_json_path, remove_json_path_value, set_json_path_value,
        JsonPathSegment,
    };
    use serde_json::json;

    #[test]
    fn parse_json_path_supports_keys_and_indices() {
        let parsed = parse_json_path("agents.sisyphus.tools.0.name").expect("path should parse");
        assert_eq!(
            parsed,
            vec![
                JsonPathSegment::Key("agents".to_string()),
                JsonPathSegment::Key("sisyphus".to_string()),
                JsonPathSegment::Key("tools".to_string()),
                JsonPathSegment::Index(0),
                JsonPathSegment::Key("name".to_string()),
            ]
        );
    }

    #[test]
    fn set_and_get_json_path_value_creates_missing_objects() {
        let mut value = json!({});
        let path = parse_json_path("agents.sisyphus.temperature").expect("path should parse");

        set_json_path_value(&mut value, &path, json!(0.5), "agents.sisyphus.temperature")
            .expect("path should be set");

        let actual = get_json_path_value(&value, &path, "agents.sisyphus.temperature")
            .expect("path should exist");
        assert_eq!(actual, &json!(0.5));
        assert_eq!(value["agents"]["sisyphus"]["temperature"], json!(0.5));
    }

    #[test]
    fn set_json_path_value_supports_arrays() {
        let mut value = json!({});
        let path = parse_json_path("agents.sisyphus.tools.1.name").expect("path should parse");

        set_json_path_value(
            &mut value,
            &path,
            json!("shell"),
            "agents.sisyphus.tools.1.name",
        )
        .expect("array path should be set");

        assert_eq!(
            value["agents"]["sisyphus"]["tools"][1]["name"],
            json!("shell")
        );
    }

    #[test]
    fn remove_json_path_value_removes_nested_value() {
        let mut value = json!({
            "agents": {
                "sisyphus": {
                    "permission": {
                        "bash": "ask"
                    }
                }
            }
        });
        let path = parse_json_path("agents.sisyphus.permission.bash").expect("path should parse");

        let removed = remove_json_path_value(&mut value, &path, "agents.sisyphus.permission.bash")
            .expect("path should be removed");

        assert_eq!(removed, json!("ask"));
        assert!(value["agents"]["sisyphus"]["permission"]
            .get("bash")
            .is_none());
    }
}
