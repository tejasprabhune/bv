use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use bv_core::lockfile::{CondaPackagePin, LayerDescriptor};
use oci_client::{
    client::{Client, ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
    Reference,
};
use sha2::{Digest, Sha256};

use crate::layering::{pack, LayerGroup, PackingStrategy};
use crate::popularity::PopularityMap;
use crate::spec::ResolvedSpec;

// SOURCE_DATE_EPOCH = 0 (1970-01-01T00:00:00Z).
// Reproducibility rule: all file mtimes set to this value so that two builds
// of the same packages produce bit-identical compressed layer blobs.
// Reference: https://reproducible-builds.org/docs/source-date-epoch/
const SOURCE_DATE_EPOCH: u64 = 0;

/// An in-memory OCI image ready to be pushed or saved.
pub struct OciImage {
    pub name: String,
    pub version: String,
    pub layers: Vec<OciLayer>,
    /// OCI image config JSON bytes (sha256 needed for manifest).
    pub config: Vec<u8>,
}

pub struct OciLayer {
    pub compressed: Vec<u8>,
    pub descriptor: LayerDescriptor,
    /// sha256 of the uncompressed tarball; used for the OCI config DiffID.
    pub uncompressed_digest: String,
}

impl OciImage {
    /// Compute the OCI image manifest JSON (image manifest v2/OCI schema).
    pub fn manifest_json(&self) -> Result<Vec<u8>> {
        let config_digest = sha256_hex(&self.config);
        let config_size = self.config.len() as u64;

        let mut layers_json = String::from("[\n");
        for (i, layer) in self.layers.iter().enumerate() {
            let comma = if i + 1 == self.layers.len() { "" } else { "," };
            layers_json.push_str(&format!(
                "    {{\"mediaType\":\"{}\",\"digest\":\"{}\",\"size\":{}}}{}\n",
                layer.descriptor.media_type,
                layer.descriptor.digest,
                layer.descriptor.size,
                comma,
            ));
        }
        layers_json.push(']');

        let manifest = format!(
            r#"{{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "config": {{
    "mediaType": "application/vnd.oci.image.config.v1+json",
    "digest": "sha256:{config_digest}",
    "size": {config_size}
  }},
  "layers": {layers_json}
}}"#
        );
        Ok(manifest.into_bytes())
    }
}

/// Build an `OciImage` from a `ResolvedSpec`.
///
/// Each package in the spec becomes one OCI layer (or a group when packing
/// is enabled). A base OS layer (defaults to debian:12-slim) is prepended so
/// the container has the dynamic linker and glibc that conda binaries require.
pub async fn build(
    resolved: &ResolvedSpec,
    strategy: &PackingStrategy,
    popularity: Option<&PopularityMap>,
) -> Result<OciImage> {
    let groups = pack(&resolved.packages, strategy, popularity);

    let http = reqwest::Client::builder()
        .user_agent("bv-builder/0.1")
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    // Pull base image layers first so the container has glibc + dynamic linker.
    let base_ref = resolved
        .base
        .as_deref()
        .unwrap_or("docker.io/library/debian:12-slim");
    let mut layers = fetch_base_layers(base_ref).await
        .with_context(|| format!("fetch base image '{base_ref}'"))?;

    for group in &groups {
        let layer = build_group_layer(&http, group).await?;
        layers.push(layer);
    }

    // Meta layer: conda-meta JSON for all packages.
    let meta_layer = build_meta_layer(resolved)?;
    layers.push(meta_layer);

    // Entrypoint layer.
    let entrypoint_layer = build_entrypoint_layer(resolved)?;
    layers.push(entrypoint_layer);

    let config = build_config(resolved, &layers)?;

    Ok(OciImage {
        name: resolved.name.clone(),
        version: resolved.version.clone(),
        layers,
        config,
    })
}

