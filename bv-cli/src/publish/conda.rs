use std::path::{Path, PathBuf};

use anyhow::Context;
use bv_builder::{
    build, catalog::LayerCatalog, layering::PackingStrategy, oci, resolve, spec::BuildSpec,
};
use owo_colors::{OwoColorize, Stream};

use super::pr::{self, PrContext};
use super::scaffold::{ScaffoldResult, load_publish_config};
use super::source::FetchedSource;
use super::{auth, scaffold};

const CATALOG_REGISTRY_PATH: &str = "layers/catalog.json";
const MAX_LAYERS: usize = 125;

pub struct CondaPublishOpts {
    pub spec: PathBuf,
    /// Directory to search for bv-publish.toml (usually ".").
    pub source_dir: PathBuf,
    pub tool_name: Option<String>,
    pub version: Option<String>,
    pub non_interactive: bool,
    pub no_push: bool,
    pub no_pr: bool,
    pub github_token: Option<String>,
    pub ghcr_token: Option<String>,
    pub registry_repo: String,
    pub push_to: Option<String>,
}

pub async fn run(opts: CondaPublishOpts) -> anyhow::Result<()> {
    let spec_content = std::fs::read_to_string(&opts.spec)
        .with_context(|| format!("read spec '{}'", opts.spec.display()))?;
    let build_spec: BuildSpec = toml::from_str(&spec_content)
        .with_context(|| format!("parse spec '{}'", opts.spec.display()))?;

    eprintln!(
        "  {} {} {}",
        "Spec".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        build_spec.name,
        build_spec.version
    );

    let config = load_publish_config(&opts.source_dir);

    let scaffold_result = collect_metadata(
        &build_spec,
        config.as_ref(),
        opts.tool_name.as_deref(),
        opts.version.as_deref(),
        &opts.source_dir,
        opts.non_interactive,
    )?;

    let github_token =
        auth::resolve_github_token(opts.github_token.as_deref(), opts.non_interactive)?;
    let ghcr_token = auth::resolve_ghcr_token(opts.ghcr_token.as_deref(), &github_token);

    let github_username = pr::get_github_username(&github_token).await?;
    let namespace = opts.push_to.as_deref().unwrap_or(&github_username);
    let image_ref = format!(
        "ghcr.io/{}/{}:{}",
        namespace, scaffold_result.name, scaffold_result.version
    );

    eprintln!(
        "  {} {}...",
        "Resolving".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        build_spec.name
    );
    let resolved = resolve::resolve(&build_spec)
        .await
        .context("resolve conda packages")?;
    eprintln!(
        "  {} {} packages",
        "Resolved".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        resolved.packages.len()
    );

    let (catalog, catalog_json_original) = if opts.no_push || opts.no_pr {
        (LayerCatalog::new(), String::new())
    } else {
        fetch_catalog(&github_token, &opts.registry_repo).await?
    };

    let (hits, misses) = build::catalog_coverage(&resolved.packages, &catalog);
    if hits > 0 {
        eprintln!(
            "  {} {}/{} packages already have registry blobs",
            "Catalog".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
            hits,
            hits + misses
        );
    }

    let strategy = PackingStrategy::CatalogAware {
        max_layers: MAX_LAYERS,
    };

    eprintln!(
        "  {} {} layers...",
        "Building".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        resolved.packages.len()
    );
    let image = build::build(&resolved, &strategy, None, Some(&catalog))
        .await
        .context("build OCI image")?;

    eprintln!(
        "  {} {} layers  (manifest {})",
        "Built".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        image.layers.len(),
        &image
            .manifest_json()
            .map(|b| format!("sha256:{}", build::sha256_hex(&b)))
            .unwrap_or_else(|_| "?".into())[..20]
    );

    let digest = if opts.no_push {
        eprintln!(
            "  {} image push (--no-push)",
            "Skipping".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        String::new()
    } else {
        eprintln!(
            "  {} {} ...",
            "Pushing".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
            image_ref
        );
        oci::push_authenticated(&image, &image_ref, &ghcr_token)
            .await
            .with_context(|| format!("push image to {image_ref}"))?
    };

    if !digest.is_empty() {
        eprintln!(
            "  {} {}",
            "Digest".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
            &digest[..digest.len().min(20)]
        );
    }

    let updated_catalog_json = if !opts.no_push {
        let mut updated_catalog = catalog;
        let updates = build::catalog_updates_from_image(&image);
        let new_entries = updates.len();
        for (name, version, build_str, layer_digest) in updates {
            updated_catalog.record(name, version, build_str, layer_digest);
        }
        let json = updated_catalog
            .to_json()
            .context("serialize updated catalog")?;
        if json != catalog_json_original {
            eprintln!(
                "  {} {} new entries to catalog",
                "Added".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
                new_entries
            );
        }
        json
    } else {
        String::new()
    };

    let layers: Vec<bv_core::lockfile::LayerDescriptor> =
        image.layers.iter().map(|l| l.descriptor.clone()).collect();

    let manifest_toml = scaffold_result
        .to_conda_manifest_toml(&image_ref, &digest, &layers)
        .context("generate manifest TOML")?;

    eprintln!(
        "\n  {}",
        "Manifest:".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
    for line in manifest_toml.lines() {
        eprintln!("    {}", line);
    }

    if opts.no_pr {
        eprintln!(
            "\n  {} PR creation (--no-pr)",
            "Skipping".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        return Ok(());
    }

    let spec_registry_path = format!(
        "specs/{}/{}.toml",
        scaffold_result.name, scaffold_result.version
    );

    let mut extra_files = vec![(spec_registry_path, spec_content)];
    if !updated_catalog_json.is_empty() {
        extra_files.push((CATALOG_REGISTRY_PATH.to_string(), updated_catalog_json));
    }

    let source_url = opts.spec.display().to_string();
    let pr_url = pr::open_pr(PrContext {
        tool_name: &scaffold_result.name,
        version: &scaffold_result.version,
        manifest_toml: &manifest_toml,
        github_token: &github_token,
        registry_repo: &opts.registry_repo,
        source_url: &source_url,
        extra_files,
    })
    .await?;

    eprintln!(
        "\n  {} {}",
        "PR opened:".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
        pr_url
    );

    Ok(())
}

/// Fetch `layers/catalog.json` from the registry repo via GitHub raw content.
/// Returns `(catalog, original_json)`. If the file doesn't exist yet, returns
/// an empty catalog and an empty string (triggering creation in the PR).
async fn fetch_catalog(
    github_token: &str,
    registry_repo: &str,
) -> anyhow::Result<(LayerCatalog, String)> {
    let client = reqwest::Client::builder()
        .user_agent("bv-cli")
        .build()
        .context("build HTTP client")?;

    let url =
        format!("https://raw.githubusercontent.com/{registry_repo}/main/{CATALOG_REGISTRY_PATH}");

    let resp = client
        .get(&url)
        .header("Authorization", format!("token {github_token}"))
        .send()
        .await
        .with_context(|| format!("fetch catalog from {url}"))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok((LayerCatalog::new(), String::new()));
    }

    if !resp.status().is_success() {
        anyhow::bail!("failed to fetch catalog from {url}: HTTP {}", resp.status());
    }

    let json = resp.text().await.context("read catalog response")?;
    let catalog: LayerCatalog =
        serde_json::from_str(&json).context("parse registry catalog.json")?;
    Ok((catalog, json))
}

fn collect_metadata(
    build_spec: &BuildSpec,
    config: Option<&scaffold::PublishConfig>,
    name_override: Option<&str>,
    version_override: Option<&str>,
    source_dir: &Path,
    non_interactive: bool,
) -> anyhow::Result<ScaffoldResult> {
    let fetched = FetchedSource::local_dir(
        source_dir.to_path_buf(),
        build_spec.name.clone(),
        Some(build_spec.version.clone()),
    );

    if non_interactive {
        scaffold::from_config(config, &fetched, name_override, version_override, None)
    } else {
        scaffold::interactive(config, &fetched, name_override, version_override, None)
    }
}
