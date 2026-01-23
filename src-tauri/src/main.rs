// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cc_switch_lib::cli;
use clap::Parser;

#[tokio::main]
async fn main() {
    // 检查是否有命令行参数（排除第一个参数即程序名本身）
    let args: Vec<String> = std::env::args().collect();
    let has_cli_args = args.len() > 1;

    if has_cli_args {
        // CLI模式
        run_cli_mode().await;
    } else {
        // GUI模式
        run_gui_mode();
    }
}

/// 运行CLI模式
async fn run_cli_mode() {
    // 解析命令行参数
    let cli = cli::Cli::parse();

    // 执行命令
    let result = match cli.command {
        cli::Commands::Server(cmd) => match cmd {
            cli::ServerCommands::Start { port, host, daemon } => {
                cli::server::start_headless_server(host, port, daemon).await
            }
            cli::ServerCommands::Stop => cli::server::stop_server().await,
            cli::ServerCommands::Status => cli::server::server_status().await,
            cli::ServerCommands::Restart { port } => cli::server::restart_server(port).await,
        },
        cli::Commands::Provider(cmd) => match cmd {
            cli::ProviderCommands::List { app, verbose } => {
                cli::commands::provider_list(&app, verbose).await
            }
            cli::ProviderCommands::Add {
                app,
                name,
                key,
                url,
                file,
            } => cli::commands::provider_add(&app, &name, key, url, file).await,
            cli::ProviderCommands::Remove { app, id } => {
                cli::commands::provider_remove(&app, &id).await
            }
            cli::ProviderCommands::Switch { app, id } => {
                cli::commands::provider_switch(&app, &id).await
            }
            cli::ProviderCommands::Weight { app, id, weight } => {
                cli::commands::provider_set_weight(&app, &id, weight).await
            }
            cli::ProviderCommands::ModelMap { app, id, from, to } => {
                cli::commands::provider_set_model_mapping(&app, &id, &from, &to).await
            }
            cli::ProviderCommands::EnvSet {
                app,
                id,
                key,
                value,
            } => cli::commands::provider_set_env(&app, &id, &key, &value).await,
            cli::ProviderCommands::Show { app, id } => {
                cli::commands::provider_show(&app, &id).await
            }
            cli::ProviderCommands::Test { app, id } => {
                cli::commands::provider_test(&app, &id).await
            }
            cli::ProviderCommands::Export {
                app,
                output,
                id,
                redact,
            } => cli::commands::provider_export(&app, &output, id.as_deref(), redact).await,
            cli::ProviderCommands::Import {
                app,
                input,
                overwrite,
                new_ids,
                set_current,
            } => cli::commands::provider_import(&app, &input, overwrite, new_ids, set_current).await,
            cli::ProviderCommands::Update {
                app,
                id,
                file,
                key,
                url,
                replace,
                name,
                notes,
            } => {
                cli::commands::provider_update(
                    &app,
                    &id,
                    file.as_deref(),
                    key,
                    url,
                    replace,
                    name,
                    notes,
                )
                .await
            }
        },
        cli::Commands::Config(cmd) => match cmd {
            cli::ConfigCommands::Show { app } => cli::commands::config_show(app).await,
            cli::ConfigCommands::Set { key, value, app } => {
                cli::commands::config_set(&key, &value, app).await
            }
            cli::ConfigCommands::Export { output } => cli::commands::config_export(&output).await,
            cli::ConfigCommands::Import { input } => cli::commands::config_import(&input).await,
            cli::ConfigCommands::Proxy { app } => cli::commands::config_proxy(app).await,
            cli::ConfigCommands::Loadbalance { app, enabled } => {
                cli::commands::config_loadbalance(&app, enabled).await
            }
        },
        cli::Commands::Failover(cmd) => match cmd {
            cli::FailoverCommands::Queue { app } => cli::commands::failover_queue(&app).await,
            cli::FailoverCommands::Add { app, id } => cli::commands::failover_add(&app, &id).await,
            cli::FailoverCommands::Remove { app, id } => {
                cli::commands::failover_remove(&app, &id).await
            }
            cli::FailoverCommands::Toggle { app, enabled } => {
                cli::commands::failover_toggle(&app, enabled).await
            }
            cli::FailoverCommands::CircuitBreaker { app, id } => {
                cli::commands::failover_circuit_breaker(&app, id).await
            }
            cli::FailoverCommands::Reset { app, id } => {
                cli::commands::failover_reset(&app, &id).await
            }
        },
        cli::Commands::Stats(cmd) => match cmd {
            cli::StatsCommands::Summary { days, app } => {
                cli::commands::stats_summary(days, app).await
            }
            cli::StatsCommands::Provider { app, id, days } => {
                cli::commands::stats_provider(&app, id, days).await
            }
            cli::StatsCommands::Model { days } => cli::commands::stats_model(days).await,
            cli::StatsCommands::Logs {
                limit,
                app,
                provider,
            } => cli::commands::stats_logs(limit, app, provider).await,
        },
        cli::Commands::Mcp(cmd) => match cmd {
            cli::McpCommands::List { app } => cli::commands::mcp_list(app).await,
            cli::McpCommands::Add {
                name,
                command,
                args,
                enabled,
            } => cli::commands::mcp_add(&name, &command, args, enabled).await,
            cli::McpCommands::Remove { name } => cli::commands::mcp_remove(&name).await,
            cli::McpCommands::Toggle { name, app, enabled } => {
                cli::commands::mcp_toggle(&name, &app, enabled).await
            }
        },
        cli::Commands::Prompt(cmd) => match cmd {
            cli::PromptCommands::List { app } => cli::commands::prompt_list(app).await,
            cli::PromptCommands::Add { name, content, app } => {
                cli::commands::prompt_add(&name, &content, &app).await
            }
            cli::PromptCommands::Remove { name, app } => {
                cli::commands::prompt_remove(&name, &app).await
            }
            cli::PromptCommands::Show { name, app } => {
                cli::commands::prompt_show(&name, &app).await
            }
        },
        cli::Commands::Skill(cmd) => match cmd {
            cli::SkillCommands::List { app } => cli::commands::skill_list(app).await,
            cli::SkillCommands::Install { id, apps } => {
                cli::commands::skill_install(&id, apps).await
            }
            cli::SkillCommands::Uninstall { id, app } => {
                cli::commands::skill_uninstall(&id, app).await
            }
            cli::SkillCommands::Discover => cli::commands::skill_discover().await,
        },
    };

    // 处理结果
    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            cli::output::error(&e);
            std::process::exit(1);
        }
    }
}

/// 运行GUI模式
fn run_gui_mode() {
    // 在 Linux 上设置 WebKit 环境变量以解决 DMA-BUF 渲染问题
    // 某些 Linux 系统（如 Debian 13.2、Nvidia GPU）上 WebKitGTK 的 DMA-BUF 渲染器可能导致白屏/黑屏
    // 参考: https://github.com/tauri-apps/tauri/issues/9394
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    cc_switch_lib::run();
}