/// Pull a base OCI image from a registry and return its layers.
///
/// The base image (typically `debian:12-slim`) provides glibc and the dynamic
/// linker that conda binaries depend on. Its layers are prepended before the
/// conda package layers so the container root FS is complete.
async fn fetch_base_layers(base_ref: &str) -> Result<Vec<OciLayer>> {
    use futures_util::StreamExt;

    let reference: Reference = base_ref
        .parse()
        .with_context(|| format!("parse base OCI reference '{base_ref}'"))?;

    let oci_config = ClientConfig {
        protocol: ClientProtocol::HttpsExcept(vec!["localhost".into(), "127.0.0.1".into()]),
        ..Default::default()
    };
    let client = Client::new(oci_config);
    let auth = RegistryAuth::Anonymous;

    let (manifest, _digest, config_json) = client
        .pull_manifest_and_config(&reference, &auth)
        .await
        .with_context(|| format!("pull manifest+config for '{base_ref}'"))?;

    let base_config: serde_json::Value = serde_json::from_str(&config_json)
        .context("parse base image config")?;
    let base_diff_ids = base_config["rootfs"]["diff_ids"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut result = Vec::new();
    for (i, layer_desc) in manifest.layers.iter().enumerate() {
        let digest = &layer_desc.digest;
        let media_type = &layer_desc.media_type;
        let size = layer_desc.size as u64;

        let mut compressed = Vec::new();
        let mut stream = client
            .pull_blob_stream(&reference, layer_desc)
            .await
            .with_context(|| format!("pull base layer blob {digest}"))?;
        while let Some(chunk) = stream.next().await {
            compressed.extend_from_slice(&chunk?);
        }

        let uncompressed_digest = base_diff_ids
            .get(i)
            .and_then(|v| v.as_str())
            .unwrap_or(digest)
            .to_string();

        result.push(OciLayer {
            compressed,
            uncompressed_digest,
            descriptor: LayerDescriptor {
                digest: digest.clone(),
                size,
                media_type: media_type.clone(),
                conda_package: None,
            },
        });
    }

    Ok(result)
}

/// Download and layer a single package group.
async fn build_group_layer(client: &reqwest::Client, group: &LayerGroup) -> Result<OciLayer> {
    let work_dir = tempfile::tempdir().context("create temp dir for layer build")?;

    for pkg in &group.packages {
        download_and_extract_package(client, pkg, work_dir.path()).await?;
    }

    let (compressed, uncompressed_digest) = create_reproducible_layer(work_dir.path())?;
    let digest = format!("sha256:{}", sha256_hex(&compressed));
    let size = compressed.len() as u64;

    // For single-package groups, attach conda_package metadata.
    let conda_package = if group.packages.len() == 1 {
        let pkg = &group.packages[0];
        Some(CondaPackagePin {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            build: pkg.build.clone(),
            channel: pkg.channel.clone(),
            sha256: pkg.sha256.clone(),
        })
    } else {
        None
    };

    Ok(OciLayer {
        compressed,
        uncompressed_digest: format!("sha256:{uncompressed_digest}"),
        descriptor: LayerDescriptor {
            digest,
            size,
            media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
            conda_package,
        },
    })
}

/// Download a conda package and extract it into `dest_dir`.
async fn download_and_extract_package(
    client: &reqwest::Client,
    pkg: &crate::spec::ResolvedPackage,
    dest_dir: &Path,
) -> Result<()> {
    use futures_util::StreamExt;

    let resp = client
        .get(&pkg.url)
        .send()
        .await
        .with_context(|| format!("download {}", pkg.url))?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching {}", resp.status(), pkg.url);
    }

    let mut bytes = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk?);
    }

    // Verify sha256 if present.
    if !pkg.sha256.is_empty() {
        let actual = sha256_hex(&bytes);
        if actual != pkg.sha256 {
            anyhow::bail!(
                "sha256 mismatch for {} ({}): expected {} got {}",
                pkg.name,
                pkg.filename,
                pkg.sha256,
                actual
            );
        }
    }

    // Extract .conda (zip) or .tar.bz2.
    if pkg.filename.ends_with(".conda") {
        extract_conda_archive(&bytes, dest_dir)
            .with_context(|| format!("extract {}", pkg.filename))?;
    } else if pkg.filename.ends_with(".tar.bz2") {
        extract_tar_bz2(&bytes, dest_dir)
            .with_context(|| format!("extract {}", pkg.filename))?;
    }

    Ok(())
}

