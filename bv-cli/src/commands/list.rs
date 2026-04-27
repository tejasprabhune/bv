use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::manifest::{Manifest, Tier};
use bv_core::project::BvLock;

use crate::commands::add::{format_size, short_digest};

pub fn run(binaries: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        println!("No bv.lock found. Run `bv add <tool>` to add tools to this project.");
        return Ok(());
    }

    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    if binaries {
        return run_binaries(&lockfile);
    }

    if lockfile.tools.is_empty() {
        println!("No tools installed. Run `bv add <tool>` to get started.");
        return Ok(());
    }

    let cache = CacheLayout::new();
    let mut tools: Vec<_> = lockfile.tools.values().collect();
    tools.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));

    let w_tool = tools
        .iter()
        .map(|e| e.tool_id.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let w_ver = tools
        .iter()
        .map(|e| e.version.len())
        .max()
        .unwrap_or(7)
        .max(7);

    println!(
        "  {:<w_tool$}  {:<w_ver$}  {:<12}  {:<12}  {:<8}  {}",
        "Tool".bold(),
        "Version".bold(),
        "Tier".bold(),
        "Digest".bold(),
        "Size".bold(),
        "Added".bold(),
    );
    println!("  {}", "-".repeat(w_tool + w_ver + 12 + 12 + 8 + 10 + 10));

    for entry in tools {
        let tier = read_cached_tier(&cache, &entry.tool_id, &entry.version);
        let tier_display = format_tier(&tier);
        let digest_short = short_digest(&entry.image_digest);
        let size = entry
            .image_size_bytes
            .map(format_size)
            .unwrap_or_else(|| "-".into());
        let date = entry.resolved_at.format("%Y-%m-%d").to_string();

        println!(
            "  {:<w_tool$}  {:<w_ver$}  {:<12}  {:<12}  {:<8}  {}",
            entry.tool_id,
            entry.version,
            tier_display,
            digest_short,
            size,
            date.if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
        );
    }

    Ok(())
}

fn run_binaries(lockfile: &bv_core::lockfile::Lockfile) -> anyhow::Result<()> {
    if lockfile.binary_index.is_empty() {
        println!("No binaries indexed. Run `bv sync` to regenerate.");
        return Ok(());
    }

    let cache = CacheLayout::new();
    let mut pairs: Vec<_> = lockfile.binary_index.iter().collect();
    pairs.sort_by_key(|(bin, _)| bin.as_str());

    let w_bin = pairs.iter().map(|(b, _)| b.len()).max().unwrap_or(6).max(6);

    println!("  {:<w_bin$}  {}", "Binary".bold(), "Tool".bold(),);
    println!("  {}", "-".repeat(w_bin + 20));

    for (binary, tool_id) in &pairs {
        let entry = lockfile.tools.get(tool_id.as_str());
        let version = entry.map(|e| e.version.as_str()).unwrap_or("-");
        println!(
            "  {:<w_bin$}  {} {}",
            binary,
            tool_id.if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
            version.if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
        );
        let _ = cache;
    }

    Ok(())
}

fn read_cached_tier(cache: &CacheLayout, tool_id: &str, version: &str) -> Tier {
    let path = cache.manifest_path(tool_id, version);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| Manifest::from_toml_str(&s).ok())
        .map(|m| m.tool.tier)
        .unwrap_or_default()
}

fn format_tier(tier: &Tier) -> String {
    match tier {
        Tier::Core => "core"
            .if_supports_color(Stream::Stdout, |t| t.green().to_string())
            .to_string(),
        Tier::Community => "community"
            .if_supports_color(Stream::Stdout, |t| t.yellow().to_string())
            .to_string(),
        Tier::Experimental => "experimental"
            .if_supports_color(Stream::Stdout, |t| t.red().to_string())
            .to_string(),
    }
}
