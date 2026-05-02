use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

use bv_core::lockfile::SpecKind;
use bv_core::project::BvLock;

use crate::commands::add::short_digest;

pub fn run(package: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    if !bv_lock_path.exists() {
        println!("no bv.lock found; run `bv add <tool>` to add tools to this project");
        return Ok(());
    }

    let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;

    if lockfile.tools.is_empty() {
        println!("No tools installed.");
        return Ok(());
    }

    let pkg_lower = package.to_lowercase();
    let mut found = false;

    let mut tools: Vec<_> = lockfile.tools.values().collect();
    tools.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));

    for entry in tools {
        match entry.spec_kind {
            SpecKind::FactoredOci => {
                for (layer_idx, layer) in entry.layers.iter().enumerate() {
                    let Some(pkg) = &layer.conda_package else {
                        continue;
                    };
                    if pkg.name.to_lowercase().contains(&pkg_lower) {
                        println!(
                            "  {}  layer[{}]  {}  {}  {}",
                            entry
                                .tool_id
                                .if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
                            layer_idx,
                            format!("{}=={}", pkg.name, pkg.version)
                                .if_supports_color(Stream::Stdout, |t| t.cyan().to_string()),
                            pkg.channel
                                .if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
                            short_digest(&layer.digest)
                                .if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
                        );
                        found = true;
                    }
                }
            }
            SpecKind::LegacyImage => {
                // Legacy images are monolithic squashed layers; conda package
                // provenance is not tracked in the lockfile. Report the tool
                // as a possible match so the user knows to check the image.
                println!(
                    "  {}  (legacy image, conda package provenance not tracked)  {}",
                    entry
                        .tool_id
                        .if_supports_color(Stream::Stdout, |t| t.bold().to_string()),
                    short_digest(&entry.image_digest)
                        .if_supports_color(Stream::Stdout, |t| t.dimmed().to_string()),
                );
                found = true;
            }
        }
    }

    if !found {
        println!(
            "Package '{}' not found in any installed tool's layer list.",
            package
        );
        println!("  Run `bv list --layers` to see what is installed.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use bv_core::lockfile::{CondaPackagePin, LayerDescriptor, LockfileEntry, SpecKind};
    use std::collections::BTreeMap;

    fn factored_entry_with_pkg(tool_id: &str, pkg_name: &str) -> LockfileEntry {
        LockfileEntry {
            tool_id: tool_id.into(),
            declared_version_req: String::new(),
            version: "1.0.0".into(),
            spec_kind: SpecKind::FactoredOci,
            image_reference: format!("ghcr.io/example/{tool_id}:1.0.0"),
            image_digest: format!("sha256:img-{tool_id}"),
            manifest_sha256: String::new(),
            image_size_bytes: None,
            layers: vec![
                LayerDescriptor {
                    digest: "sha256:shared-openssl".into(),
                    size: 10_000_000,
                    media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
                    conda_package: Some(CondaPackagePin {
                        name: "openssl".into(),
                        version: "3.2.1".into(),
                        build: "h0_0".into(),
                        channel: "conda-forge".into(),
                        sha256: "abcd".into(),
                    }),
                },
                LayerDescriptor {
                    digest: format!("sha256:pkg-{pkg_name}"),
                    size: 20_000_000,
                    media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
                    conda_package: Some(CondaPackagePin {
                        name: pkg_name.into(),
                        version: "1.0.0".into(),
                        build: "h0_0".into(),
                        channel: "bioconda".into(),
                        sha256: "efgh".into(),
                    }),
                },
            ],
            resolved_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1700000000, 0).unwrap(),
            reference_data_pins: BTreeMap::new(),
            binaries: vec![tool_id.into()],
        }
    }

    #[test]
    fn openssl_found_in_factored_entry() {
        let entry = factored_entry_with_pkg("samtools", "samtools");
        let matches: Vec<_> = entry
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.conda_package
                    .as_ref()
                    .map(|p| p.name.contains("openssl"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0, 0);
    }

    #[test]
    fn tool_specific_pkg_found() {
        let entry = factored_entry_with_pkg("bwa", "bwa");
        let matches: Vec<_> = entry
            .layers
            .iter()
            .filter(|l| {
                l.conda_package
                    .as_ref()
                    .map(|p| p.name.contains("bwa"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].conda_package.as_ref().unwrap().name, "bwa");
    }
}
