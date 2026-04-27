pub mod auth;
pub mod build;
pub mod detect;
pub mod pr;
pub mod scaffold;
pub mod source;

use anyhow::Context;
use bv_runtime::ContainerRuntime as _;
use owo_colors::{OwoColorize, Stream};

pub struct PublishOpts {
    pub source: String,
    pub tool_name: Option<String>,
    pub version: Option<String>,
    pub non_interactive: bool,
    pub no_push: bool,
    pub no_pr: bool,
    pub github_token: Option<String>,
    pub ghcr_token: Option<String>,
    pub registry_repo: String,
    /// GHCR namespace to push to. `None` defaults to the authenticated GitHub user's
    /// own namespace, so third-party publishers don't need org write-access.
    pub push_to: Option<String>,
    pub platform: String,
}

pub async fn run(opts: PublishOpts) -> anyhow::Result<()> {
    bv_runtime::DockerRuntime
        .health_check()
        .context("Docker is not available. Is Docker Desktop running?")?;

    // Parse source spec and fetch to a local directory.
    let src = source::Source::parse(&opts.source)?;
    let fetched = src.fetch()?;

    eprintln!(
        "  {} {}",
        "Source".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        fetched.source_url
    );

    // Detect build system.
    let build_sys = detect::detect(&fetched.dir);
    eprintln!(
        "  {} {}",
        "Detected".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
        build_sys.description()
    );

    // Load bv-publish.toml if present.
    let config = scaffold::load_publish_config(&fetched.dir);

    // Collect manifest metadata.
    let scaffold_result = if opts.non_interactive {
        scaffold::from_config(
            config.as_ref(),
            &fetched,
            opts.tool_name.as_deref(),
            opts.version.as_deref(),
        )?
    } else {
        scaffold::interactive(
            config.as_ref(),
            &fetched,
            opts.tool_name.as_deref(),
            opts.version.as_deref(),
        )?
    };

    // Ensure a Dockerfile exists (generating one if needed).
    let dockerfile = detect::ensure_dockerfile(&build_sys, &fetched.dir)?;

    eprintln!();

    // Dry run: print the manifest with a placeholder ref and stop. No auth needed.
    if opts.no_push && opts.no_pr {
        let placeholder = opts.push_to.as_deref().unwrap_or("<your-github-username>");
        let image_ref = format!(
            "ghcr.io/{}/{}:{}",
            placeholder, scaffold_result.name, scaffold_result.version
        );
        let manifest_toml = scaffold_result.to_manifest_toml(&image_ref, "")?;
        eprintln!("  {}", bold("Manifest (draft, no push):"));
        for line in manifest_toml.lines() {
            eprintln!("    {}", line);
        }
        return Ok(());
    }

    // Resolve tokens.
    let github_token =
        auth::resolve_github_token(opts.github_token.as_deref(), opts.non_interactive)?;
    let ghcr_token = auth::resolve_ghcr_token(opts.ghcr_token.as_deref(), &github_token);

    // Get GitHub username for docker login (and to use as the default GHCR namespace).
    let github_username = pr::get_github_username(&github_token).await?;

    // By default, push to the authenticated user's own GHCR namespace. This means
    // third-party publishers don't need write access to any shared org: their PR
    // proposes a manifest pointing at `ghcr.io/<them>/<tool>`. Override with --push-to.
    let namespace = opts.push_to.as_deref().unwrap_or(&github_username);
    let image_ref = format!(
        "ghcr.io/{}/{}:{}",
        namespace, scaffold_result.name, scaffold_result.version
    );

    // Build and push.
    let digest = if opts.no_push {
        eprintln!(
            "  {} image push (--no-push)",
            "Skipping".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
        String::new()
    } else {
        build::build_and_push(
            &fetched.dir,
            &dockerfile,
            &image_ref,
            &ghcr_token,
            &github_username,
            &opts.platform,
        )?
    };

    if !digest.is_empty() {
        eprintln!(
            "  {} {}",
            "Digest".if_supports_color(Stream::Stderr, |t| t.cyan().bold().to_string()),
            &digest[..digest.len().min(20)]
        );
    }

    let manifest_toml = scaffold_result.to_manifest_toml(&image_ref, &digest)?;

    eprintln!("\n  {}", bold("Manifest:"));
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

    eprintln!();
    let pr_url = pr::open_pr(pr::PrContext {
        tool_name: &scaffold_result.name,
        version: &scaffold_result.version,
        manifest_toml: &manifest_toml,
        github_token: &github_token,
        registry_repo: &opts.registry_repo,
        source_url: &fetched.source_url,
    })
    .await?;

    eprintln!(
        "\n  {} {}",
        "PR opened:".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
        pr_url
    );

    Ok(())
}

fn bold(s: &str) -> String {
    format!(
        "{}",
        s.if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    )
}
