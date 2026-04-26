use anyhow::Context;
use owo_colors::OwoColorize;

use bv_core::project::BvLock;

use crate::commands::add::{format_size, short_digest};

pub fn run() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        println!("No bv.lock found. Run `bv add <tool>` to add tools to this project.");
        return Ok(());
    }

    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    if lockfile.tools.is_empty() {
        println!("No tools installed. Run `bv add <tool>` to get started.");
        return Ok(());
    }

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
        "  {:<w_tool$}  {:<w_ver$}  {:<12}  {:<8}  {}",
        "Tool".bold(),
        "Version".bold(),
        "Digest".bold(),
        "Size".bold(),
        "Added".bold(),
    );
    println!("  {}", "-".repeat(w_tool + w_ver + 12 + 8 + 10 + 8));

    for entry in tools {
        let digest_short = short_digest(&entry.image_digest);
        let size = entry
            .image_size_bytes
            .map(format_size)
            .unwrap_or_else(|| "-".into());
        let date = entry.resolved_at.format("%Y-%m-%d").to_string();

        println!(
            "  {:<w_tool$}  {:<w_ver$}  {:<12}  {:<8}  {}",
            entry.tool_id,
            entry.version,
            digest_short,
            size,
            date.dimmed(),
        );
    }

    Ok(())
}
