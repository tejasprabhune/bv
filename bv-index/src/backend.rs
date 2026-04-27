use bv_core::data::DataManifest;
use bv_core::error::Result;
use bv_core::manifest::Manifest;
use semver::{Version, VersionReq};

#[derive(Debug, Clone)]
pub struct ToolSummary {
    pub id: String,
    pub latest_version: String,
    pub description: Option<String>,
    pub tier: bv_core::manifest::Tier,
    pub deprecated: bool,
    pub input_types: Vec<String>,
    pub output_types: Vec<String>,
}

pub trait IndexBackend {
    fn name(&self) -> &str;

    /// Refresh the local copy of the index (clone or pull).
    fn refresh(&self) -> Result<()>;

    /// Resolve the best manifest matching `version` for `tool`.
    fn get_manifest(&self, tool: &str, version: &VersionReq) -> Result<Manifest>;

    /// List all available versions of `tool`.
    fn list_versions(&self, tool: &str) -> Result<Vec<Version>>;

    /// List all tools in this index.
    fn list_tools(&self) -> Result<Vec<ToolSummary>>;

    /// Resolve the data manifest for `dataset`. `version` is the exact version string
    /// (e.g. `"2024_01"`); pass `None` to get the lexicographically latest version.
    fn get_data_manifest(&self, dataset: &str, version: Option<&str>) -> Result<DataManifest>;

    /// List all available version strings for a dataset (lexicographically sorted).
    fn list_data_versions(&self, dataset: &str) -> Result<Vec<String>>;
}
