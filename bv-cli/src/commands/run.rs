use std::path::PathBuf;

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::cache::CacheLayout;
use bv_core::manifest::Manifest;
use bv_core::project::BvLock;
use bv_runtime::{ContainerRuntime, GpuProfile, Mount, OciRef, RunSpec};

pub async fn run(tool: &str, args: &[String], backend: Option<&str>) -> anyhow::Result<()> {
    // trailing_var_arg passes the literal "--" through; strip it so
    // `bv run blast -- blastn -version` keeps working alongside
    // `bv run blastn -version`.
    let args: &[String] = args
        .first()
        .filter(|a| a.as_str() == "--")
        .map(|_| &args[1..])
        .unwrap_or(args);

    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        anyhow::bail!(
            "no bv.lock found in this directory\n\
             run `bv add {tool}` first"
        );
    }

    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    // Resolve: first try as a tool id, then as a binary name.
    let (tool_id, binary_override) = if lockfile.tools.contains_key(tool) {
        (tool.to_string(), None)
    } else if let Some(resolved) = lockfile.binary_index.get(tool) {
        (resolved.clone(), Some(tool.to_string()))
    } else {
        anyhow::bail!(
            "no tool or binary named '{tool}' in this project.\n  \
             Run `bv list --binaries` to see available binaries, or `bv add {tool}` to add it."
        );
    };

    let entry = lockfile.tools.get(&tool_id).ok_or_else(|| {
        anyhow::anyhow!("Tool '{tool_id}' is not in this project. Run `bv add {tool_id}` first.")
    })?;

    // Load the cached manifest to get the entrypoint.
    let cache = CacheLayout::new();
    let manifest_path = cache.manifest_path(&tool_id, &entry.version);
    let manifest = if manifest_path.exists() {
        let s = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read cached manifest for '{tool_id}'"))?;
        Manifest::from_toml_str(&s)?
    } else {
        // Manifest was evicted from cache (e.g. user deleted ~/.cache/bv).
        // Recover it from the local git index clone without a network request.
        let index_manifest = cache
            .index_dir("default")
            .join("tools")
            .join(&tool_id)
            .join(format!("{}.toml", entry.version));

        if index_manifest.exists() {
            let s = std::fs::read_to_string(&index_manifest)
                .with_context(|| format!("failed to read index manifest for '{tool_id}'"))?;
            let m = Manifest::from_toml_str(&s)?;
            crate::commands::add::cache_manifest(&cache, &m)
                .with_context(|| format!("failed to restore cached manifest for '{tool_id}'"))?;
            m
        } else {
            anyhow::bail!(
                "Manifest for '{tool_id}@{}' is not in cache or local index.\n\
                 Run `bv add {tool_id}` to restore it.",
                entry.version
            );
        }
    };

    // Build the OciRef with pinned digest for reproducible execution.
    // Keep the tag; it must agree with what `bv sync` pulls (sync.rs also
    // keeps the tag) so the apptainer backend, which keys SIF lookups by
    // tag-context rather than registry digest, can find the local image.
    let mut image: OciRef = entry
        .image_reference
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid image ref in lockfile: {e}"))?;
    image.digest = Some(entry.image_digest.clone());

    // Decide command. Cases (in order):
    //   * binary alias  (`bv run blastn -query x`)   -> [binary, ...args]
    //   * tool id + matching subcommand
    //         (`bv run genie2 train --devices 1`)    -> subcommands[name] + ...rest
    //   * tool id, no args, no entrypoint            -> print available subcommands
    //   * tool id, no args, with entrypoint          -> [entrypoint.command]
    //   * tool id, with args, with entrypoint
    //         (`bv run bcftools view ...`)           -> [entrypoint.command, ...args]
    //   * tool id, with args, subcommands declared but no match
    //         -> friendly error listing subcommands
    let command = if let Some(bin) = binary_override {
        let mut cmd = vec![bin];
        cmd.extend_from_slice(args);
        cmd
    } else if let Some(first) = args.first()
        && let Some(subcmd) = manifest.tool.subcommands.get(first)
    {
        let mut cmd = subcmd.clone();
        cmd.extend_from_slice(&args[1..]);
        cmd
    } else if !manifest.tool.subcommands.is_empty() && manifest.tool.entrypoint.is_none() {
        if args.is_empty() {
            print_subcommands(&tool_id, &manifest);
            return Ok(());
        }
        let mut names: Vec<&String> = manifest.tool.subcommands.keys().collect();
        names.sort();
        let list = names
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "'{tool_id}' has no subcommand '{}'.\n  \
             Available subcommands: {list}",
            args[0]
        );
    } else {
        let ep = manifest.tool.entrypoint.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "'{tool_id}' has no [tool.entrypoint] declared.\n  \
                 This manifest is malformed; `bv conformance` should reject it."
            )
        })?;
        let mut cmd = vec![ep.command.clone()];
        cmd.extend_from_slice(args);
        cmd
    };

    // Resolve runtime up front so the mount-building code can ask which backend
    // it's targeting (apptainer needs writable bind mounts for caches that
    // docker would otherwise satisfy via its writable upper layer).
    let bv_toml = bv_core::project::BvToml::from_path(&cwd.join("bv.toml")).ok();
    let runtime = crate::runtime_select::resolve_runtime(backend, bv_toml.as_ref())?;

    // Build reference data mounts; fail fast on missing required datasets.
    let mut mounts = vec![Mount {
        host_path: cwd.clone(),
        container_path: PathBuf::from("/workspace"),
        read_only: false,
    }];
    mounts.extend(crate::mounts::cache_mounts(
        &tool_id,
        runtime.name(),
        &manifest.tool,
        bv_toml.as_ref(),
    )?);
    let mut missing_required: Vec<String> = Vec::new();
    for (key, spec) in &manifest.tool.reference_data {
        let data_dir = cache.data_dir(&spec.id, &spec.version);
        if data_dir.exists() {
            if let Some(mount_path) = &spec.mount_path {
                mounts.push(Mount {
                    host_path: data_dir,
                    container_path: PathBuf::from(mount_path),
                    read_only: true,
                });
            }
        } else if spec.required {
            missing_required.push(format!("{}@{}", spec.id, spec.version));
            eprintln!(
                "  {} reference data '{key}' not found in cache",
                "error:".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
            );
        } else {
            eprintln!(
                "  {} optional reference data '{key}' not downloaded; skipping",
                "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string())
            );
        }
    }
    if !missing_required.is_empty() {
        let fetch_args = missing_required
            .iter()
            .map(|s| s.split('@').next().unwrap_or(s).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        anyhow::bail!(
            "required reference data missing\n  \
             Run: bv data fetch {fetch_args}"
        );
    }

    let spec = RunSpec {
        image,
        command,
        env: manifest
            .tool
            .entrypoint
            .as_ref()
            .map(|e| e.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
        mounts,
        gpu: GpuProfile {
            spec: manifest.tool.hardware.gpu.clone(),
        },
        working_dir: Some(PathBuf::from("/workspace")),
        capture_output: false,
    };

    runtime
        .health_check()
        .map_err(|e| anyhow::anyhow!("runtime not available: {e}"))?;

    // Check the pinned image is locally present; if not, guide the user to bv sync.
    let base_ref = crate::ops::base_image_ref(&entry.image_reference);
    if !runtime.is_locally_available(&base_ref, &entry.image_digest) {
        anyhow::bail!(
            "Image for '{tool_id}@{}' is not available locally.\n  \
             Run `bv sync` to pull it.",
            entry.version
        );
    }

    let outcome = runtime
        .run(&spec)
        .with_context(|| format!("failed to run '{tool_id}'"))?;

    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }

    Ok(())
}