fn extract_conda_archive(data: &[u8], dest: &Path) -> Result<()> {
    use std::io::Read;
    let cursor = std::io::Cursor::new(data);
    let mut zip = zip::ZipArchive::new(cursor).context("open .conda zip")?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.name().starts_with("pkg-") && entry.name().ends_with(".tar.zst") {
            let mut zstd_bytes = Vec::new();
            entry.read_to_end(&mut zstd_bytes)?;
            let decompressed = zstd::decode_all(std::io::Cursor::new(zstd_bytes))
                .context("decompress pkg- zstd")?;
            extract_tar_bytes(&decompressed, dest)?;
        } else if entry.name().starts_with("info-") && entry.name().ends_with(".tar.zst") {
            let mut zstd_bytes = Vec::new();
            entry.read_to_end(&mut zstd_bytes)?;
            let decompressed = zstd::decode_all(std::io::Cursor::new(zstd_bytes))
                .context("decompress info- zstd")?;
            extract_tar_bytes(&decompressed, dest)?;
        }
    }
    Ok(())
}

fn extract_tar_bz2(data: &[u8], dest: &Path) -> Result<()> {
    let decompressed = bzip2::read::BzDecoder::new(data);
    let mut archive = tar::Archive::new(decompressed);
    archive.unpack(dest).context("unpack tar.bz2")?;
    Ok(())
}

fn extract_tar_bytes(data: &[u8], dest: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(std::io::Cursor::new(data));
    archive.unpack(dest).context("unpack tar")?;
    Ok(())
}

/// Create a reproducible, sorted, zstd-compressed OCI layer tarball from `dir`.
///
/// Reproducibility rules (https://reproducible-builds.org/docs/archives/):
/// - PAX tar format
/// - All mtimes set to SOURCE_DATE_EPOCH
/// - All uid/gid set to 0
/// - Entries sorted by path
/// - zstd level 19 compression
fn create_reproducible_layer(dir: &Path) -> Result<(Vec<u8>, String)> {
    use std::fs;

    let mut entries: Vec<std::path::PathBuf> = Vec::new();
    collect_files(dir, &mut entries)?;
    entries.sort();

    let mut uncompressed: Vec<u8> = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut uncompressed);
        builder.follow_symlinks(false);

        for entry_path in &entries {
            let rel = entry_path.strip_prefix(dir).unwrap();
            let meta = fs::symlink_metadata(entry_path)?;

            let mut header = tar::Header::new_ustar();
            header.set_metadata(&meta);
            header.set_mtime(SOURCE_DATE_EPOCH);
            header.set_uid(0);
            header.set_gid(0);
            header.set_username("")?;
            header.set_groupname("")?;

            if meta.file_type().is_symlink() {
                let target = fs::read_link(entry_path)?;
                header.set_size(0);
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name(&target)?;
                header.set_cksum();
                builder.append(&header, std::io::empty())?;
            } else if meta.is_file() {
                let data = fs::read(entry_path)?;
                header.set_size(data.len() as u64);
                header.set_cksum();
                builder.append_data(&mut header, rel, data.as_slice())?;
            } else if meta.is_dir() {
                header.set_size(0);
                header.set_cksum();
                builder.append_data(&mut header, rel, std::io::empty())?;
            }
        }
        builder.finish()?;
    }

    let uncompressed_digest = sha256_hex(&uncompressed);

    // zstd level 19 for maximum compression density.
    let compressed = zstd::encode_all(std::io::Cursor::new(&uncompressed), 19)
        .context("zstd compress layer")?;

    Ok((compressed, uncompressed_digest))
}

