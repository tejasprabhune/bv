mod cli;
mod commands;
mod errors;
mod mounts;
mod ops;
mod progress;
mod publish;
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
        } => {
            commands::add::run(
                tools,
                registry.as_deref(),
                *ignore_hardware,
                *allow_experimental,
                backend.as_deref(),
                *require_signed,
            )
            .await
        }
        Commands::Conformance {
            tool,
            registry,
            backend,
        } => commands::conform::run(tool, registry.as_deref(), backend.as_deref()).await,
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
        Commands::List { binaries } => commands::list::run(*binaries),
        Commands::Exec { command, args, .. } => commands::exec::run(command, args),
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
        } => commands::sync::run(*frozen, registry.as_deref(), backend.as_deref()).await,
        Commands::Doctor => commands::doctor::run(),
        Commands::Data(dc) => commands::data_cmd(dc).await,
        Commands::Cache(cache_cmd) => commands::cache(cache_cmd),
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
                platform: platform.clone(),
            })
            .await
        }
    }
}
