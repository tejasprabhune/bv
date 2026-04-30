use std::path::PathBuf;

use anyhow::{Context, Result};
use bv_builder::{
    build::{self},
    layering::PackingStrategy,
    oci,
    popularity::{self, PopularityMap},
    registry,
    resolve,
    spec::BuildSpec,
};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "bv-builder",
    about = "Build reproducible factored OCI images from conda package specs"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Resolve a spec to a fully-pinned package list.
    Resolve {
        /// Path to the build spec YAML.
        spec: PathBuf,
        /// Write resolved spec JSON to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Build a factored OCI image from a spec.
    Build {
        /// Path to the build spec YAML.
        spec: PathBuf,
        /// Write the OCI image as a tarball to this path.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Maximum number of OCI layers (enables popularity-based packing).
        #[arg(long, default_value = "0")]
        max_layers: usize,
        /// Path to a popularity.json file produced by `bv-builder pack`.
        #[arg(long)]
        popularity: Option<PathBuf>,
    },
    /// Push a built OCI image to a registry.
    Push {
        /// OCI tarball produced by `build`.
        image: PathBuf,
        /// Full registry reference, e.g. `ghcr.io/owner/repo:tag`.
        reference: String,
    },
    /// Fetch and verify a pushed image's digest.
    Verify {
        /// Full registry reference with digest, e.g. `registry/repo@sha256:…`.
        reference: String,
        /// Expected manifest digest (`sha256:…`).
        #[arg(long)]
        digest: String,
    },
    /// Compute package popularity from all specs in a registry specs directory.
    ///
    /// Walks `<specs-dir>/**/*.yaml`, counts package co-occurrences, and writes
    /// a `popularity.json` that `bv-builder build --popularity` reads to decide
    /// which packages get their own OCI layer vs. the shared long-tail layer.
    Pack {
        /// Root of the `specs/` directory in the bv-registry repo.
        specs_dir: PathBuf,
        /// Write the popularity map to this path (default: popularity.json).
        #[arg(long, default_value = "popularity.json")]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Resolve { spec, out } => {
            let build_spec = load_spec(&spec)?;
            eprintln!("  Resolving {} {}...", build_spec.name, build_spec.version);
            let resolved = resolve::resolve(&build_spec)
                .await
                .context("resolve packages")?;
            eprintln!(
                "  Resolved {} packages",
                resolved.packages.len()
            );
            let json = serde_json::to_string_pretty(&resolved)?;
            if let Some(out) = out {
                std::fs::write(&out, &json)
                    .with_context(|| format!("write resolved spec to {}", out.display()))?;
                eprintln!("  Written to {}", out.display());
            } else {
                println!("{json}");
            }
        }

        Commands::Build { spec, output, max_layers, popularity: pop_path } => {
            let build_spec = load_spec(&spec)?;
            eprintln!("  Resolving {} {}...", build_spec.name, build_spec.version);
            let resolved = resolve::resolve(&build_spec)
                .await
                .context("resolve packages")?;

            let pop_map: Option<PopularityMap> = pop_path
                .as_ref()
                .map(|p| PopularityMap::load(p))
                .transpose()
                .context("load popularity map")?;

            let strategy = if max_layers > 0 {
                PackingStrategy::PopularityBased { max_layers }
            } else {
                PackingStrategy::OnePerPackage
            };

            eprintln!(
                "  Building {} layers...",
                resolved.packages.len()
            );
            let image = build::build(&resolved, &strategy, pop_map.as_ref())
                .await
                .context("build OCI image")?;

            let manifest = image.manifest_json()?;
            let manifest_digest = format!(
                "sha256:{}",
                build::sha256_hex(&manifest)
            );
            eprintln!("  Manifest digest: {manifest_digest}");
            eprintln!("  Layers: {}", image.layers.len());

            if let Some(out) = output {
                save_oci_tarball(&image, &out)?;
                eprintln!("  Written to {}", out.display());
            }

            let snapshot = registry::build_repodata_snapshot(&resolved)?;
            let snapshot_digest = format!(
                "sha256:{}",
                build::sha256_hex(&snapshot)
            );
            eprintln!("  Repodata snapshot digest: {snapshot_digest}");
        }

        Commands::Push { image, reference } => {
            eprintln!("  Loading tarball from {}...", image.display());
            let loaded = oci::load_from_tarball(&image).context("load OCI tarball")?;
            eprintln!("  Pushing {} layers to {reference}...", loaded.layers.len());
            let digest = oci::push(&loaded, &reference).await.context("push image")?;
            eprintln!("  Pushed: {digest}");
            std::fs::write("/tmp/push-digest.txt", &digest)
                .context("write /tmp/push-digest.txt")?;
        }

        Commands::Verify { reference, digest } => {
            eprintln!("  Verifying {reference}...");
            oci::verify(&reference, &digest)
                .await
                .context("verify image digest")?;
            eprintln!("  Digest verified: {digest}");
        }

        Commands::Pack { specs_dir, output } => {
            eprintln!("  Scanning specs in {}...", specs_dir.display());
            let pop = popularity::compute_from_spec_dir(&specs_dir)
                .context("compute popularity from spec directory")?;
            let total: u64 = pop.packages.values().sum();
            eprintln!(
                "  {} unique packages, {} total occurrences",
                pop.packages.len(),
                total
            );
            pop.save(&output)
                .with_context(|| format!("write popularity map to {}", output.display()))?;
            eprintln!("  Written to {}", output.display());
        }
    }

    Ok(())
}

fn load_spec(path: &PathBuf) -> Result<BuildSpec> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("read spec '{}'", path.display()))?;
    serde_yaml::from_str(&s).with_context(|| format!("parse spec '{}'", path.display()))
}

fn save_oci_tarball(image: &bv_builder::build::OciImage, path: &PathBuf) -> Result<()> {
    let f = std::fs::File::create(path)
        .with_context(|| format!("create {}", path.display()))?;
    let mut builder = tar::Builder::new(f);

    // Write each layer blob at blobs/sha256/<hex>.
    // Deduplicate: multiple layers can share the same digest (e.g. empty
    // packages all produce an identical layer). Write each blob only once.
    let mut written: std::collections::HashSet<String> = std::collections::HashSet::new();
    for layer in &image.layers {
        let hex = layer.descriptor.digest.strip_prefix("sha256:").unwrap_or(&layer.descriptor.digest);
        if !written.insert(hex.to_string()) {
            continue;
        }
        let entry_path = format!("blobs/sha256/{hex}");
        let mut header = tar::Header::new_ustar();
        header.set_size(layer.compressed.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(
            &mut header,
            &entry_path,
            layer.compressed.as_slice(),
        )?;
    }

    // Write config.
    let config_hex = build::sha256_hex(&image.config);
    let mut ch = tar::Header::new_ustar();
    ch.set_size(image.config.len() as u64);
    ch.set_mode(0o644);
    ch.set_cksum();
    builder.append_data(
        &mut ch,
        format!("blobs/sha256/{config_hex}"),
        image.config.as_slice(),
    )?;

    // Write manifest.json.
    let manifest = image.manifest_json()?;
    let mut mh = tar::Header::new_ustar();
    mh.set_size(manifest.len() as u64);
    mh.set_mode(0o644);
    mh.set_cksum();
    builder.append_data(&mut mh, "manifest.json", manifest.as_slice())?;

    builder.finish()?;
    Ok(())
}