fn collect_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            out.push(path);
        } else if meta.is_dir() {
            out.push(path.clone());
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Build a thin layer containing `/conda-meta/<pkg>.json` for every package.
fn build_meta_layer(resolved: &ResolvedSpec) -> Result<OciLayer> {
    let work_dir = tempfile::tempdir().context("create temp dir for meta layer")?;
    let conda_meta = work_dir.path().join("conda-meta");
    std::fs::create_dir_all(&conda_meta)?;

    for pkg in &resolved.packages {
        let meta = serde_json::json!({
            "name": pkg.name,
            "version": pkg.version,
            "build": pkg.build,
            "channel": pkg.channel,
            "url": pkg.url,
            "sha256": pkg.sha256,
        });
        let filename = format!("{}-{}-{}.json", pkg.name, pkg.version, pkg.build);
        let path = conda_meta.join(filename);
        std::fs::write(&path, serde_json::to_string_pretty(&meta)?)?;
    }

    let (compressed, uncompressed_digest) = create_reproducible_layer(work_dir.path())?;
    let digest = format!("sha256:{}", sha256_hex(&compressed));
    let size = compressed.len() as u64;

    Ok(OciLayer {
        compressed,
        uncompressed_digest: format!("sha256:{uncompressed_digest}"),
        descriptor: LayerDescriptor {
            digest,
            size,
            media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
            conda_package: None,
        },
    })
}

/// Build the entrypoint layer: a `/bv-entrypoint.sh` script that exec's the
/// tool's declared command.
fn build_entrypoint_layer(_resolved: &ResolvedSpec) -> Result<OciLayer> {
    let work_dir = tempfile::tempdir().context("create temp dir for entrypoint layer")?;
    let script_path = work_dir.path().join("bv-entrypoint.sh");
    {
        let mut f = std::fs::File::create(&script_path)?;
        writeln!(f, "#!/bin/sh")?;
        writeln!(f, "# Generated by bv-builder — do not edit")?;
        writeln!(f, "exec \"$@\"")?;
    }
    // Make executable (755).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms)?;
    }

    let (compressed, uncompressed_digest) = create_reproducible_layer(work_dir.path())?;
    let digest = format!("sha256:{}", sha256_hex(&compressed));
    let size = compressed.len() as u64;

    Ok(OciLayer {
        compressed,
        uncompressed_digest: format!("sha256:{uncompressed_digest}"),
        descriptor: LayerDescriptor {
            digest,
            size,
            media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
            conda_package: None,
        },
    })
}

/// Build the OCI image config JSON.
fn build_config(resolved: &ResolvedSpec, layers: &[OciLayer]) -> Result<Vec<u8>> {
    let diff_ids: Vec<String> = layers
        .iter()
        .map(|l| l.uncompressed_digest.clone())
        .collect();

    let config = serde_json::json!({
        "architecture": resolved.platform.to_string().split('/').nth(1).unwrap_or("amd64"),
        "os": "linux",
        "created": "1970-01-01T00:00:00Z",
        "author": "bv-builder",
        "config": {
            "Labels": {
                "org.opencontainers.image.title": &resolved.name,
                "org.opencontainers.image.version": &resolved.version,
            }
        },
        "rootfs": {
            "type": "layers",
            "diff_ids": diff_ids,
        },
        "history": []
    });

    Ok(serde_json::to_vec_pretty(&config)?)
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_is_correct() {
        let hash = sha256_hex(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn create_reproducible_layer_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), b"content").unwrap();
        let (c1, d1) = create_reproducible_layer(dir.path()).unwrap();
        let (c2, d2) = create_reproducible_layer(dir.path()).unwrap();
        assert_eq!(c1, c2, "compressed bytes differ between two runs");
        assert_eq!(d1, d2, "digests differ between two runs");
    }
}
