pub mod add;
pub mod data;
pub mod doctor;
pub mod list;
pub mod lock;
pub mod remove;
pub mod run;
pub mod show;
pub mod sync;

use crate::cli::{CacheCommands, DataCommands};

pub async fn data_cmd(cmd: &DataCommands) -> anyhow::Result<()> {
    match cmd {
        DataCommands::Fetch {
            datasets,
            registry,
            yes,
        } => data::fetch(datasets, registry.as_deref(), *yes).await,
        DataCommands::List => data::list(),
    }
}

pub fn cache(cmd: &CacheCommands) -> anyhow::Result<()> {
    match cmd {
        CacheCommands::Prune { apply } => {
            if *apply {
                eprintln!("  cache prune: not yet implemented");
                eprintln!("  Use `docker image prune` to remove unused Docker images for now.");
            } else {
                eprintln!("  (dry run) cache prune: not yet implemented");
                eprintln!("  Use `docker images` to see locally cached images.");
                eprintln!("  Pass --apply to actually remove (once implemented).");
            }
            Ok(())
        }
    }
}
