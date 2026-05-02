use std::path::Path;

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};
use serde_json::Value;

use bv_core::cache::CacheLayout;
use bv_core::hardware::DetectedHardware;
use bv_core::lockfile::SpecKind;
use bv_core::project::BvLock;
use bv_runtime::{ContainerRuntime, DockerRuntime};
use bv_runtime_apptainer::{ApptainerRuntime, is_available as apptainer_available};

use crate::commands::add::format_size;

// Width of the left "key" column inside each section.
const KEY_W: usize = 10;

pub fn run() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let mut blocking_issues = 0;

    section("Runtime");
    let docker_ok = match DockerRuntime.health_check() {
        Ok(info) => {
            let server_ver = info
                .extra
                .get("server_version")
                .cloned()
                .unwrap_or_default();
            let server = if server_ver.is_empty() {
                String::new()
            } else {
                format!("  server {}", server_ver.dimmed())
            };
            kv("docker", &format!("{}{}", info.version, server));

            let major: u32 = server_ver
                .split('.')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if major > 0 && major < 28 {
                eprintln!(
                    "    {:KEY_W$} {} Docker 28+ recommended for native GHCR pull (bv sync ghcr.io/...)",
                    "",
                    "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string()),
                );
            }

            check_ghcr_credentials();
            true
        }
        Err(_) => {
            kv_dim("docker", "not available");
            false
        }
    };
    let _ = docker_ok;

    let cache = CacheLayout::new();
    let apptainer_rt = ApptainerRuntime::new(cache.sif_dir());
    match apptainer_rt.health_check() {
        Ok(info) => {
            kv("apptainer", &info.version);
        }
        Err(_) if apptainer_available() => {
            kv_dim("apptainer", "found but health check failed");
        }
        Err(_) => {
            kv_dim("apptainer", "not available");
        }
    }

    if DockerRuntime.health_check().is_err() && !apptainer_available() {
        eprintln!(
            "    {:KEY_W$} {}",
            "",
            "no container runtime found; install Docker or Apptainer"
                .if_supports_color(Stream::Stderr, |t| t.red().to_string())
        );
        blocking_issues += 1;
    }

    section("Hardware");
    let hw = DetectedHardware::detect();
    kv("cpu", &format!("{} logical cores", hw.cpu_cores));
    kv("ram", &format!("{:.1} GB total", hw.ram_gb()));
    kv("disk", &format!("{:.1} GB free", hw.disk_free_gb()));

    if hw.gpus.is_empty() {
        kv_dim("gpu", "none detected");
    } else {
        for gpu in &hw.gpus {
            let vram_gb = gpu.vram_mb as f64 / 1024.0;
            let cuda = gpu
                .cuda_version
                .as_ref()
                .map(|v| format!("  CUDA {v}"))
                .unwrap_or_default();
            kv(
                "gpu",
                &format!("{} ({:.0} GB VRAM{})", gpu.name, vram_gb, cuda),
            );
        }
    }

    section("Cache");
    let cache = CacheLayout::new();
    let root = cache.root().clone();
    kv("path", &root.display().to_string());

    // Recursive sizing of ~/.cache/bv can take seconds for users with many GB
    // pulled. Show a spinner so it doesn't look frozen.
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_style(
        indicatif::ProgressStyle::with_template("    {spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.set_message("computing cache size");
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    let total = dir_size_bytes(&root);
    spinner.finish_and_clear();

    // The previous code did `cache.image_dir("").parent()`, which returned
    // ~/.cache/bv (the cache root) and counted every top-level child (data,
    // index, sif, images, tmp) as an "image". Use the images directory
    // directly: `image_dir("")` itself is `<root>/images/`.
    let images_dir = cache.image_dir("");
    let image_count = if images_dir.exists() {
        count_subdirs(&images_dir)
    } else {
        0
    };
    kv(
        "size",
        &format!(
            "{}  {} image{}",
            format_size(total),
            image_count,
            if image_count == 1 { "" } else { "s" }
        ),
    );

    section("Index");
    let index_parent = cache.index_dir("default");
    let index_parent = index_parent.parent().unwrap_or(cache.root());

    let mut index_count = 0;
    if index_parent.exists() {
        for entry in std::fs::read_dir(index_parent)
            .into_iter()
            .flatten()
            .flatten()
        {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();
                let url = git_remote_url(&path).unwrap_or_else(|| path.display().to_string());
                kv(&name, &url);
                index_count += 1;
            }
        }
    }
    if index_count == 0 {
        kv_dim("indexes", "none cloned yet; run `bv add <tool>` first");
    }

    section("Data");
    let data_root = cache.root().join("data");
    if data_root.exists() {
        let mut dataset_count = 0usize;
        let mut dataset_size = 0u64;
        if let Ok(entries) = std::fs::read_dir(&data_root) {
            for id_entry in entries.flatten() {
                if !id_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                if let Ok(ver_entries) = std::fs::read_dir(id_entry.path()) {
                    for ver_entry in ver_entries.flatten() {
                        if ver_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            dataset_count += 1;
                            dataset_size += dir_size_bytes(&ver_entry.path());
                        }
                    }
                }
            }
        }
        if dataset_count == 0 {
            kv_dim("datasets", "none downloaded yet");
        } else {
            kv(
                "datasets",
                &format!("{}  {}", dataset_count, format_size(dataset_size)),
            );
        }
    } else {
        kv_dim("datasets", "none downloaded yet");
    }

    section("Project");
    let bv_toml_path = cwd.join("bv.toml");
    let bv_lock_path = cwd.join("bv.lock");

    if bv_toml_path.exists() {
        // Count declared tools.
        let n = bv_core::project::BvToml::from_path(&bv_toml_path)
            .map(|t| t.tools.len())
            .unwrap_or(0);
        kv(
            "bv.toml",
            &format!("{} tool{} declared", n, if n == 1 { "" } else { "s" }),
        );
    } else {
        kv_dim("bv.toml", "not found in current directory");
    }

    if bv_lock_path.exists() {
        let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;
        let tools: Vec<_> = {
            let mut v: Vec<_> = lockfile.tools.values().collect();
            v.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));
            v.iter()
                .map(|e| format!("{} {}", e.tool_id, e.version))
                .collect()
        };
        kv("bv.lock", &tools.join(", "));

        // Factored image summary.
        let total_tools = lockfile.tools.len();
        let factored_count = lockfile
            .tools
            .values()
            .filter(|e| matches!(e.spec_kind, SpecKind::FactoredOci))
            .count();
        let legacy_count = total_tools - factored_count;

        if total_tools > 0 {
            let total_layers: usize = lockfile
                .tools
                .values()
                .filter(|e| matches!(e.spec_kind, SpecKind::FactoredOci))
                .map(|e| e.layers.len())
                .sum();

            let unique_layer_count = {
                let mut seen = std::collections::HashSet::new();
                for entry in lockfile.tools.values() {
                    if matches!(entry.spec_kind, SpecKind::FactoredOci) {
                        for layer in &entry.layers {
                            seen.insert(layer.digest.clone());
                        }
                    }
                }
                seen.len()
            };

            kv(
                "images",
                &format!(
                    "{} factored  {} legacy",
                    factored_count.if_supports_color(Stream::Stderr, |t| t.green().to_string()),
                    if legacy_count > 0 {
                        legacy_count
                            .if_supports_color(Stream::Stderr, |t| t.yellow().to_string())
                            .to_string()
                    } else {
                        legacy_count
                            .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
                            .to_string()
                    }
                ),
            );

            if total_layers > 0 {
                let dedup_pct =
                    100u64.saturating_sub((unique_layer_count as u64 * 100) / total_layers as u64);
                kv(
                    "dedup",
                    &format!(
                        "{}% ({} unique / {} total layers)",
                        dedup_pct, unique_layer_count, total_layers
                    ),
                );
            }

            if legacy_count > 0 && factored_count > 0 {
                eprintln!(
                    "    {:KEY_W$} {} tool{} still on legacy images; \
                     run `bv add <tool>` to migrate once registry rebuilds are available",
                    "",
                    legacy_count.if_supports_color(Stream::Stderr, |t| t.yellow().to_string()),
                    if legacy_count == 1 { "" } else { "s" }
                );
            }
        }
    } else {
        kv_dim("bv.lock", "not found");
    }

    eprintln!();

    if blocking_issues > 0 {
        anyhow::bail!(
            "{} blocking issue{} found",
            blocking_issues,
            if blocking_issues == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

// Output helpers

fn section(title: &str) {
    eprintln!(
        "\n  {}",
        title.if_supports_color(Stream::Stderr, |t| t.bold().underline().to_string())
    );
}

fn kv(key: &str, value: &str) {
    eprintln!(
        "    {:<KEY_W$} {}",
        key.if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        value
    );
}

fn kv_dim(key: &str, value: &str) {
    eprintln!(
        "    {:<KEY_W$} {}",
        key.if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        value.if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
    );
}

fn check_ghcr_credentials() {
    let config_path = match dirs_config_json_path() {
        Some(p) => p,
        None => {
            kv_dim(
                "ghcr.io",
                "no ~/.docker/config.json; run `docker login ghcr.io` to pull private images",
            );
            return;
        }
    };

    if !config_path.exists() {
        kv_dim(
            "ghcr.io",
            "no ~/.docker/config.json; run `docker login ghcr.io` to pull private images",
        );
        return;
    }

    let configured = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .map(|json| ghcr_configured(&json))
        .unwrap_or(false);

    if !configured {
        kv_dim(
            "ghcr.io",
            "no ghcr.io credentials; run `docker login ghcr.io` to pull private images",
        );
    } else {
        kv_dim("ghcr.io", "credentials configured");
    }
}

fn dirs_config_json_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join(".docker")
            .join("config.json"),
    )
}

fn ghcr_configured(json: &Value) -> bool {
    if let Some(helpers) = json.get("credHelpers").and_then(|v| v.as_object())
        && helpers.contains_key("ghcr.io")
    {
        return true;
    }
    if json
        .get("credsStore")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        // A global creds store may hold ghcr.io; treat as configured.
        return true;
    }
    if let Some(auths) = json.get("auths").and_then(|v| v.as_object())
        && auths.contains_key("ghcr.io")
    {
        return true;
    }
    false
}

// Filesystem helpers

fn dir_size_bytes(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                dir_size_bytes(&p)
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

fn count_subdirs(path: &Path) -> usize {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .count()
}

fn git_remote_url(repo_path: &Path) -> Option<String> {
    crate::registry::git_remote_url(repo_path)
}
