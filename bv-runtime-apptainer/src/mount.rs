use bv_runtime::Mount;

/// Convert a `Mount` into Apptainer `--bind src:dst[:ro]` argument pairs.
pub fn bind_args(mounts: &[Mount]) -> Vec<String> {
    mounts
        .iter()
        .flat_map(|m| {
            let spec = if m.read_only {
                format!(
                    "{}:{}:ro",
                    m.host_path.display(),
                    m.container_path.display()
                )
            } else {
                format!("{}:{}", m.host_path.display(), m.container_path.display())
            };
            ["--bind".to_string(), spec]
        })
        .collect()
}
