use clap::Parser;

use super::{
    commands, output, server, Cli, Commands, ConfigCommands, FailoverCommands, McpCommands,
    PromptCommands, ProviderCommands, ServerCommands, SkillCommands, StatsCommands,
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
            ProviderCommands::Show { app, id } => commands::provider_show(&app, &id).await,
            ProviderCommands::Test { app, id } => commands::provider_test(&app, &id).await,
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
            ConfigCommands::Export { output } => commands::config_export(&output).await,
            ConfigCommands::Import { input } => commands::config_import(&input).await,
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
            FailoverCommands::CircuitBreaker { app, id } => {
                commands::failover_circuit_breaker(&app, id).await
            }
            FailoverCommands::Reset { app, id } => commands::failover_reset(&app, &id).await,
        },
        Commands::Stats(cmd) => match cmd {
            StatsCommands::Summary { days, app } => commands::stats_summary(days, app).await,
            StatsCommands::Provider { app, id, days } => {
                commands::stats_provider(&app, id, days).await
            }
            StatsCommands::Model { days } => commands::stats_model(days).await,
            StatsCommands::Logs {
                limit,
                app,
                provider,
            } => commands::stats_logs(limit, app, provider).await,
        },
        Commands::Mcp(cmd) => match cmd {
            McpCommands::List { app } => commands::mcp_list(app).await,
            McpCommands::Add {
                name,
                command,
                args,
                enabled,
            } => commands::mcp_add(&name, &command, args, enabled).await,
            McpCommands::Remove { name } => commands::mcp_remove(&name).await,
            McpCommands::Toggle { name, app, enabled } => {
                commands::mcp_toggle(&name, &app, enabled).await
            }
        },
        Commands::Prompt(cmd) => match cmd {
            PromptCommands::List { app } => commands::prompt_list(app).await,
            PromptCommands::Add { name, content, app } => {
                commands::prompt_add(&name, &content, &app).await
            }
            PromptCommands::Remove { name, app } => commands::prompt_remove(&name, &app).await,
            PromptCommands::Show { name, app } => commands::prompt_show(&name, &app).await,
        },
        Commands::Skill(cmd) => match cmd {
            SkillCommands::List { app } => commands::skill_list(app).await,
            SkillCommands::Install { id, apps } => commands::skill_install(&id, apps).await,
            SkillCommands::Uninstall { id, app } => commands::skill_uninstall(&id, app).await,
            SkillCommands::Discover => commands::skill_discover().await,
        },
    }
}
