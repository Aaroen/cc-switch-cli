//! 无头代理服务器实现

use crate::store::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::signal;

/// 启动无头代理服务器
pub async fn start_headless_server(host: String, port: u16, daemon: bool) -> Result<(), String> {
    crate::cli::output::info(&format!("正在启动代理服务器 {}:{}...", host, port));

    // 初始化数据库（使用默认路径）
    let db = match crate::database::Database::init() {
        Ok(db) => Arc::new(db),
        Err(e) => {
            crate::cli::output::error(&format!("数据库初始化失败: {}", e));
            return Err(e.to_string());
        }
    };

    // 创建AppState
    let app_state = Arc::new(AppState::new(db));

    // 初始化全局HTTP客户端
    {
        let proxy_url = app_state.db.get_global_proxy_url().ok().flatten();
        if let Err(e) = crate::proxy::http_client::init(proxy_url.as_deref()) {
            crate::cli::output::warning(&format!("HTTP客户端初始化失败: {}", e));
        }
    }

    // 启动代理服务（不接受参数，使用数据库配置）
    match app_state.proxy_service.start().await {
        Ok(info) => {
            crate::cli::output::success(&format!(
                "代理服务器已启动: {}:{}",
                info.address, info.port
            ));

            // 保存PID文件
            save_pid_file().map_err(|e| format!("保存PID失败: {}", e))?;

            if daemon {
                crate::cli::output::info("以后台模式运行");
                // 注意：真正的后台化需要使用daemonize crate
                // 这里简单实现，用户需要手动使用 & 或 nohup
            } else {
                crate::cli::output::info("按 Ctrl+C 停止服务器");
                // 等待Ctrl+C信号
                signal::ctrl_c().await.map_err(|e| e.to_string())?;
                crate::cli::output::info("\n正在停止服务器...");
                app_state
                    .proxy_service
                    .stop()
                    .await
                    .map_err(|e| e.to_string())?;
                remove_pid_file()?;
                crate::cli::output::success("服务器已停止");
            }

            Ok(())
        }
        Err(e) => {
            crate::cli::output::error(&format!("启动失败: {}", e));
            Err(e)
        }
    }
}

/// 停止代理服务器
pub async fn stop_server() -> Result<(), String> {
    crate::cli::output::info("正在停止代理服务器...");

    // 读取PID文件
    let pid = read_pid_file()?;

    // 尝试终止进程
    #[cfg(unix)]
    {
        use std::process::Command;
        let output = Command::new("kill")
            .arg(pid.to_string())
            .output()
            .map_err(|e| format!("终止进程失败: {}", e))?;

        if output.status.success() {
            remove_pid_file()?;
            crate::cli::output::success("服务器已停止");
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&output.stderr);
            Err(format!("终止进程失败: {}", err))
        }
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        let output = Command::new("taskkill")
            .args(&["/PID", &pid.to_string(), "/F"])
            .output()
            .map_err(|e| format!("终止进程失败: {}", e))?;

        if output.status.success() {
            remove_pid_file()?;
            crate::cli::output::success("服务器已停止");
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&output.stderr);
            Err(format!("终止进程失败: {}", err))
        }
    }
}

/// 查看服务器状态
pub async fn server_status() -> Result<(), String> {
    let pid_file_path = get_pid_file_path();

    if !pid_file_path.exists() {
        crate::cli::output::info("服务器未运行");
        return Ok(());
    }

    let pid = read_pid_file()?;

    // 检查进程是否存在
    let running = check_process_running(pid);

    if running {
        crate::cli::output::service_status("代理服务器", true, Some(pid));

        // 尝试读取代理配置
        if let Ok(db) = crate::database::Database::init() {
            if let Ok(config) = db.get_global_proxy_config().await {
                crate::cli::output::key_value(vec![
                    (
                        "监听地址",
                        format!("{}:{}", config.listen_address, config.listen_port),
                    ),
                    ("启用应用", format!("{:?}", get_enabled_apps(&db).await)),
                ]);
            }
        }
    } else {
        crate::cli::output::service_status("代理服务器", false, None);
        crate::cli::output::warning("PID文件存在但进程未运行（可能异常退出）");
        remove_pid_file()?;
    }

    Ok(())
}

/// 重启服务器
pub async fn restart_server(port: u16) -> Result<(), String> {
    crate::cli::output::info("正在重启服务器...");

    // 先停止
    if get_pid_file_path().exists() {
        stop_server().await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    // 再启动
    start_headless_server("127.0.0.1".to_string(), port, true).await
}

// ============================================================================
// 辅助函数
// ============================================================================

fn get_config_dir() -> PathBuf {
    let config_dir = dirs::home_dir()
        .expect("无法获取用户主目录")
        .join(".cc-switch");

    std::fs::create_dir_all(&config_dir).ok();
    config_dir
}

fn get_db_path() -> PathBuf {
    get_config_dir().join("cc-switch.db")
}

fn get_pid_file_path() -> PathBuf {
    get_config_dir().join("server.pid")
}

fn save_pid_file() -> Result<(), String> {
    let pid = std::process::id();
    let pid_file = get_pid_file_path();

    std::fs::write(&pid_file, pid.to_string()).map_err(|e| format!("写入PID文件失败: {}", e))?;

    Ok(())
}

fn read_pid_file() -> Result<u32, String> {
    let pid_file = get_pid_file_path();

    if !pid_file.exists() {
        return Err("服务器未运行（PID文件不存在）".to_string());
    }

    let content =
        std::fs::read_to_string(&pid_file).map_err(|e| format!("读取PID文件失败: {}", e))?;

    content
        .trim()
        .parse::<u32>()
        .map_err(|e| format!("解析PID失败: {}", e))
}

fn remove_pid_file() -> Result<(), String> {
    let pid_file = get_pid_file_path();

    if pid_file.exists() {
        std::fs::remove_file(&pid_file).map_err(|e| format!("删除PID文件失败: {}", e))?;
    }

    Ok(())
}

#[cfg(unix)]
fn check_process_running(pid: u32) -> bool {
    use std::process::Command;

    Command::new("kill")
        .args(&["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn check_process_running(pid: u32) -> bool {
    use std::process::Command;

    Command::new("tasklist")
        .args(&["/FI", &format!("PID eq {}", pid)])
        .output()
        .ok()
        .and_then(|output| {
            String::from_utf8(output.stdout)
                .ok()
                .map(|s| s.contains(&pid.to_string()))
        })
        .unwrap_or(false)
}

async fn get_enabled_apps(db: &crate::database::Database) -> Vec<String> {
    let mut apps = Vec::new();

    for app in ["claude", "codex", "gemini"] {
        if let Ok(config) = db.get_proxy_config_for_app(app).await {
            if config.enabled {
                apps.push(app.to_string());
            }
        }
    }

    apps
}
