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

    // 防御性：确保 rustls CryptoProvider 已安装。run_from_env 已在入口安装，此处幂等兜底，
    // 避免未来重构改变入口调度时上游 HTTPS 连接因缺省 provider 而 panic。
    crate::ensure_rustls_crypto_provider();

    // 无头模式先以临时级别（info）安装 log facade 后端：否则 log::warn!（含 USG-001 用量
    // 写入失败）、log::info! 等会被静默丢弃，守护进程下无法诊断。提前到 Database::init() 之前
    // 安装，确保数据库迁移/启动日志亦可见；待 DB 就绪后再按 LogConfig 调整级别。
    crate::cli::headless_log::init(log::LevelFilter::Info);

    crate::cli::output::info(&format!("正在启动代理服务器 {}:{}...", host, port));

    // 初始化数据库（使用默认路径）
    let db = match crate::database::Database::init() {
        Ok(db) => Arc::new(db),
        Err(e) => {
            crate::cli::output::error(&format!("数据库初始化失败: {}", e));
            return Err(e.to_string());
        }
    };

    // 按 DB LogConfig 调整日志级别（headless_log::init 幂等：已安装后端时仅 set_max_level）。
    if let Ok(cfg) = db.get_log_config() {
        crate::cli::headless_log::init(cfg.to_level_filter());
    }

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

            // Session 日志用量同步：启动同步一次，之后每 60 秒（与桌面端 setup 对齐）。
            // CLI/headless 此前缺失该任务，导致 session 来源（claude/codex/gemini 本地日志）
            // 用量在纯命令行部署下不回填，仪表盘缺少该维度数据。
            {
                let db_for_session_sync = app_state.db.clone();
                tokio::spawn(async move {
                    const SESSION_SYNC_INTERVAL_SECS: u64 = 60;

                    fn run_step<T>(name: &str, result: Result<T, crate::error::AppError>) {
                        if let Err(e) = result {
                            log::warn!("{name} failed: {e}");
                        }
                    }

                    let db = &db_for_session_sync;

                    run_step(
                        "Usage cost startup backfill",
                        db.backfill_missing_usage_costs(),
                    );
                    run_step(
                        "Session usage initial sync",
                        crate::services::session_usage::sync_claude_session_logs(db),
                    );
                    run_step(
                        "Codex usage initial sync",
                        crate::services::session_usage_codex::sync_codex_usage(db),
                    );
                    run_step(
                        "Gemini usage initial sync",
                        crate::services::session_usage_gemini::sync_gemini_usage(db),
                    );

                    let mut interval = tokio::time::interval(Duration::from_secs(
                        SESSION_SYNC_INTERVAL_SECS,
                    ));
                    interval.tick().await; // 跳过立即触发的首个 tick（启动同步已完成）
                    loop {
                        interval.tick().await;
                        run_step(
                            "Session usage periodic sync",
                            crate::services::session_usage::sync_claude_session_logs(db),
                        );
                        run_step(
                            "Codex usage periodic sync",
                            crate::services::session_usage_codex::sync_codex_usage(db),
                        );
                        run_step(
                            "Gemini usage periodic sync",
                            crate::services::session_usage_gemini::sync_gemini_usage(db),
                        );
                    }
                });
            }

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
                let mut info = vec![
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
                ];

                // 添加 web 控制台信息
                if let Ok(Some(web_port)) = db.get_web_panel_port() {
                    let web_bind = db.get_web_panel_bind().ok().flatten().unwrap_or_else(|| "0.0.0.0".to_string());

                    // 本机访问地址
                    info.push(("✓ 本机访问", format!("http://127.0.0.1:{}", web_port)));

                    // 局域网访问地址（仅当 web_bind 为 0.0.0.0 时显示）
                    if web_bind == "0.0.0.0" {
                        if let Some(local_ip) = local_ipaddress::get() {
                            info.push(("✓ 局域网访问", format!("http://{}:{}", local_ip, web_port)));
                        }
                    }
                }

                crate::cli::output::key_value(info);
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

    // 从数据库读取实际配置的端口（优先于命令行参数）
    let (actual_port, web_port, web_bind) = match crate::database::Database::init() {
        Ok(db) => {
            // 直接从数据库查询 listen_port
            let listen_port = db
                .conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT listen_port FROM proxy_config WHERE app_type='claude' LIMIT 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .ok()
                .map(|p| p.clamp(1024, 65535) as u16);

            let web_port = db.get_web_panel_port().ok().flatten();
            let web_bind = db
                .get_web_panel_bind()
                .ok()
                .flatten()
                .unwrap_or_else(|| "0.0.0.0".to_string());

            (listen_port, web_port, web_bind)
        }
        Err(_) => (None, None, "0.0.0.0".to_string()),
    };

    // 使用数据库中的端口，如果数据库读取失败则使用命令行参数
    let final_port = actual_port.unwrap_or(port);

    start_headless_server("127.0.0.1".to_string(), final_port, true, web_port, web_bind).await
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

    let log_path = get_config_dir().join("logs").join("summary.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // 文件内轮转：summary.log 超过 5MB 时，保留最后 4MB 内容，删除开头旧数据
    const MAX_LOG_SIZE_BYTES: u64 = 5 * 1024 * 1024; // 5MB
    const KEEP_BYTES: u64 = 4 * 1024 * 1024; // 保留 4MB
    if let Ok(metadata) = std::fs::metadata(&log_path) {
        if metadata.len() >= MAX_LOG_SIZE_BYTES {
            use std::io::{Read, Seek, SeekFrom, Write};
            if let Ok(mut file) = std::fs::OpenOptions::new().read(true).write(true).open(&log_path) {
                // 定位到末尾前 KEEP_BYTES 位置
                if file.seek(SeekFrom::End(-(KEEP_BYTES as i64))).is_ok() {
                    let mut tail_content = Vec::new();
                    if file.read_to_end(&mut tail_content).is_ok() {
                        // 截断文件并写回保留的内容
                        let _ = file.set_len(0);
                        let _ = file.seek(SeekFrom::Start(0));
                        let _ = std::io::Write::write_all(&mut file, &tail_content);
                        let _ = file.flush();
                    }
                }
            }
        }
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

    // 解析有效 Web 配置（CLI 端口优先；未显式指定端口时继承持久化绑定地址）
    let persisted_web = crate::database::Database::init().ok().map(|db| {
        (
            db.get_web_panel_port().ok().flatten(),
            db.get_web_panel_bind().ok().flatten(),
        )
    });
    let eff_web_port = web_port.or_else(|| persisted_web.as_ref().and_then(|(port, _)| *port));
    let eff_web_bind = if web_port.is_some() {
        web_bind.clone()
    } else {
        persisted_web
            .and_then(|(_, bind)| bind)
            .unwrap_or_else(|| web_bind.clone())
    };

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
        cmd_args.push(eff_web_bind.clone());
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
                // 等待子进程完成 Web 面板监听，再向用户终端打印访问地址。
                let probe_addr = panel_probe_addr(&eff_web_bind, wp);
                let mut panel_ready = false;
                for _ in 0..25 {
                    if timeout(
                        Duration::from_millis(300),
                        tokio::net::TcpStream::connect(&probe_addr),
                    )
                    .await
                    .is_ok()
                    {
                        panel_ready = true;
                        break;
                    }
                    sleep(Duration::from_millis(80)).await;
                }
                if panel_ready {
                    print_web_panel_urls(&eff_web_bind, wp);
                    crate::cli::output::hint("首次访问 Web 控制台时请设置访问密码");
                } else {
                    crate::cli::output::warning(&format!(
                        "Web 控制台尚未在预期时间内响应，请查看日志: {}",
                        log_path.display()
                    ));
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
    let eff_bind = if web_port.is_some() {
        web_bind
    } else {
        app_state
            .db
            .get_web_panel_bind()
            .ok()
            .flatten()
            .unwrap_or(web_bind)
    };

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
        let _ = app_state.db.set_web_panel_bind(&eff_bind);
    }

    let mut server =
        crate::web_panel::WebPanelServer::new(app_state.clone(), eff_bind.clone(), eff_port);
    match server.start().await {
        Ok(()) => {
            if daemon {
                crate::cli::output::info("Web 控制台已启动");
            } else {
                print_web_panel_urls(&eff_bind, eff_port);
                crate::cli::output::hint("首次访问 Web 控制台时请设置访问密码");
            }
            Some(server)
        }
        Err(e) => {
            crate::cli::output::warning(&format!("Web 控制台启动失败（不影响代理）: {e}"));
            None
        }
    }
}

fn panel_probe_addr(bind: &str, port: u16) -> String {
    let host = match bind.trim() {
        "0.0.0.0" | "::" => "127.0.0.1",
        other => other,
    };
    format_socket_addr(host, port)
}

fn format_socket_addr(host: &str, port: u16) -> String {
    let host = host.trim();
    if host.starts_with('[') || !host.contains(':') {
        format!("{host}:{port}")
    } else {
        format!("[{host}]:{port}")
    }
}

fn format_url_host(host: &str) -> String {
    let host = host.trim();
    if host.starts_with('[') || !host.contains(':') {
        host.to_string()
    } else {
        format!("[{host}]")
    }
}

fn detect_lan_ip() -> Option<String> {
    detect_lan_ip_from_interfaces().or_else(detect_lan_ip_from_route)
}

#[cfg(unix)]
fn detect_lan_ip_from_interfaces() -> Option<String> {
    let output = Command::new("ip")
        .args(["-4", "addr", "show", "scope", "global"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| {
            let addr = line
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .find_map(|pair| (pair[0] == "inet").then_some(pair[1]))?;
            let ip = addr.split('/').next()?.to_string();
            lan_ip_rank(&ip).map(|rank| (rank, ip))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, ip)| ip)
}

#[cfg(not(unix))]
fn detect_lan_ip_from_interfaces() -> Option<String> {
    None
}

fn detect_lan_ip_from_route() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    let ip = ip.to_string();
    lan_ip_rank(&ip).map(|_| ip)
}

fn lan_ip_rank(ip: &str) -> Option<u8> {
    let ip: std::net::Ipv4Addr = ip.parse().ok()?;
    let octets = ip.octets();
    if ip.is_loopback() || ip.is_link_local() || ip.is_multicast() || ip.is_unspecified() {
        return None;
    }
    match octets {
        [192, 168, _, _] => Some(0),
        [10, _, _, _] => Some(1),
        [172, b, _, _] if (16..=31).contains(&b) => Some(2),
        [100, b, _, _] if (64..=127).contains(&b) => Some(3),
        [198, b, _, _] if b == 18 || b == 19 => None,
        _ => Some(4),
    }
}

fn web_panel_urls(bind: &str, port: u16) -> Vec<(&'static str, String)> {
    match bind.trim() {
        "0.0.0.0" | "::" => {
            let mut urls = vec![("本机访问", format!("http://127.0.0.1:{port}"))];
            let lan_url = detect_lan_ip()
                .map(|ip| format!("http://{}:{port}", format_url_host(&ip)))
                .unwrap_or_else(|| format!("http://<本机局域网IP>:{port}"));
            urls.push(("局域网访问", lan_url));
            urls
        }
        host => vec![(
            "Web 控制台",
            format!("http://{}:{port}", format_url_host(host)),
        )],
    }
}

fn print_web_panel_urls(bind: &str, port: u16) {
    for (label, url) in web_panel_urls(bind, port) {
        crate::cli::output::success(&format!("{label}: {url}"));
    }
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
