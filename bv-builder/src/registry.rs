use anyhow::Result;

use crate::spec::ResolvedSpec;

/// Snapshot of the repodata used during resolve, stored as an OCI artifact
/// alongside the image using the OCI 1.1 referrers spec.
///
/// Media type: `application/vnd.bv.repodata.snapshot.v1+json`
pub const REPODATA_SNAPSHOT_MEDIA_TYPE: &str =
    "application/vnd.bv.repodata.snapshot.v1+json";

/// Produce a JSON snapshot of the channels + package pins used during resolve.
/// This is pushed as an OCI referrer so that any future `bv-builder resolve`
/// can reproduce the exact same `ResolvedSpec` without hitting live repodata.
pub fn build_repodata_snapshot(resolved: &ResolvedSpec) -> Result<Vec<u8>> {
    let snapshot = serde_json::json!({
        "schema": "bv.repodata.snapshot.v1",
        "name": resolved.name,
        "version": resolved.version,
        "platform": resolved.platform.to_string(),
        "channels": resolved.channels,
        "packages": resolved.packages.iter().map(|p| serde_json::json!({
            "name": p.name,
            "version": p.version,
            "build": p.build,
            "channel": p.channel,
            "url": p.url,
            "sha256": p.sha256,
            "filename": p.filename,
        })).collect::<Vec<_>>(),
    });
    Ok(serde_json::to_vec_pretty(&snapshot)?)
}
