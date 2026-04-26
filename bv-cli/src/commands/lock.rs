use anyhow::Context;
use indicatif::MultiProgress;
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::project::{BvLock, BvToml};

use crate::ops;

pub async fn run(check: bool, registry_flag: Option<&str>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_toml_path = cwd.join("bv.toml");
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_toml_path.exists() {
        anyhow::bail!("no bv.toml found; run `bv add <tool>` to set up a project");
    }

    let bv_toml = BvToml::from_path(&bv_toml_path).context("failed to read bv.toml")?;
    let existing_lock = bv_lock_path
        .exists()
        .then(|| BvLock::from_path(&bv_lock_path))
        .transpose()
        .context("failed to read bv.lock")?;

    let registry_url = crate::registry::resolve_registry_url(registry_flag, Some(&bv_toml));
    let cache = CacheLayout::new();
    let index = crate::registry::open_index(&registry_url, &cache);

    // bv lock uses the cached index; run `bv add` to fetch the latest registry state.
    crate::registry::require_index(&index, &registry_url)?;

    let resolved = ops::resolve_all(&bv_toml.tools, &index)?;

    let mp = MultiProgress::new();
    let new_lock = ops::generate_lockfile(resolved, existing_lock.as_ref(), None, &mp).await?;

    if check {
        match &existing_lock {
            None => {
                anyhow::bail!("no bv.lock found\n  run `bv lock` to generate it");
            }
            Some(existing) => {
                if existing.is_equivalent_to(&new_lock) {
                    eprintln!(
                        "  {} bv.lock is up to date ({} tool{})",
                        "ok".if_supports_color(Stream::Stderr, |t| t.green().to_string()),
                        new_lock.tools.len(),
                        if new_lock.tools.len() == 1 { "" } else { "s" }
                    );
                } else {
                    let diff = ops::lock_diff(existing, &new_lock);
                    for line in &diff {
                        eprintln!("{line}");
                    }
                    eprintln!();
                    anyhow::bail!("bv.lock is out of date; run `bv lock` to update");
                }
            }
        }
    } else {
        BvLock::to_path(&new_lock, &bv_lock_path)?;
        let n = new_lock.tools.len();
        eprintln!(
            "  {} bv.lock ({} tool{})",
            "Updated".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
            n,
            if n == 1 { "" } else { "s" }
        );
    }

    Ok(())
}
