//! 无头代理服务器实现

use crate::store::AppState;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::signal;
use tokio::time::{sleep, timeout, Duration, Instant};

/// 启动无头代理服务器
pub async fn start_headless_server(host: String, port: u16, daemon: bool) -> Result<(), String> {
    // 默认后台启动：父进程负责拉起子进程并退出，子进程在后台常驻。
    if daemon && !is_daemon_child() {
        return start_headless_server_daemon(host, port).await;
    }

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

    // 将 CLI 传入的 host/port 写入数据库配置，确保本次启动使用该监听地址。
    if let Ok(mut cfg) = app_state.db.get_proxy_config().await {
        cfg.listen_address = host.clone();
        cfg.listen_port = port;
        if let Err(e) = app_state.db.update_proxy_config(cfg).await {
            crate::cli::output::warning(&format!("更新代理监听地址失败（将使用已有配置）: {e}"));
        }
    }

    // 初始化全局HTTP客户端
    {
        let proxy_state = app_state
            .db
            .get_global_proxy_state()
            .unwrap_or(crate::database::GlobalProxyState::Unset);

        let init_arg: Option<String> = match proxy_state {
            crate::database::GlobalProxyState::Unset => None,
            crate::database::GlobalProxyState::Direct => Some(String::new()),
            crate::database::GlobalProxyState::Proxy(url) => Some(url),
        };

        if let Err(e) = crate::proxy::http_client::init(init_arg.as_deref()) {
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
                // 后台子进程：等待 SIGTERM/SIGINT，便于 stop/restart 时优雅退出并清理 PID。
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal as unix_signal, SignalKind};

                    let mut sigterm =
                        unix_signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
                    let mut sigint =
                        unix_signal(SignalKind::interrupt()).map_err(|e| e.to_string())?;

                    tokio::select! {
                        _ = sigterm.recv() => {},
                        _ = sigint.recv() => {},
                    }
                }

                #[cfg(not(unix))]
                {
                    // 非 unix 平台：退化为 Ctrl+C
                    signal::ctrl_c().await.map_err(|e| e.to_string())?;
                }

                app_state
                    .proxy_service
                    .stop()
                    .await
                    .map_err(|e| e.to_string())?;
                remove_pid_file()?;
            } else {
                crate::cli::output::info("按 Ctrl+C 停止服务器");
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
            // 等待进程真正退出，避免“端口仍被占用但 PID 文件已删除”的情况
            for _ in 0..50 {
                if !check_process_running(pid) {
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }

            // 仍未退出则强制杀死
            if check_process_running(pid) {
                let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
            }

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

fn is_daemon_child() -> bool {
    std::env::var("CC_SWITCH_DAEMON_CHILD")
        .map(|v| v == "1")
        .unwrap_or(false)
}

async fn start_headless_server_daemon(host: String, port: u16) -> Result<(), String> {
    // 已在运行则直接返回状态
    let pid_file_path = get_pid_file_path();
    if pid_file_path.exists() {
        if let Ok(pid) = read_pid_file() {
            if check_process_running(pid) {
                crate::cli::output::success("代理服务器已在运行");
                server_status().await?;
                return Ok(());
            }
        }
        // PID 文件存在但进程不在：清理后再启动
        let _ = remove_pid_file();
    }

    let log_path = get_config_dir().join("logs").join("server.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("打开日志文件失败: {} ({})", log_path.display(), e))?;
    let log_file_err = log_file
        .try_clone()
        .map_err(|e| format!("打开日志文件失败: {}", e))?;

    let exe = std::env::current_exe().map_err(|e| format!("获取可执行文件路径失败: {}", e))?;

    let mut cmd = Command::new(exe);
    cmd.args([
        "server",
        "start",
        "--host",
        &host,
        "--port",
        &port.to_string(),
    ])
    .env("CC_SWITCH_DAEMON_CHILD", "1")
    .stdin(Stdio::null())
    .stdout(Stdio::from(log_file))
    .stderr(Stdio::from(log_file_err));

    // 在 unix 上创建新 session，避免终端关闭导致 SIGHUP 终止后台服务
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // setsid 失败也不算致命：至少能后台运行
                let _ = libc::setsid();
                // 忽略 SIGHUP（保险起见）
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                Ok(())
            });
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动后台进程失败: {}", e))?;

    let start = Instant::now();
    let deadline = Duration::from_secs(12);

    // 等待端口可用或子进程退出
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "启动失败（子进程已退出，退出码: {:?}），请查看日志: {}",
                status.code(),
                log_path.display()
            ));
        }

        let addr = format!("{}:{}", host, port);
        if timeout(Duration::from_millis(300), tokio::net::TcpStream::connect(&addr))
            .await
            .is_ok()
        {
            crate::cli::output::success("代理服务器启动成功（后台运行）");
            server_status().await?;
            crate::cli::output::hint("使用 'csc server stop' 停止服务");
            return Ok(());
        }

        if start.elapsed() > deadline {
            return Err(format!(
                "启动超时，请查看日志: {}",
                log_path.display()
            ));
        }

        sleep(Duration::from_millis(200)).await;
    }
}

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
