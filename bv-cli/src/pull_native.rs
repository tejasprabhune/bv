use std::io::Write;

use anyhow::{Context, Result};
use bv_runtime::OciRef;

/// Pull an OCI image directly from the registry via HTTPS, bypassing the
/// Docker Desktop VM. Downloads layer blobs concurrently, assembles an OCI
/// Image Layout tar in memory, and loads it via `docker load`. Returns the
/// manifest digest (`sha256:<hex>`).
pub async fn pull_native(oci_ref: &OciRef) -> Result<String> {
    let token = fetch_bearer_token(oci_ref).await?;
    let client = build_client();

    let (manifest_bytes, manifest_digest) = fetch_manifest(&client, oci_ref, &token).await?;

    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).context("parse manifest JSON")?;

    let config_digest = manifest["config"]["digest"]
        .as_str()
        .context("manifest.config.digest missing")?
        .to_string();
    let config_bytes = fetch_blob(&client, oci_ref, &token, &config_digest).await?;

    let layers_json = manifest["layers"]
        .as_array()
        .context("manifest.layers missing")?;

    // Download all layers concurrently.
    let mut layer_futs = tokio::task::JoinSet::new();
    for layer in layers_json {
        let digest = layer["digest"]
            .as_str()
            .context("layer.digest missing")?
            .to_string();
        let media_type = layer["mediaType"]
            .as_str()
            .context("layer.mediaType missing")?
            .to_string();
        let size = layer["size"].as_u64().context("layer.size missing")?;
        let client2 = client.clone();
        let oci_ref2 = oci_ref.clone();
        let token2 = token.clone();
        layer_futs.spawn(async move {
            let data = fetch_blob(&client2, &oci_ref2, &token2, &digest).await?;
            Ok::<_, anyhow::Error>((digest, media_type, size, data))
        });
    }

    let mut layers: Vec<(String, String, u64, Vec<u8>)> = Vec::new();
    while let Some(res) = layer_futs.join_next().await {
        layers.push(res.context("layer download task panicked")??);
    }
    // Restore manifest order for reproducibility.
    let order: Vec<&str> = layers_json
        .iter()
        .filter_map(|l| l["digest"].as_str())
        .collect();
    layers.sort_by_key(|(d, _, _, _)| order.iter().position(|o| o == d).unwrap_or(usize::MAX));

    let tar_bytes =
        assemble_oci_layout(manifest_bytes, manifest_digest.clone(), config_bytes, layers, oci_ref)
            .context("assemble OCI layout tar")?;

    load_into_docker(tar_bytes).context("docker load")?;

    Ok(manifest_digest)
}

async fn fetch_bearer_token(oci_ref: &OciRef) -> Result<String> {
    let creds = docker_credentials(&oci_ref.registry);
    let client = build_client();

    // Initial request to /v2/ returns a 401 with WWW-Authenticate: Bearer.
    let www_auth = client
        .get(format!("https://{}/v2/", oci_ref.registry))
        .send()
        .await
        .context("probe /v2/")?
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let realm = extract_bearer_param(&www_auth, "realm")
        .unwrap_or_else(|| format!("https://{}/token", oci_ref.registry));
    let service = extract_bearer_param(&www_auth, "service")
        .unwrap_or_else(|| oci_ref.registry.clone());
    let scope = format!("repository:{}:pull", oci_ref.repository);

    let mut req = client
        .get(&realm)
        .query(&[("service", &service), ("scope", &scope)]);

    if let Some((user, pass)) = creds {
        req = req.basic_auth(user, Some(pass));
    }

    let resp: serde_json::Value = req
        .send()
        .await
        .context("fetch bearer token")?
        .json()
        .await
        .context("parse bearer token response")?;

    resp["token"]
        .as_str()
        .or_else(|| resp["access_token"].as_str())
        .map(str::to_string)
        .context("bearer token not found in response")
}

async fn fetch_manifest(
    client: &reqwest::Client,
    oci_ref: &OciRef,
    token: &str,
) -> Result<(Vec<u8>, String)> {
    let reference = oci_ref
        .digest
        .as_deref()
        .or(oci_ref.tag.as_deref())
        .context("OCI ref has neither tag nor digest")?;

    let url = format!(
        "https://{}/v2/{}/manifests/{}",
        oci_ref.registry, oci_ref.repository, reference
    );

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Accept",
            "application/vnd.docker.distribution.manifest.v2+json, \
             application/vnd.oci.image.manifest.v1+json",
        )
        .send()
        .await
        .context("fetch manifest")?;

    let digest = resp
        .headers()
        .get("docker-content-digest")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let bytes = resp.bytes().await.context("read manifest body")?.to_vec();

    let digest = if digest.is_empty() {
        format!("sha256:{}", hex_encode(&sha256_bytes(&bytes)))
    } else {
        digest
    };

    Ok((bytes, digest))
}

async fn fetch_blob(
    client: &reqwest::Client,
    oci_ref: &OciRef,
    token: &str,
    digest: &str,
) -> Result<Vec<u8>> {
    let url = format!(
        "https://{}/v2/{}/blobs/{}",
        oci_ref.registry, oci_ref.repository, digest
    );
    let bytes = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .with_context(|| format!("fetch blob {digest}"))?
        .bytes()
        .await
        .with_context(|| format!("read blob {digest}"))?
        .to_vec();
    Ok(bytes)
}

