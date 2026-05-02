use std::collections::{BTreeMap, HashSet};

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::lockfile::SpecKind;
use bv_core::manifest::{Manifest, Tier};
use bv_core::project::BvLock;

use crate::commands::add::{format_size, short_digest};

pub fn run(binaries: bool, layers: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        println!("no bv.lock found; run `bv add <tool>` to add tools to this project");
        return Ok(());
    }

    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    if binaries {
        return run_binaries(&lockfile);
    }

    if layers {
        return run_layers(&lockfile);
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

fn run_layers(lockfile: &bv_core::lockfile::Lockfile) -> anyhow::Result<()> {
    if lockfile.tools.is_empty() {
        println!("No tools installed.");
        return Ok(());
    }

    // Collect all layer digests across all factored tools to find shared layers.
    let mut digest_to_tools: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for entry in lockfile.tools.values() {
        if matches!(entry.spec_kind, SpecKind::FactoredOci) {
            for layer in &entry.layers {
                digest_to_tools
                    .entry(layer.digest.clone())
                    .or_default()
                    .push(&entry.tool_id);
            }
        }
    }

    let unique_digests: HashSet<&str> = digest_to_tools.keys().map(|s| s.as_str()).collect();

    let mut tools: Vec<_> = lockfile.tools.values().collect();
    tools.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));

    for entry in &tools {
        let kind_badge = match entry.spec_kind {
            SpecKind::FactoredOci => "factored"
                .if_supports_color(Stream::Stdout, |t| t.green().to_string())
                .to_string(),
            SpecKind::LegacyImage => "legacy"
                .if_supports_color(Stream::Stdout, |t| t.yellow().to_string())
                .to_string(),
        };

        println!(
            "  {} {}  {}",
            entry
                .tool_id
                .if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
            entry.version,
            kind_badge,
        );

        if matches!(entry.spec_kind, SpecKind::FactoredOci) {
            let mut unique_bytes: u64 = 0;
            let mut shared_bytes: u64 = 0;
            let shared_with: HashSet<String> = HashSet::new();
            let _ = shared_with;

            for layer in &entry.layers {
                let sharers = digest_to_tools
                    .get(&layer.digest)
                    .map(|v| v.len())
                    .unwrap_or(1);
                if sharers > 1 {
                    shared_bytes += layer.size;
                } else {
                    unique_bytes += layer.size;
                }
            }

            // Collect the set of tools this one shares at least one layer with.
            let mut shared_with_tools: HashSet<&str> = HashSet::new();
            for layer in &entry.layers {
                if let Some(tools_for_layer) = digest_to_tools.get(&layer.digest) {
                    for other in tools_for_layer {
                        if *other != entry.tool_id.as_str() {
                            shared_with_tools.insert(other);
                        }
                    }
                }
            }

            let unique_str = format_size(unique_bytes);
            let shared_str = if shared_bytes > 0 {
                format!(
                    "{}  shared{}",
                    format_size(shared_bytes)
                        .if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
                    if shared_with_tools.is_empty() {
                        String::new()
                    } else {
                        let mut names: Vec<_> = shared_with_tools.iter().copied().collect();
                        names.sort_unstable();
                        format!(" with {}", names.join(", "))
                            .if_supports_color(Stream::Stdout, |t| t.dimmed().to_string())
                            .to_string()
                    }
                )
            } else {
                String::new()
            };

            println!(
                "    {} layers  {} unique  {}",
                entry.layers.len(),
                unique_str,
                shared_str,
            );

            for layer in &entry.layers {
                let pkg_note = layer
                    .conda_package
                    .as_ref()
                    .map(|p| format!("{}=={}", p.name, p.version))
                    .unwrap_or_else(|| "-".to_string());
                let size_str = format_size(layer.size);
                let digest_str = short_digest(&layer.digest);
                let sharers = digest_to_tools
                    .get(&layer.digest)
                    .map(|v| v.len())
                    .unwrap_or(1);
                let shared_note = if sharers > 1 {
                    format!("  ↑shared×{}", sharers)
                        .if_supports_color(Stream::Stdout, |t| t.dimmed().to_string())
                        .to_string()
                } else {
                    String::new()
                };
                println!(
                    "      {}  {}  {}{}",
                    pkg_note,
                    size_str.if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
                    digest_str.if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
                    shared_note,
                );
            }
        } else {
            let size_str = entry
                .image_size_bytes
                .map(format_size)
                .unwrap_or_else(|| "-".into());
            println!(
                "    monolithic image  {}",
                size_str.if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
            );
        }
    }

    // Print dedup summary.
    if !unique_digests.is_empty() {
        let total_layers: usize = lockfile
            .tools
            .values()
            .filter(|e| matches!(e.spec_kind, SpecKind::FactoredOci))
            .map(|e| e.layers.len())
            .sum();
        let unique_count = unique_digests.len();
        let savings_pct = if total_layers > 0 {
            100u64.saturating_sub((unique_count as u64 * 100) / total_layers as u64)
        } else {
            0
        };
        println!();
        println!(
            "  {} unique layers across {} total  ({}% deduplication)",
            unique_count, total_layers, savings_pct,
        );
    }

    Ok(())
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
