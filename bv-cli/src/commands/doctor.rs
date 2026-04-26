use std::path::Path;

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::hardware::DetectedHardware;
use bv_core::project::BvLock;
use bv_runtime::{ContainerRuntime, DockerRuntime};

use crate::commands::add::format_size;

// Width of the left "key" column inside each section.
const KEY_W: usize = 10;

pub fn run() -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let mut blocking_issues = 0;

    section("Runtime");
    match DockerRuntime.health_check() {
        Ok(info) => {
            let server = info
                .extra
                .get("server_version")
                .map(|v| format!("  server {}", v.dimmed()))
                .unwrap_or_default();
            kv("docker", &format!("{}{}", info.version, server));
        }
        Err(e) => {
            kv_err("docker", "cannot connect to daemon");
            eprintln!(
                "    {:KEY_W$} {e}",
                ""
            );
            eprintln!(
                "    {:KEY_W$} {} {}",
                "",
                "is Docker Desktop running? try".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
                "open -a Docker".if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
            );
            blocking_issues += 1;
        }
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
            kv("gpu", &format!("{} ({:.0} GB VRAM{})", gpu.name, vram_gb, cuda));
        }
    }

    section("Cache");
    let cache = CacheLayout::new();
    let root = cache.root().clone();
    kv("path", &root.display().to_string());

    let total = dir_size_bytes(&root);
    let image_count = {
        let images_dir = cache.image_dir("");
        let parent = images_dir.parent().unwrap_or(&root);
        count_subdirs(parent)
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
        for entry in std::fs::read_dir(index_parent).into_iter().flatten().flatten() {
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

    section("Project");
    let bv_toml_path = cwd.join("bv.toml");
    let bv_lock_path = cwd.join("bv.lock");

    if bv_toml_path.exists() {
        // Count declared tools.
        let n = bv_core::project::BvToml::from_path(&bv_toml_path)
            .map(|t| t.tools.len())
            .unwrap_or(0);
        kv("bv.toml", &format!("{} tool{} declared", n, if n == 1 { "" } else { "s" }));
    } else {
        kv_dim("bv.toml", "not found in current directory");
    }

    if bv_lock_path.exists() {
        let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;
        let tools: Vec<_> = {
            let mut v: Vec<_> = lockfile.tools.values().collect();
            v.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));
            v.iter().map(|e| format!("{} {}", e.tool_id, e.version)).collect()
        };
        kv("bv.lock", &tools.join(", "));
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

fn kv_err(key: &str, value: &str) {
    eprintln!(
        "    {:<KEY_W$} {}",
        key.if_supports_color(Stream::Stderr, |t| t.red().bold().to_string()),
        value.if_supports_color(Stream::Stderr, |t| t.red().to_string())
    );
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
            if p.is_dir() { dir_size_bytes(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }
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
    let out = std::process::Command::new("git")
        .args(["-C", &repo_path.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
