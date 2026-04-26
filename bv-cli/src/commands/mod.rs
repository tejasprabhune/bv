pub mod add;
pub mod doctor;
pub mod list;
pub mod lock;
pub mod remove;
pub mod run;
pub mod sync;

use owo_colors::{OwoColorize, Stream};

use crate::cli::{CacheCommands, DataCommands};

pub fn data(cmd: &DataCommands) -> anyhow::Result<()> {
    match cmd {
        DataCommands::Fetch { dataset } => {
            eprintln!(
                "  {} data fetch {dataset}: not yet implemented",
                "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
            );
        }
        DataCommands::List => {
            eprintln!(
                "  {} data list: not yet implemented",
                "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
            );
        }
    }
    Ok(())
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
        }
    }
    Ok(())
}
