use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

use bv_core::manifest::ToolManifest;
use bv_core::project::BvToml;
use bv_runtime::Mount;

/// Container paths bv binds writable on apptainer when neither the manifest
/// nor `bv.toml` declares anything for them. These are the de-facto cache
/// roots used by the bulk of bioinformatics images, and apptainer's
/// read-only SIF root would otherwise block any tool that writes there.
///
/// Skipped on docker (writable upper layer covers the same need).
const APPTAINER_FALLBACK_CACHE_PATHS: &[&str] = &["/cache", "/root/.cache"];

/// Build the cache mounts for a `bv run` invocation, in precedence order:
///
/// 1. **Manifest** (`tool.cache_paths`) : the tool author's authoritative
///    list of paths that need writable backing.
/// 2. **User** (`[[cache]]` in `bv.toml`) : adds new paths or overrides the
///    host side of any path declared by the manifest.
/// 3. **Hardcoded apptainer fallbacks** : only on apptainer, only for
///    container paths nothing else has claimed.
///
/// All produced host directories are created if they don't already exist.
pub fn cache_mounts(
    tool_id: &str,
    backend: &str,
    manifest: &ToolManifest,
    bv_toml: Option<&BvToml>,
) -> Result<Vec<Mount>> {
    // We dedupe on container_path; later layers can override the host_path
    // of an earlier layer's entry but cannot remove it.
    let mut by_container: HashMap<String, PathBuf> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let cache_root = bv_cache_dir()?.join(tool_id);

    // 1. Manifest declarations.
    for cp in &manifest.cache_paths {
        let host = cache_root.join(slug_for(cp));
        if !by_container.contains_key(cp) {
            order.push(cp.clone());
        }
        by_container.insert(cp.clone(), host);
    }

    // 2. User overrides / additions.
    if let Some(toml) = bv_toml {
        for entry in &toml.caches {
            if !match_tool(&entry.tool_match, tool_id) {
                continue;
            }
            let host = expand_host(&entry.host_path, tool_id)?;
            if !by_container.contains_key(&entry.container_path) {
                order.push(entry.container_path.clone());
            }
            by_container.insert(entry.container_path.clone(), host);
        }
    }

    // 3. Apptainer fallbacks for tools that haven't declared anything yet.
    if backend == "apptainer" {
        for cp in APPTAINER_FALLBACK_CACHE_PATHS {
            if by_container.contains_key(*cp) {
                continue;
            }
            let host = cache_root.join(slug_for(cp));
            order.push(cp.to_string());
            by_container.insert(cp.to_string(), host);
        }
    }

    let mut out = Vec::with_capacity(order.len());
    for cp in order {
        let host = by_container.remove(&cp).expect("populated above");
        std::fs::create_dir_all(&host)
            .with_context(|| format!("failed to create cache dir {}", host.display()))?;
        out.push(Mount {
            host_path: host,
            container_path: PathBuf::from(cp),
            read_only: false,
        });
    }
    Ok(out)
}

fn match_tool(pattern: &str, tool: &str) -> bool {
    pattern == "*" || pattern == tool
}

/// Turn a container path into a filesystem-safe filename component.
/// `/cache/colabfold` -> `cache-colabfold`.
fn slug_for(container_path: &str) -> String {
    let trimmed = container_path.trim_start_matches('/');
    if trimmed.is_empty() {
        "root".to_string()
    } else {
        trimmed.replace('/', "-")
    }
}

/// Expand `~` and `{tool}` in a user-declared `host_path` template.
fn expand_host(template: &str, tool: &str) -> Result<PathBuf> {
    let mut s = template.replace("{tool}", tool);
    if s == "~" {
        s = home_dir()?;
    } else if let Some(rest) = s.strip_prefix("~/") {
        s = format!("{}/{rest}", home_dir()?);
    }
    Ok(PathBuf::from(s))
}

fn home_dir() -> Result<String> {
    std::env::var("HOME").map_err(|_| anyhow::anyhow!("$HOME is not set"))
}

