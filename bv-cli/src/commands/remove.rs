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
    let _ = lockfile.rebuild_binary_index(&bv_toml.binary_overrides);

    // True two-file atomicity isn't possible without a journal, but writing
    // bv.toml FIRST minimizes the bad case: if the second write fails, the
    // user's source-of-truth (bv.toml) is correct and `bv lock` will
    // regenerate bv.lock. The previous order (lock first) left bv.lock with
    // the tool removed but bv.toml still declaring it.
    bv_toml.to_path(&bv_toml_path)?;
    BvLock::to_path(&lockfile, &bv_lock_path)?;
    crate::shims::write_shims(&cwd, &lockfile)?;

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