/// Build an OCI Image Layout tar and pipe to `docker load`.
///
/// Uses the raw manifest bytes so sha256(manifest_bytes) == manifest_digest,
/// which lets Docker index the loaded image by its registry manifest digest.
fn assemble_oci_layout(
    manifest_bytes: Vec<u8>,
    manifest_digest: String,
    config_bytes: Vec<u8>,
    layers: Vec<(String, String, u64, Vec<u8>)>,
    oci_ref: &OciRef,
) -> Result<Vec<u8>> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).context("re-parse manifest")?;
    let media_type = manifest["mediaType"]
        .as_str()
        .unwrap_or("application/vnd.oci.image.manifest.v1+json");

    let config_digest = manifest["config"]["digest"]
        .as_str()
        .context("config digest")?;

    // org.opencontainers.image.ref.name tells Docker how to tag the loaded image.
    let ref_name = match &oci_ref.tag {
        Some(tag) => format!("{}/{}:{}", oci_ref.registry, oci_ref.repository, tag),
        None => format!("{}/{}", oci_ref.registry, oci_ref.repository),
    };

    let index_json = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": media_type,
            "digest": manifest_digest,
            "size": manifest_bytes.len(),
            "annotations": {
                "org.opencontainers.image.ref.name": ref_name,
            }
        }]
    }))?;

    let oci_layout = br#"{"imageLayoutVersion":"1.0.0"}"#;

    let manifest_hex = manifest_digest
        .strip_prefix("sha256:")
        .unwrap_or(&manifest_digest);
    let config_hex = config_digest.strip_prefix("sha256:").unwrap_or(config_digest);

    let mut buf = Vec::new();
    let mut ar = tar::Builder::new(&mut buf);

    append_entry(&mut ar, "oci-layout", oci_layout)?;
    append_entry(&mut ar, "index.json", &index_json)?;
    append_entry(
        &mut ar,
        &format!("blobs/sha256/{config_hex}"),
        &config_bytes,
    )?;
    append_entry(
        &mut ar,
        &format!("blobs/sha256/{manifest_hex}"),
        &manifest_bytes,
    )?;

    // Deduplicate layer blobs (empty layers often share a digest).
    let mut seen = std::collections::HashSet::new();
    for (digest, _media_type, _size, data) in &layers {
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest.as_str());
        if seen.insert(hex.to_string()) {
            append_entry(&mut ar, &format!("blobs/sha256/{hex}"), data)?;
        }
    }

    ar.finish().context("finish tar")?;
    drop(ar);
    Ok(buf)
}

fn load_into_docker(tar_bytes: Vec<u8>) -> Result<()> {
    use std::process::Stdio;

    let mut child = std::process::Command::new("docker")
        .arg("load")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn docker load")?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(&tar_bytes)
        .context("write to docker load stdin")?;
    drop(stdin);

    let output = child.wait_with_output().context("wait for docker load")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("docker load failed: {stderr}");
    }

    Ok(())
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("build reqwest client")
}

fn docker_credentials(registry: &str) -> Option<(String, String)> {
    let config = home_docker_config()?;

    // Per-registry or global credential helper.
    let helper = config
        .get("credHelpers")
        .and_then(|h| h.get(registry))
        .and_then(|v| v.as_str())
        .or_else(|| config.get("credsStore").and_then(|v| v.as_str()));

    if let Some(helper) = helper {
        if let Some(creds) = run_credential_helper(helper, registry) {
            return Some(creds);
        }
    }

    // Static base64-encoded auth entry.
    config
        .get("auths")
        .and_then(|a| a.get(registry))
        .and_then(|r| r.get("auth"))
        .and_then(|v| v.as_str())
        .and_then(|b64| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.decode(b64).ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|s| {
            let (user, pass) = s.split_once(':')?;
            Some((user.to_string(), pass.to_string()))
        })
}

fn home_docker_config() -> Option<serde_json::Value> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let s = std::fs::read_to_string(home.join(".docker/config.json")).ok()?;
    serde_json::from_str(&s).ok()
}

fn run_credential_helper(helper: &str, registry: &str) -> Option<(String, String)> {
    use std::io::Write as _;
    use std::process::Stdio;

    let cmd = format!("docker-credential-{helper}");
    let mut child = std::process::Command::new(&cmd)
        .arg("get")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    child
        .stdin
        .take()?
        .write_all(registry.as_bytes())
        .ok()?;

    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }

    let creds: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let username = creds.get("Username")?.as_str()?.to_string();
    let secret = creds.get("Secret")?.as_str()?.to_string();
    if username.is_empty() || secret.is_empty() {
        return None;
    }
    Some((username, secret))
}

/// Extract a parameter value from a `WWW-Authenticate: Bearer ...` header.
fn extract_bearer_param(header: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=\"");
    let start = header.find(&prefix)? + prefix.len();
    let end = header[start..].find('"')? + start;
    Some(header[start..end].to_string())
}

fn sha256_bytes(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).to_vec()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn append_entry<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, path, data)
        .with_context(|| format!("append '{path}' to tar"))
}

