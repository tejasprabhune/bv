mod cli;
mod commands;
mod errors;
mod ops;
mod progress;
mod publish;
mod registry;

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
        } => commands::add::run(tools, registry.as_deref(), *ignore_hardware).await,
        Commands::Remove { tool } => commands::remove::run(tool),
        Commands::Run { tool, args } => commands::run::run(tool, args),
        Commands::List => commands::list::run(),
        Commands::Show { tool, format } => commands::show::run(tool, format.clone()),
        Commands::Info { tool } => commands::run::info(tool),
        Commands::Lock { check, registry } => {
            commands::lock::run(*check, registry.as_deref()).await
        }
        Commands::Sync { frozen, registry } => {
            commands::sync::run(*frozen, registry.as_deref()).await
        }
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
