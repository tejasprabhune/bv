use clap::{Parser, Subcommand};

use crate::commands::show::ShowFormat;


#[derive(Parser)]
#[command(
    name = "bv",
    about = "A fast, project-scoped tool manager for bioinformatics",
    version,
    propagate_version = true
)]
pub struct Cli {
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Add one or more tools to bv.toml (and pull their images).
    Add {
        /// Tool identifiers, optionally with version (e.g. `blast blast@2.15.0 hmmer`).
        #[arg(required = true)]
        tools: Vec<String>,
        /// Registry URL or local path. Overrides BV_REGISTRY env var and bv.toml.
        #[arg(long, env = "BV_REGISTRY")]
        registry: Option<String>,
        /// Skip hardware requirement checks (useful on dev Macs for GPU tools).
        #[arg(long)]
        ignore_hardware: bool,
    },

    /// Remove a tool from bv.toml and bv.lock.
    Remove {
        /// Tool identifier.
        tool: String,
    },

    /// Run a tool inside its container.
    Run {
        /// Tool identifier.
        tool: String,
        /// Arguments forwarded verbatim to the container entrypoint.
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// List tools installed in this project.
    List,

    /// Show typed I/O schema and metadata for a tool.
    Show {
        /// Tool identifier.
        tool: String,
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<ShowFormat>,
    },

    /// Show detailed lockfile information about an installed tool.
    Info {
        /// Tool identifier.
        tool: String,
    },

    /// Resolve bv.toml and write (or check) bv.lock.
    Lock {
        /// Exit 1 if bv.lock would change; useful in CI.
        #[arg(long)]
        check: bool,
        /// Registry URL or local path. Overrides BV_REGISTRY env var and bv.toml.
        #[arg(long, env = "BV_REGISTRY")]
        registry: Option<String>,
    },

    /// Pull every image in bv.lock, making the project runnable offline.
    Sync {
        /// Fail if bv.toml and bv.lock are not consistent with each other.
        #[arg(long)]
        frozen: bool,
        /// Registry URL (used only for drift detection; optional).
        #[arg(long, env = "BV_REGISTRY")]
        registry: Option<String>,
    },

    /// Check that the environment is correctly configured.
    Doctor,

    /// Reference data management.
    #[command(subcommand)]
    Data(DataCommands),

    /// Local cache management.
    #[command(subcommand)]
    Cache(CacheCommands),

    /// Build and publish a tool to bv-registry (opens a PR).
    Publish {
        /// Local directory or GitHub repo, e.g. `./my-tool` or `github:user/repo` or `github:user/repo@v2.0`.
        source: String,
        /// Tool name override (also settable in bv-publish.toml).
        #[arg(long)]
        tool_name: Option<String>,
        /// Version override (also settable in bv-publish.toml).
        #[arg(long)]
        tool_version: Option<String>,
        /// Skip all interactive prompts; requires bv-publish.toml or explicit flags.
        #[arg(long)]
        non_interactive: bool,
        /// Build the image but do not push to GHCR.
        #[arg(long)]
        no_push: bool,
        /// Push the image but do not open a PR.
        #[arg(long)]
        no_pr: bool,
        /// GitHub PAT with `repo` and `write:packages` scopes. Reads GITHUB_TOKEN env var.
        #[arg(long, env = "GITHUB_TOKEN")]
        github_token: Option<String>,
        /// GHCR token override. Falls back to --github-token when absent.
        #[arg(long, env = "GHCR_TOKEN")]
        ghcr_token: Option<String>,
        /// Target registry repository in `owner/repo` form.
        #[arg(long, default_value = "mlberkeley/bv-registry")]
        registry_repo: String,
        /// Docker build platform.
        #[arg(long, default_value = "amd64")]
        platform: String,
    },
}

#[derive(Subcommand)]
pub enum DataCommands {
    /// Download one or more reference datasets from the registry.
    Fetch {
        /// Dataset identifiers, optionally with version (e.g. `pdbaa pdbaa@2024_01`).
        #[arg(required = true)]
        datasets: Vec<String>,
        /// Registry URL or local path. Overrides BV_REGISTRY env var and bv.toml.
        #[arg(long, env = "BV_REGISTRY")]
        registry: Option<String>,
        /// Skip the size confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List reference datasets present in the local cache.
    List,
}

#[derive(Subcommand)]
pub enum CacheCommands {
    /// List (and optionally remove) images not referenced by any bv.lock.
    Prune {
        /// Actually remove images (default: dry run).
        #[arg(long)]
        apply: bool,
    },
}