fn print_subcommands(tool_id: &str, manifest: &Manifest) {
    eprintln!(
        "  {} {}",
        "Tool".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        tool_id
    );
    eprintln!(
        "  {}",
        "Available subcommands:".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
    let mut entries: Vec<(&String, &Vec<String>)> = manifest.tool.subcommands.iter().collect();
    entries.sort_by_key(|(k, _)| k.as_str());
    let max_name = entries.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (name, cmd) in &entries {
        eprintln!("    {:width$}  {}", name, cmd.join(" "), width = max_name);
    }
    eprintln!(
        "\n  Run with: {} {tool_id} {}",
        "bv run".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        "<subcommand> [args...]".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
    );
}

pub fn info(tool: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        anyhow::bail!("no bv.lock found; run `bv add {tool}` first");
    }

    let lockfile = BvLock::from_path(&bv_lock_path)?;
    let entry = lockfile
        .tools
        .get(tool)
        .ok_or_else(|| anyhow::anyhow!("Tool '{tool}' is not in this project."))?;

    let cache = CacheLayout::new();
    let manifest_path = cache.manifest_path(tool, &entry.version);

    println!("Tool:      {}", entry.tool_id);
    println!("Version:   {}", entry.version);
    println!("Image:     {}", entry.image_reference);
    println!("Digest:    {}", entry.image_digest);
    if let Some(sz) = entry.image_size_bytes {
        println!("Size:      {}", crate::commands::add::format_size(sz));
    }
    println!(
        "Locked:    {}",
        entry.resolved_at.format("%Y-%m-%d %H:%M UTC")
    );
    println!("Manifest:  {}", manifest_path.display());

    if manifest_path.exists() {
        let s = std::fs::read_to_string(&manifest_path)?;
        if let Ok(m) = Manifest::from_toml_str(&s) {
            if let Some(desc) = &m.tool.description {
                println!("About:     {desc}");
            }
            if let Some(hp) = &m.tool.homepage {
                println!("Homepage:  {hp}");
            }
            if let Some(ep) = &m.tool.entrypoint {
                println!("Entrypoint: {}", ep.command);
                if let Some(tmpl) = &ep.args_template {
                    println!("Args template: {tmpl}");
                }
            }
            if !m.tool.subcommands.is_empty() {
                let mut names: Vec<&String> = m.tool.subcommands.keys().collect();
                names.sort();
                println!(
                    "Subcommands: {}",
                    names
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    } else {
        eprintln!(
            "{}",
            "(manifest not cached; run `bv add` to refresh)"
                .if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
    }

    Ok(())
}
