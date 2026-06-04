use clap::Parser;

use super::{
    commands, output, server, Cli, ColorWhen, Commands, ConfigCommands, FailoverCommands,
    HyperparamsCommands, ProviderCommands, ServerCommands,
};

pub fn has_cli_args() -> bool {
    // 跳过出现在子命令之前的全局标志（--color[=..] / --no-color），
    // 再用第一个非全局标志 token 判定是否进入 CLI（而非启动 GUI）。
    let mut args = std::env::args_os()
        .skip(1)
        .filter_map(|arg| arg.into_string().ok());

    while let Some(arg) = args.next() {
        let token = arg.trim();
        if token.is_empty() {
            continue;
        }
        // --color VALUE（空格分隔）：跳过其值
        if token == "--color" {
            let _ = args.next();
            continue;
        }
        // --color=VALUE / --no-color：直接跳过
        if token.starts_with("--color=") || token == "--no-color" {
            continue;
        }
        return matches!(
            token,
            "server"
                | "srv"
                | "start"
                | "stop"
                | "status"
                | "restart"
                | "provider"
                | "p"
                | "config"
                | "cfg"
                | "failover"
                | "fo"
                | "stats"
                | "st"
                | "mcp"
                | "m"
                | "prompt"
                | "pr"
                | "skill"
                | "sk"
                | "help"
                | "-h"
                | "--help"
                | "-V"
                | "--version"
        );
    }
    false
}

pub fn run_from_env() -> ! {
    crate::ensure_rustls_crypto_provider();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|err| panic!("failed to build CLI runtime: {err}"));

    let exit_code = runtime.block_on(async {
        let cli = Cli::parse();

        // 在任何输出之前设置全局颜色模式（显式 --color/--no-color 优先于自动检测）
        let color_mode = if cli.no_color || cli.color == ColorWhen::Never {
            output::ColorMode::Never
        } else if cli.color == ColorWhen::Always {
            output::ColorMode::Always
        } else {
            output::ColorMode::Auto
        };
        output::init_color(color_mode);

        match run_cli(cli).await {
            Ok(()) => 0,
            Err(err) => {
                output::error(&err);
                1
            }
        }
    });

    std::process::exit(exit_code);
}

async fn run_cli(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Server(cmd) => match cmd {
            ServerCommands::Start {
                port,
                host,
                foreground,
                daemon,
                web_port,
                web_bind,
            } => {
                let background = daemon || !foreground;
                server::start_headless_server(host, port, background, web_port, web_bind).await
            }
            ServerCommands::Stop => server::stop_server().await,
            ServerCommands::Status => server::server_status().await,
            ServerCommands::Restart { port } => server::restart_server(port).await,
        },
        Commands::Start {
            port,
            host,
            foreground,
            daemon,
            web_port,
            web_bind,
        } => {
            let background = daemon || !foreground;
            server::start_headless_server(host, port, background, web_port, web_bind).await
        }
        Commands::Stop => server::stop_server().await,
        Commands::Status => server::server_status().await,
        Commands::Restart { port } => server::restart_server(port).await,
        Commands::Provider(cmd) => match cmd {
            ProviderCommands::List { app, verbose } => commands::provider_list(&app, verbose).await,
            ProviderCommands::Add {
                app,
                name,
                key,
                url,
                file,
            } => commands::provider_add(&app, &name, key, url, file).await,
            ProviderCommands::Remove { app, id } => commands::provider_remove(&app, &id).await,
            ProviderCommands::Switch { app, id } => commands::provider_switch(&app, &id).await,
            ProviderCommands::Weight { app, id, weight } => {
                commands::provider_set_weight(&app, &id, weight).await
            }
            ProviderCommands::ModelMap { app, id, from, to } => {
                commands::provider_set_model_mapping(&app, &id, &from, &to).await
            }
            ProviderCommands::EnvSet {
                app,
                id,
                key,
                value,
            } => commands::provider_set_env(&app, &id, &key, &value).await,
            ProviderCommands::Hyperparams(cmd) => match cmd {
                HyperparamsCommands::Show { app, id, path } => {
                    commands::provider_hyperparams_show(&app, &id, path.as_deref()).await
                }
                HyperparamsCommands::Set {
                    app,
                    id,
                    path,
                    json,
                    value,
                } => {
                    commands::provider_hyperparams_set(
                        &app,
                        &id,
                        &path,
                        json.as_deref(),
                        value.as_deref(),
                    )
                    .await
                }
                HyperparamsCommands::Remove { app, id, path } => {
                    commands::provider_hyperparams_remove(&app, &id, &path).await
                }
            },
            ProviderCommands::Show { app, id } => commands::provider_show(&app, &id).await,
            ProviderCommands::Export {
                app,
                output,
                id,
                redact,
            } => commands::provider_export(&app, &output, id.as_deref(), redact).await,
            ProviderCommands::Import {
                app,
                input,
                overwrite,
                new_ids,
                set_current,
            } => commands::provider_import(&app, &input, overwrite, new_ids, set_current).await,
            ProviderCommands::Update {
                app,
                id,
                file,
                key,
                url,
                replace,
                name,
                notes,
            } => {
                commands::provider_update(
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
        Commands::Config(cmd) => match cmd {
            ConfigCommands::Show { app } => commands::config_show(app).await,
            ConfigCommands::Set { key, value, app } => {
                commands::config_set(&key, &value, app).await
            }
            ConfigCommands::Proxy { app } => commands::config_proxy(app).await,
            ConfigCommands::Loadbalance {
                app,
                enabled,
                strategy,
            } => commands::config_loadbalance(&app, enabled, strategy).await,
        },
        Commands::Failover(cmd) => match cmd {
            FailoverCommands::Queue { app } => commands::failover_queue(&app).await,
            FailoverCommands::Add { app, id } => commands::failover_add(&app, &id).await,
            FailoverCommands::Remove { app, id } => commands::failover_remove(&app, &id).await,
            FailoverCommands::Toggle { app, enabled } => {
                commands::failover_toggle(&app, enabled).await
            }
        },
    }
}
