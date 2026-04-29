use clap::{ArgAction, Parser, Subcommand};

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
        /// Allow adding tools that are marked `experimental` in the registry.
        #[arg(long)]
        allow_experimental: bool,
        /// Container backend: `docker`, `apptainer`, or `auto` (default).
        #[arg(long, env = "BV_BACKEND")]
        backend: Option<String>,
        /// Refuse to add tools that do not carry a Sigstore image signature.
        #[arg(long)]
        require_signed: bool,
    },

    /// Verify a tool manifest against the conformance test suite.
    ///
    /// With no tool argument, walks every tool in the registry and prints a
    /// summary table. Useful as a one-shot registry health check.
    Conformance {
        /// Tool identifier. Omit to walk the entire registry.
        tool: Option<String>,
        /// Registry URL or local path. Overrides BV_REGISTRY env var.
        #[arg(long, env = "BV_REGISTRY")]
        registry: Option<String>,
        /// Container backend: `docker`, `apptainer`, or `auto`.
        #[arg(long, env = "BV_BACKEND")]
        backend: Option<String>,
        /// In walk mode: only run tools whose id contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// In walk mode: skip tools that require a GPU.
        #[arg(long)]
        skip_gpu: bool,
        /// In walk mode: skip tools that require reference data.
        #[arg(long)]
        skip_reference_data: bool,
    },

    /// Search the registry for tools.
    Search {
        /// Search query (matches tool id, description, and I/O types).
        query: String,
        /// Tier filter: `all`, `core`, `community`, or `experimental`.
        /// Default shows core and community tools.
        #[arg(long)]
        tier: Option<String>,
        /// Registry URL or local path. Overrides BV_REGISTRY env var.
        #[arg(long, env = "BV_REGISTRY")]
        registry: Option<String>,
        /// Maximum number of results to show.
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Remove a tool from bv.toml and bv.lock.
    Remove {
        /// Tool identifier.
        tool: String,
    },

    /// Run a tool or binary inside its container.
    ///
    /// bv flags (e.g. --backend) must come before the tool/binary name.
    /// Everything after the name is forwarded verbatim to the container,
    /// including flags like --help and --version.
    ///
    ///   bv run blastn -query foo.fa -db nr          (binary routing)
    ///   bv run blast -- blastn -query foo.fa         (name the tool explicitly)
    ///   bv run --backend apptainer blastn -version   (bv flag before name)
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Run {
        /// Print help for `bv run` (use before the tool/binary name).
        #[arg(short = 'h', action = ArgAction::Help, exclusive = true)]
        help: Option<bool>,
        /// Tool id or exposed binary name.
        tool: String,
        /// Container backend: `docker`, `apptainer`, or `auto` (default).
        #[arg(long, env = "BV_BACKEND")]
        backend: Option<String>,
        /// Arguments forwarded verbatim to the container entrypoint.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// List tools installed in this project.
    List {
        /// Show binary routing table instead of tool list.
        #[arg(long)]
        binaries: bool,
    },

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
        /// Container backend: `docker`, `apptainer`, or `auto` (default).
        #[arg(long, env = "BV_BACKEND")]
        backend: Option<String>,
    },

    /// Check that the environment is correctly configured.
    Doctor,

    /// Reference data management.
    #[command(subcommand)]
    Data(DataCommands),

    /// Local cache management.
    #[command(subcommand)]
    Cache(CacheCommands),

    /// Run a command with bv-managed binaries on PATH (for scripts and CI).
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Exec {
        /// Print help for `bv exec`.
        #[arg(short = 'h', action = ArgAction::Help, exclusive = true)]
        help: Option<bool>,
        /// Command to run.
        command: String,
        /// Arguments forwarded to the command.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Spawn an interactive subshell with bv-managed binaries on PATH.
    Shell {
        /// Shell to spawn (defaults to $SHELL).
        #[arg(long)]
        shell: Option<String>,
    },

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
        /// GHCR namespace to push the image into. Defaults to your own GitHub username
        /// (e.g. `ghcr.io/<you>/<tool>:<ver>`), so you don't need org permissions to publish.
        /// Override with e.g. `--push-to bv-registry` if you have write access to that org.
        #[arg(long)]
        push_to: Option<String>,
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