fn bv_cache_dir() -> Result<PathBuf> {
    let base = if let Ok(d) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(d)
    } else {
        PathBuf::from(home_dir()?).join(".cache")
    };
    Ok(base.join("bv"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bv_core::manifest::{EntrypointSpec, HardwareSpec, ImageSpec, ToolManifest};
    use bv_core::project::{CacheMount, ProjectMeta};

    fn manifest_with(cache_paths: Vec<&str>) -> ToolManifest {
        ToolManifest {
            id: "colabfold".into(),
            version: "1.6.0".into(),
            description: None,
            homepage: None,
            license: None,
            tier: Default::default(),
            maintainers: vec![],
            deprecated: false,
            image: ImageSpec {
                backend: "docker".into(),
                reference: "ghcr.io/sokrypton/colabfold:1.6.0-cuda12".into(),
                digest: None,
            },
            hardware: HardwareSpec {
                gpu: None,
                cpu_cores: None,
                ram_gb: None,
                disk_gb: None,
            },
            reference_data: Default::default(),
            inputs: vec![],
            outputs: vec![],
            entrypoint: EntrypointSpec {
                command: "colabfold_batch".into(),
                args_template: None,
                env: Default::default(),
            },
            cache_paths: cache_paths.into_iter().map(String::from).collect(),
            binaries: None,
            smoke: None,
            signatures: None,
        }
    }

    fn toml_with(caches: Vec<CacheMount>) -> BvToml {
        BvToml {
            project: ProjectMeta {
                name: "t".into(),
                description: None,
            },
            registry: None,
            tools: vec![],
            data: Default::default(),
            hardware: Default::default(),
            runtime: Default::default(),
            binary_overrides: Default::default(),
            caches,
        }
    }

    fn cache(tool_match: &str, container: &str, host: &str) -> CacheMount {
        CacheMount {
            tool_match: tool_match.into(),
            container_path: container.into(),
            host_path: host.into(),
        }
    }

    // Tests mutate $HOME, which is process-global; serialize them.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let td = tempfile::tempdir().unwrap();
        let prev = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("HOME", td.path());
        }
        f();
        match prev {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn match_star_matches_anything() {
        assert!(match_tool("*", "colabfold"));
        assert!(match_tool("colabfold", "colabfold"));
        assert!(!match_tool("blast", "colabfold"));
    }

    #[test]
    fn host_path_interpolates_tool() {
        let p = expand_host("/tmp/{tool}/cache", "colabfold").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/colabfold/cache"));
    }

    #[test]
    fn slug_collapses_slashes() {
        assert_eq!(slug_for("/cache"), "cache");
        assert_eq!(slug_for("/cache/colabfold"), "cache-colabfold");
        assert_eq!(slug_for("/root/.cache"), "root-.cache");
    }

    #[test]
    fn manifest_declarations_become_mounts() {
        with_temp_home(|| {
            let m = manifest_with(vec!["/cache/colabfold"]);
            let mounts = cache_mounts("colabfold", "docker", &m, None).unwrap();
            assert_eq!(mounts.len(), 1);
            assert_eq!(mounts[0].container_path, PathBuf::from("/cache/colabfold"));
            assert!(mounts[0].host_path.ends_with("colabfold/cache-colabfold"));
        });
    }

    #[test]
    fn user_overrides_manifest_host_path() {
        with_temp_home(|| {
            let m = manifest_with(vec!["/cache/colabfold"]);
            let toml = toml_with(vec![cache(
                "colabfold",
                "/cache/colabfold",
                "~/shared-cache",
            )]);
            let mounts = cache_mounts("colabfold", "docker", &m, Some(&toml)).unwrap();
            assert_eq!(mounts.len(), 1);
            assert_eq!(mounts[0].container_path, PathBuf::from("/cache/colabfold"));
            assert!(mounts[0].host_path.ends_with("shared-cache"));
        });
    }

    #[test]
    fn apptainer_fallbacks_apply_when_unclaimed() {
        with_temp_home(|| {
            let m = manifest_with(vec![]);
            let mounts = cache_mounts("colabfold", "apptainer", &m, None).unwrap();
            let paths: Vec<_> = mounts.iter().map(|m| m.container_path.clone()).collect();
            assert!(paths.contains(&PathBuf::from("/cache")));
            assert!(paths.contains(&PathBuf::from("/root/.cache")));
        });
    }

    #[test]
    fn apptainer_fallbacks_skipped_when_manifest_claims_them() {
        with_temp_home(|| {
            let m = manifest_with(vec!["/cache"]);
            let mounts = cache_mounts("colabfold", "apptainer", &m, None).unwrap();
            // /cache appears once (from manifest), /root/.cache from fallback.
            let cache_count = mounts
                .iter()
                .filter(|x| x.container_path == PathBuf::from("/cache"))
                .count();
            assert_eq!(cache_count, 1);
        });
    }

    #[test]
    fn docker_skips_fallbacks() {
        with_temp_home(|| {
            let m = manifest_with(vec![]);
            let mounts = cache_mounts("colabfold", "docker", &m, None).unwrap();
            assert!(mounts.is_empty());
        });
    }
}
