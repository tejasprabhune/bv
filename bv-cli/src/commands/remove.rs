use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::project::{BvLock, BvToml};

pub fn run(tool: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_toml_path = cwd.join("bv.toml");
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_toml_path.exists() {
        anyhow::bail!("no bv.toml found in current directory");
    }

    let mut bv_toml = BvToml::from_path(&bv_toml_path).context("failed to read bv.toml")?;

    let before = bv_toml.tools.len();
    bv_toml.tools.retain(|t| t.id != tool);
    if bv_toml.tools.len() == before {
        anyhow::bail!("tool '{}' is not declared in bv.toml", tool);
    }

    let mut lockfile = if bv_lock_path.exists() {
        BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?
    } else {
        bv_core::lockfile::Lockfile::new()
    };

    lockfile.tools.remove(tool);

    bv_toml.to_path(&bv_toml_path)?;
    BvLock::to_path(&lockfile, &bv_lock_path)?;

    eprintln!(
        "  {} {}",
        "Removed".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
        tool.if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
    );
    eprintln!("  The container image is still in your local Docker cache.");
    eprintln!(
        "  Run `{}` to remove unused images.",
        "bv cache prune".if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
    );

    Ok(())
}
