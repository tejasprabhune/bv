pub mod add;
pub mod cache;
pub mod conform;
pub mod data;
pub mod doctor;
pub mod exec;
pub mod export;
pub mod list;
pub mod lock;
pub mod remove;
pub mod run;
pub mod search;
pub mod shell;
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
        DataCommands::Verify {
            registry,
            filter,
            jobs,
            size_tolerance,
        } => {
            data::verify(
                registry.as_deref(),
                filter.as_deref(),
                *jobs,
                *size_tolerance,
            )
            .await
        }
    }
}

pub fn cache_cmd(cmd: &CacheCommands) -> anyhow::Result<()> {
    cache::run(cmd)
}
