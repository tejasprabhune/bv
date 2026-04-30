use anyhow::{Context, Result};
use oci_client::{
    client::{Client, ClientConfig, ImageLayer, Config},
    secrets::RegistryAuth,
    Reference,
};

use crate::build::{OciImage, OciLayer};
use bv_core::lockfile::LayerDescriptor;

fn registry_auth() -> RegistryAuth {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        RegistryAuth::Basic("token".into(), token)
    } else {
        RegistryAuth::Anonymous
    }
}

/// Load an OCI image from a tarball previously saved by `save_oci_tarball`.
pub fn load_from_tarball(path: &std::path::Path) -> Result<OciImage> {
    use std::io::Read;

    let f = std::fs::File::open(path)
        .with_context(|| format!("open tarball {}", path.display()))?;
    let mut archive = tar::Archive::new(f);

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut blobs: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();

    for entry in archive.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let path_str = entry.path().context("get entry path")?.to_string_lossy().into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).context("read entry data")?;

        if path_str == "manifest.json" {
            manifest_bytes = Some(data);
        } else if let Some(hex) = path_str.strip_prefix("blobs/sha256/") {
            blobs.insert(hex.to_string(), data);
        }
    }

    let manifest_bytes = manifest_bytes.context("manifest.json not found in tarball")?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .context("parse manifest.json")?;

    let config_digest = manifest["config"]["digest"]
        .as_str()
        .context("manifest.config.digest missing")?;
    let config_hex = config_digest.strip_prefix("sha256:").unwrap_or(config_digest);
    let config = blobs.remove(config_hex)
        .with_context(|| format!("config blob {config_hex} not found in tarball"))?;

    let layers_json = manifest["layers"].as_array().context("manifest.layers missing")?;
    let mut layers = Vec::new();
    for layer_json in layers_json {
        let digest = layer_json["digest"].as_str().context("layer.digest missing")?;
        let media_type = layer_json["mediaType"].as_str().context("layer.mediaType missing")?;
        let size = layer_json["size"].as_u64().context("layer.size missing")?;
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        let compressed = blobs.remove(hex)
            .with_context(|| format!("layer blob {hex} not found in tarball"))?;

        layers.push(OciLayer {
            compressed,
            descriptor: LayerDescriptor {
                digest: digest.to_string(),
                size,
                media_type: media_type.to_string(),
                conda_package: None,
            },
        });
    }

    Ok(OciImage {
        name: String::new(),
        version: String::new(),
        layers,
        config,
    })
}

/// Push an `OciImage` to a registry.
///
/// Returns the digest of the pushed manifest.
pub async fn push(image: &OciImage, reference: &str) -> Result<String> {
    let reference: Reference = reference
        .parse()
        .with_context(|| format!("parse OCI reference '{reference}'"))?;

    let config = ClientConfig {
        protocol: oci_client::client::ClientProtocol::HttpsExcept(vec![
            "localhost".into(),
            "127.0.0.1".into(),
        ]),
        ..Default::default()
    };

    let client = Client::new(config);
    let auth = registry_auth();

    let layers: Vec<ImageLayer> = image
        .layers
        .iter()
        .map(|l| {
            ImageLayer::new(l.compressed.clone(), l.descriptor.media_type.clone(), None)
        })
        .collect();

    let oci_config = Config::oci_v1(image.config.clone(), None);

    let resp = client
        .push(&reference, &layers, oci_config, &auth, None)
        .await
        .with_context(|| format!("push image to '{reference}'"))?;

    // Extract manifest digest. GHCR returns the URL as
    // `.../manifests/sha256:<hex>` so check the last path segment first,
    // then fall back to the `@sha256:<hex>` style used by some registries.
    let digest = resp
        .manifest_url
        .rsplit('/')
        .next()
        .filter(|s| s.starts_with("sha256:"))
        .or_else(|| resp.manifest_url.split('@').nth(1))
        .unwrap_or("unknown")
        .to_string();

    Ok(digest)
}

/// Fetch an image manifest from a registry and verify its digest matches
/// `expected_digest`.
pub async fn verify(reference: &str, expected_digest: &str) -> Result<()> {
    let reference: Reference = reference
        .parse()
        .with_context(|| format!("parse OCI reference '{reference}'"))?;

    let client = Client::new(ClientConfig::default());
    let auth = registry_auth();

    let (_manifest, digest) = client
        .pull_manifest(&reference, &auth)
        .await
        .with_context(|| format!("pull manifest for '{reference}'"))?;

    if digest != expected_digest {
        anyhow::bail!(
            "digest mismatch for '{reference}': expected {expected_digest} but registry returned {digest}"
        );
    }

    Ok(())
}
