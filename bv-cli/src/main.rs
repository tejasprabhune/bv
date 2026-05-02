mod cli;
mod commands;
mod errors;
mod mounts;
mod ops;
mod progress;
mod publish;
mod pull_native;
mod registry;
mod runtime_select;
mod shims;

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let level = if cli.verbose { "debug" } else { "warn" };
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)),
        )
        .with_target(false)
        .without_time()
        .init();

    match &cli.command {
        Commands::Add {
            tools,
            registry,
            ignore_hardware,
            allow_experimental,
            backend,
            require_signed,
            jobs,
        } => {
            commands::add::run(
                tools,
                registry.as_deref(),
                *ignore_hardware,
                *allow_experimental,
                backend.as_deref(),
                *require_signed,
                *jobs,
            )
            .await
        }
        Commands::Conformance {
            tool,
            registry,
            backend,
            filter,
            skip_gpu,
            skip_reference_data,
            skip_deprecated,
            jobs,
        } => match tool {
            Some(t) => commands::conform::run(t, registry.as_deref(), backend.as_deref()).await,
            None => {
                commands::conform::run_all(
                    registry.as_deref(),
                    backend.as_deref(),
                    filter.as_deref(),
                    *skip_gpu,
                    *skip_reference_data,
                    *skip_deprecated,
                    *jobs,
                )
                .await
            }
        },
        Commands::Search {
            query,
            tier,
            registry,
            limit,
        } => commands::search::run(query, tier.as_deref(), registry.as_deref(), *limit).await,
        Commands::Remove { tool } => commands::remove::run(tool),
        Commands::Run {
            tool,
            args,
            backend,
            ..
        } => commands::run::run(tool, args, backend.as_deref()).await,
        Commands::List { binaries, layers } => commands::list::run(*binaries, *layers),
        Commands::Why { package } => commands::why::run(package),
        Commands::Exec {
            command,
            args,
            no_sync,
            ..
        } => commands::exec::run(command, args, *no_sync).await,
        Commands::Shell { shell } => commands::shell::run(shell.as_deref()),
        Commands::Show { tool, format } => commands::show::run(tool, format.clone()),
        Commands::Info { tool } => commands::run::info(tool),
        Commands::Lock { check, registry } => {
            commands::lock::run(*check, registry.as_deref()).await
        }
        Commands::Sync {
            frozen,
            registry,
            backend,
            jobs,
        } => commands::sync::run(*frozen, registry.as_deref(), backend.as_deref(), *jobs).await,
        Commands::Doctor => commands::doctor::run(),
        Commands::Export { format, output } => commands::export::run(format, output.as_deref()),
        Commands::Data(dc) => commands::data_cmd(dc).await,
        Commands::Cache(cache_cmd) => commands::cache_cmd(cache_cmd),
        Commands::Publish {
            source,
            tool_name,
            tool_version,
            non_interactive,
            no_push,
            no_pr,
            github_token,
            ghcr_token,
            registry_repo,
            push_to,
            platform,
        } => {
            publish::run(publish::PublishOpts {
                source: source.clone(),
                tool_name: tool_name.clone(),
                version: tool_version.clone(),
                non_interactive: *non_interactive,
                no_push: *no_push,
                no_pr: *no_pr,
                github_token: github_token.clone(),
                ghcr_token: ghcr_token.clone(),
                registry_repo: registry_repo.clone(),
                push_to: push_to.clone(),
                platform: platform.clone(),
            })
            .await
        }
    }
}
