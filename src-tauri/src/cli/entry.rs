use clap::Parser;

use super::{
    commands, output, server, Cli, Commands, ConfigCommands, FailoverCommands, HyperparamsCommands,
    ProviderCommands, ServerCommands,
};

pub fn has_cli_args() -> bool {
    matches!(
        std::env::args_os()
            .nth(1)
            .as_deref()
            .and_then(|arg| arg.to_str())
            .map(str::trim)
            .filter(|arg| !arg.is_empty()),
        Some(
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
        )
    )
}

pub fn run_from_env() -> ! {
    crate::ensure_rustls_crypto_provider();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|err| panic!("failed to build CLI runtime: {err}"));

    let exit_code = runtime.block_on(async {
        let cli = Cli::parse();
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
            } => {
                let background = daemon || !foreground;
                server::start_headless_server(host, port, background).await
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
        } => {
            let background = daemon || !foreground;
            server::start_headless_server(host, port, background).await
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
            ConfigCommands::Loadbalance { app, enabled } => {
                commands::config_loadbalance(&app, enabled).await
            }
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
