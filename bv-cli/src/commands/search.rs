use anyhow::Context;
use owo_colors::{OwoColorize, Stream};
use serde::Deserialize;

use bv_core::cache::CacheLayout;
use bv_index::{GitIndex, IndexBackend as _};

use crate::registry::{STALE_TTL, maybe_print_refresh, open_index, resolve_registry_url};

pub async fn run(
    query: &str,
    tier_flag: Option<&str>,
    registry_flag: Option<&str>,
    limit: usize,
) -> anyhow::Result<()> {
    let cache = CacheLayout::new();
    let registry_url = resolve_registry_url(registry_flag, None);
    let index = open_index(&registry_url, &cache);

    let refreshed = index
        .refresh_if_stale(STALE_TTL)
        .context("registry refresh failed")?;
    maybe_print_refresh(refreshed);

    if !index.is_available() {
        anyhow::bail!(
            "registry not yet cloned\n  \
             Run `bv add <tool>` first to initialize the local index"
        );
    }

    let all_entries = load_entries(&index);
    let q = query.to_lowercase();

    // Deprecated tools are hidden by default but surfaced (with a marker) when
    // `--tier all` is requested, so users explicitly asking to see everything
    // still see deprecated entries.
    let include_deprecated = matches!(tier_flag, Some("all"));
    let mut results: Vec<&SearchEntry> = all_entries
        .iter()
        .filter(|e| include_deprecated || !e.deprecated)
        .filter(|e| matches_tier_filter(e, tier_flag))
        .filter(|e| q.is_empty() || score(e, &q) > 0)
        .collect();

    results.sort_by(|a, b| {
        score(b, &q)
            .cmp(&score(a, &q))
            .then(tier_sort_key(&a.tier).cmp(&tier_sort_key(&b.tier)))
            .then(a.id.cmp(&b.id))
    });
    results.truncate(limit);

    if results.is_empty() {
        let filter_note = tier_flag
            .map(|t| format!(" (tier: {})", t))
            .unwrap_or_default();
        eprintln!("No tools found matching '{}'{}", query, filter_note);
        return Ok(());
    }

    let w_id = results.iter().map(|e| e.id.len()).max().unwrap_or(4).max(4);
    let w_ver = results
        .iter()
        .map(|e| e.version.len())
        .max()
        .unwrap_or(7)
        .max(7);

    println!(
        "  {:<w_id$}  {:<w_ver$}  {:<12}  {}",
        "Tool".bold(),
        "Version".bold(),
        "Tier".bold(),
        "Description".bold(),
    );
    println!("  {}", "-".repeat(w_id + w_ver + 12 + 40 + 6));

    for entry in &results {
        let tier_display = format_tier(&entry.tier);
        let raw_desc = entry
            .description
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect::<String>();
        let desc = if entry.deprecated {
            let marker = "[deprecated]"
                .if_supports_color(Stream::Stdout, |t| t.red().to_string())
                .to_string();
            format!("{marker} {raw_desc}")
        } else {
            raw_desc
                .if_supports_color(Stream::Stdout, |t| t.dimmed().to_string())
                .to_string()
        };
        println!(
            "  {:<w_id$}  {:<w_ver$}  {:<12}  {}",
            entry.id, entry.version, tier_display, desc,
        );
    }

    println!(
        "\n  {} tools shown (use `bv show <tool>` for details)",
        results.len()
    );

    Ok(())
}

#[derive(Debug, Deserialize)]
struct SearchIndex {
    tools: Vec<SearchEntry>,
}

#[derive(Debug, Deserialize)]
struct SearchEntry {
    id: String,
    version: String,
    description: Option<String>,
    tier: String,
    #[serde(default)]
    input_types: Vec<String>,
    #[serde(default)]
    output_types: Vec<String>,
    #[serde(default)]
    deprecated: bool,
}

fn load_entries(index: &GitIndex) -> Vec<SearchEntry> {
    let json_path = index.local_path().join("index.json");
    if let Ok(content) = std::fs::read_to_string(&json_path)
        && let Ok(idx) = serde_json::from_str::<SearchIndex>(&content)
    {
        return idx.tools;
    }

    // Fallback: derive from manifest list when index.json is absent.
    index
        .list_tools()
        .unwrap_or_default()
        .into_iter()
        .map(|s| SearchEntry {
            id: s.id,
            version: s.latest_version,
            description: s.description,
            tier: s.tier.as_str().to_string(),
            input_types: s.input_types,
            output_types: s.output_types,
            deprecated: s.deprecated,
        })
        .collect()
}

fn score(entry: &SearchEntry, query: &str) -> i32 {
    let mut s = 0i32;
    if entry.id == query {
        s += 100;
    } else if entry.id.contains(query) {
        s += 50;
    }
    if entry
        .description
        .as_deref()
        .unwrap_or("")
        .to_lowercase()
        .contains(query)
    {
        s += 20;
    }
    for t in &entry.input_types {
        if t.contains(query) {
            s += 10;
        }
    }
    for t in &entry.output_types {
        if t.contains(query) {
            s += 5;
        }
    }
    s
}

fn matches_tier_filter(entry: &SearchEntry, flag: Option<&str>) -> bool {
    match flag {
        None => entry.tier != "experimental",
        Some("all") => true,
        Some(t) => entry.tier == t,
    }
}

fn tier_sort_key(tier: &str) -> u8 {
    match tier {
        "core" => 0,
        "community" => 1,
        _ => 2,
    }
}

fn format_tier(tier: &str) -> String {
    match tier {
        "core" => "core"
            .if_supports_color(Stream::Stdout, |t| t.green().to_string())
            .to_string(),
        "community" => "community"
            .if_supports_color(Stream::Stdout, |t| t.yellow().to_string())
            .to_string(),
        _ => "experimental"
            .if_supports_color(Stream::Stdout, |t| t.red().dimmed().to_string())
            .to_string(),
    }
}
