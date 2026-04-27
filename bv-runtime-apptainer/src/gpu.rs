use bv_runtime::GpuProfile;

/// Return `--nv` when the tool requires a GPU.
pub fn nv_args(profile: &GpuProfile) -> Vec<String> {
    match &profile.spec {
        Some(spec) if spec.required => vec!["--nv".to_string()],
        _ => vec![],
    }
}
