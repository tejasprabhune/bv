use anyhow::{Context, Result};
use oci_client::{
    client::{Client, ClientConfig, ImageLayer, Config},
    secrets::RegistryAuth,
    Reference,
};

use crate::build::OciImage;

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
    let auth = RegistryAuth::Anonymous;

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

    // Extract manifest digest from the manifest URL (ends with sha256:...).
    let digest = resp
        .manifest_url
        .split('@')
        .nth(1)
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
    let auth = RegistryAuth::Anonymous;

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
