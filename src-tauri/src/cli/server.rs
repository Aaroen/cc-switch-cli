//! 无头代理服务器实现

use crate::store::AppState;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::signal;
use tokio::time::{sleep, timeout, Duration, Instant};

/// 启动无头代理服务器
pub async fn start_headless_server(
    host: String,
    port: u16,
    daemon: bool,
    web_port: Option<u16>,
    web_bind: String,
) -> Result<(), String> {
    // 默认后台启动：父进程负责拉起子进程并退出，子进程在后台常驻。
    if daemon && !is_daemon_child() {
        return start_headless_server_daemon(host, port, web_port, web_bind).await;
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

    // 默认启用全部 App（claude/codex/gemini）：
    // 若三项均未启用，视为“未初始化”状态，自动开启，避免 status 显示为空。
    if let Err(e) = ensure_default_apps_enabled(&app_state.db).await {
        crate::cli::output::warning(&format!("初始化启用应用失败（可忽略）: {e}"));
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

            // 启动 Web 控制台（如配置）。绑定失败为非致命：记录后继续运行代理。
            let mut panel =
                start_web_panel_if_configured(&app_state, web_port, web_bind, port, daemon).await;

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

                if let Some(p) = panel.as_mut() {
                    p.stop().await;
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
                if let Some(p) = panel.as_mut() {
                    p.stop().await;
                }
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
                    ("启用应用", {
                        let apps = get_enabled_apps(&db).await;
                        if apps.is_empty() {
                            "(无)".to_string()
                        } else {
                            apps.join(", ")
                        }
                    }),
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

    // 再启动（继承已持久化的 Web 控制台端口）
    let web_port = crate::database::Database::init()
        .ok()
        .and_then(|db| db.get_web_panel_port().ok().flatten());
    start_headless_server(
        "127.0.0.1".to_string(),
        port,
        true,
        web_port,
        "127.0.0.1".to_string(),
    )
    .await
}

// ============================================================================
// 辅助函数
// ============================================================================

fn is_daemon_child() -> bool {
    std::env::var("CC_SWITCH_DAEMON_CHILD")
        .map(|v| v == "1")
        .unwrap_or(false)
}

async fn start_headless_server_daemon(
    host: String,
    port: u16,
    web_port: Option<u16>,
    web_bind: String,
) -> Result<(), String> {
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

    // 解析有效 Web 端口（CLI 优先，其次持久化），显式转发给子进程
    let eff_web_port = web_port.or_else(|| {
        crate::database::Database::init()
            .ok()
            .and_then(|db| db.get_web_panel_port().ok().flatten())
    });

    let mut cmd = Command::new(exe);
    let mut cmd_args: Vec<String> = vec![
        "server".to_string(),
        "start".to_string(),
        "--host".to_string(),
        host.clone(),
        "--port".to_string(),
        port.to_string(),
    ];
    if let Some(wp) = eff_web_port {
        cmd_args.push("--web-port".to_string());
        cmd_args.push(wp.to_string());
        cmd_args.push("--web-bind".to_string());
        cmd_args.push(web_bind.clone());
    }
    cmd.args(&cmd_args)
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
    let pid_file = get_pid_file_path();

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
        if timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        .is_ok()
        {
            crate::cli::output::success("代理服务器启动成功（后台运行）");
            // 端口可用不代表 PID 文件已写入（子进程写 PID 在启动逻辑后段），稍等一会儿避免“状态不一致”
            for _ in 0..25 {
                if pid_file.exists() {
                    break;
                }
                sleep(Duration::from_millis(80)).await;
            }

            if pid_file.exists() {
                server_status().await?;
            } else {
                crate::cli::output::warning(
                    "已监听端口，但尚未检测到 PID 文件（稍后可用 'ccs status' 再确认）",
                );
            }
            if let Some(wp) = eff_web_port {
                // 等待子进程写入令牌文件，再向用户终端打印完整访问地址（令牌不入后台日志）
                let mut token = None;
                for _ in 0..25 {
                    if let Some(t) = read_panel_token() {
                        token = Some(t);
                        break;
                    }
                    sleep(Duration::from_millis(80)).await;
                }
                let url = format!("http://{}:{}", web_bind, wp);
                match token {
                    Some(t) => {
                        crate::cli::output::success(&format!("Web 控制台: {url}/?token={t}"))
                    }
                    None => crate::cli::output::warning(&format!(
                        "Web 控制台: {url}（令牌见 {}）",
                        get_panel_token_path().display()
                    )),
                }
            }
            crate::cli::output::hint("使用 'ccs server stop' 停止服务");
            return Ok(());
        }

        if start.elapsed() > deadline {
            return Err(format!("启动超时，请查看日志: {}", log_path.display()));
        }

        sleep(Duration::from_millis(200)).await;
    }
}

/// 按配置启动 Web 控制台（CLI --web-port 优先，其次持久化端口）。
///
/// 返回 Some(server) 表示已启动；绑定失败为非致命（记录警告并返回 None，代理继续运行）。
async fn start_web_panel_if_configured(
    app_state: &Arc<AppState>,
    web_port: Option<u16>,
    web_bind: String,
    proxy_port: u16,
    daemon: bool,
) -> Option<crate::web_panel::WebPanelServer> {
    // 解析有效端口：CLI 指定优先；否则读取持久化端口
    let eff_port = web_port.or_else(|| app_state.db.get_web_panel_port().ok().flatten())?;

    // 端口冲突保护：绝不与代理端口相同
    if eff_port == proxy_port {
        crate::cli::output::warning(&format!(
            "Web 控制台端口 {eff_port} 与代理端口相同，已跳过启动"
        ));
        return None;
    }

    // CLI 显式指定则持久化，供 restart/后台子进程继承
    if let Some(p) = web_port {
        let _ = app_state.db.set_web_panel_port(p);
    }

    // 生成强制 Bearer 令牌并写入 0600 文件
    let token = uuid::Uuid::new_v4().simple().to_string();
    if let Err(e) = write_panel_token(&token) {
        crate::cli::output::warning(&format!("写入面板令牌文件失败: {e}"));
    }

    let mut server = crate::web_panel::WebPanelServer::new(
        app_state.clone(),
        web_bind.clone(),
        eff_port,
        token.clone(),
    );
    match server.start().await {
        Ok(()) => {
            let url = format!("http://{}:{}", web_bind, eff_port);
            if daemon {
                // 后台：stdout 重定向至日志，令牌不入日志；父进程会读 panel.token 打印完整 URL
                crate::cli::output::info(&format!(
                    "Web 控制台已启动: {url}（令牌见 {}）",
                    get_panel_token_path().display()
                ));
            } else {
                crate::cli::output::success(&format!("Web 控制台: {url}/?token={token}"));
                crate::cli::output::hint("在浏览器打开上述链接即可访问（令牌已包含在链接中）");
            }
            Some(server)
        }
        Err(e) => {
            crate::cli::output::warning(&format!("Web 控制台启动失败（不影响代理）: {e}"));
            None
        }
    }
}

fn get_panel_token_path() -> PathBuf {
    get_config_dir().join("panel.token")
}

fn write_panel_token(token: &str) -> Result<(), String> {
    let path = get_panel_token_path();
    std::fs::write(&path, token).map_err(|e| format!("写入失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn read_panel_token() -> Option<String> {
    std::fs::read_to_string(get_panel_token_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn get_config_dir() -> PathBuf {
    let config_dir = dirs::home_dir()
        .expect("无法获取用户主目录")
        .join(".cc-switch");

    std::fs::create_dir_all(&config_dir).ok();
    config_dir
}

#[allow(dead_code)] // 保留：备用 DB 路径解析
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

async fn ensure_default_apps_enabled(db: &crate::database::Database) -> Result<(), String> {
    let mut cfgs = Vec::new();

    for app in ["claude", "codex", "gemini"] {
        let cfg = db
            .get_proxy_config_for_app(app)
            .await
            .map_err(|e| e.to_string())?;
        cfgs.push(cfg);
    }

    if cfgs.iter().any(|c| c.enabled) {
        return Ok(());
    }

    for mut cfg in cfgs {
        cfg.enabled = true;
        db.update_proxy_config_for_app(cfg)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}
