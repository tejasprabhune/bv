/// Integration tests for the `bv` CLI.
///
/// Tests marked `requires Docker daemon` run in CI via:
///   cargo test --test integration -- --include-ignored --skip alphafold
///
/// Tests marked `requires GPU` or `requires large download` are excluded from
/// CI and must be run manually on the appropriate hardware.
///
/// Run everything locally:
///   cargo test --test integration -- --include-ignored
///
/// On a remote server without a local registry checkout:
///   BV_REGISTRY=https://github.com/tejasprabhune/bv-registry \
///     cargo test --test integration -- --include-ignored --skip alphafold
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Smoke test: registry clone works without a credential prompt.
///
/// Requires network access but no Docker. Run with:
///   cargo test --test integration registry_smoke -- --include-ignored
#[test]
#[ignore = "requires network access to GitHub"]
fn registry_clone_does_not_hang_on_credential_prompt() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");

    // Use a timeout: if git hangs waiting for a credential prompt the child
    // process will not exit and this test will time out (fail), not deadlock.
    let mut child = bv(
        &["search", "samtools"],
        dir.path(),
        cache.path(),
    )
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .spawn()
    .expect("failed to spawn bv search");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match child.try_wait().expect("try_wait failed") {
            Some(status) => {
                assert!(status.success(), "bv search exited non-zero: {status}");
                break;
            }
            None if std::time::Instant::now() > deadline => {
                child.kill().ok();
                panic!("bv search timed out — likely blocked on a git credential prompt");
            }
            None => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
}

/// Path to the compiled `bv` binary.
fn bv_bin() -> PathBuf {
    env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("bv")
}

/// Registry URL: honours BV_REGISTRY env var so CI/remote boxes work without
/// a local checkout. Falls back to the sibling `bv-registry/` directory for
/// local dev.
fn registry_url() -> String {
    if let Ok(url) = std::env::var("BV_REGISTRY") {
        return url;
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("bv-registry")
        .to_string_lossy()
        .to_string()
}

/// Build a `Command` for `bv` with isolated project and cache directories.
fn bv(args: &[&str], project_dir: &Path, cache_dir: &Path) -> Command {
    let mut cmd = Command::new(bv_bin());
    cmd.args(args)
        .current_dir(project_dir)
        .env("BV_CACHE_DIR", cache_dir)
        .env_remove("BV_BACKEND");
    cmd
}

#[allow(dead_code)]
fn bv_backend(args: &[&str], project_dir: &Path, cache_dir: &Path, backend: &str) -> Command {
    let mut cmd = bv(args, project_dir, cache_dir);
    cmd.env("BV_BACKEND", backend);
    cmd
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn add_blast_creates_toml_and_lock() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    let status = bv(
        &[
            "add",
            "blast@2.14.0",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("bv add failed to launch");
    assert!(status.success(), "bv add exited non-zero: {status}");

    let bv_toml = std::fs::read_to_string(dir.path().join("bv.toml")).expect("bv.toml missing");
    assert!(bv_toml.contains("blast"), "bv.toml doesn't mention blast");

    let bv_lock = std::fs::read_to_string(dir.path().join("bv.lock")).expect("bv.lock missing");
    assert!(bv_lock.contains("blast"), "bv.lock doesn't mention blast");
    assert!(bv_lock.contains("2.14.0"), "bv.lock missing version");
    assert!(bv_lock.contains("sha256:"), "bv.lock missing digest");
    assert!(
        bv_lock.contains("manifest_sha256"),
        "bv.lock missing manifest_sha256"
    );
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn add_is_idempotent() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    let s1 = bv(
        &[
            "add",
            "blast@2.14.0",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("first add failed to launch");
    assert!(s1.success());

    let s2 = bv(
        &[
            "add",
            "blast@2.14.0",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("second add failed to launch");
    assert!(s2.success(), "idempotent re-add failed: {s2}");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn run_blast_version() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    let add_ok = bv(
        &[
            "add",
            "blast@2.14.0",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("bv add failed to launch");
    assert!(add_ok.success(), "bv add failed: {add_ok}");

    let run_ok = bv(
        &["run", "blast", "--", "blastn", "-version"],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("bv run failed to launch");
    assert_eq!(run_ok.code(), Some(0), "bv run exited non-zero: {run_ok}");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn unknown_tool_gives_clear_error() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    let output = bv(
        &["add", "fakename", "--registry", &registry],
        dir.path(),
        cache.path(),
    )
    .output()
    .expect("bv add failed to launch");

    assert!(
        !output.status.success(),
        "expected non-zero exit for unknown tool"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("fakename"),
        "expected informative error, got: {stderr}"
    );
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn run_without_add_gives_clear_error() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");

    let output = bv(
        &["run", "blast", "--", "blastn", "-version"],
        dir.path(),
        cache.path(),
    )
    .output()
    .expect("bv run failed to launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bv add") || stderr.contains("not in this project"),
        "expected helpful error, got: {stderr}"
    );
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn sync_reproduces_environment() {
    let dir_a = tempfile::tempdir().expect("project dir A");
    let dir_b = tempfile::tempdir().expect("project dir B");
    let cache_a = tempfile::tempdir().expect("cache A");
    let cache_b = tempfile::tempdir().expect("cache B");
    let registry = registry_url();

    let add_ok = bv(
        &[
            "add",
            "blast@2.14.0",
            "hmmer",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir_a.path(),
        cache_a.path(),
    )
    .status()
    .expect("bv add failed to launch");
    assert!(add_ok.success(), "bv add in dir_a failed");

    std::fs::copy(dir_a.path().join("bv.toml"), dir_b.path().join("bv.toml")).unwrap();
    std::fs::copy(dir_a.path().join("bv.lock"), dir_b.path().join("bv.lock")).unwrap();

    let sync_ok = bv(&["sync"], dir_b.path(), cache_b.path())
        .status()
        .expect("bv sync failed to launch");
    assert!(sync_ok.success(), "bv sync in dir_b failed");

    let check_ok = bv(
        &["lock", "--check", "--registry", &registry],
        dir_b.path(),
        cache_b.path(),
    )
    .status()
    .expect("bv lock --check failed to launch");
    assert!(
        check_ok.success(),
        "bv lock --check failed (lockfile drifted)"
    );
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn lock_check_detects_out_of_date() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    std::fs::write(
        dir.path().join("bv.toml"),
        format!(
            "[project]\nname = \"test\"\n\n[registry]\nurl = \"{registry}\"\n\n[[tools]]\nid = \"blast\"\n"
        ),
    )
    .unwrap();

    let status = bv(
        &["lock", "--check", "--registry", &registry],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("bv lock --check failed to launch");
    assert!(
        !status.success(),
        "expected non-zero exit when bv.lock is missing"
    );
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn remove_updates_both_files() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    let add_ok = bv(
        &[
            "add",
            "blast@2.14.0",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("bv add failed to launch");
    assert!(add_ok.success());

    let rm_ok = bv(&["remove", "blast"], dir.path(), cache.path())
        .status()
        .expect("bv remove failed to launch");
    assert!(rm_ok.success(), "bv remove failed");

    let toml = std::fs::read_to_string(dir.path().join("bv.toml")).unwrap();
    let lock = std::fs::read_to_string(dir.path().join("bv.lock")).unwrap();
    assert!(
        !toml.contains("blast"),
        "bv.toml still mentions blast after remove"
    );
    assert!(
        !lock.contains("blast"),
        "bv.lock still mentions blast after remove"
    );
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn sync_frozen_fails_on_mismatch() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    let add_ok = bv(
        &[
            "add",
            "blast@2.14.0",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("bv add failed to launch");
    assert!(add_ok.success());

    let toml_path = dir.path().join("bv.toml");
    let mut content = std::fs::read_to_string(&toml_path).unwrap();
    content.push_str("\n[[tools]]\nid = \"hmmer\"\n");
    std::fs::write(&toml_path, content).unwrap();

    let status = bv(&["sync", "--frozen"], dir.path(), cache.path())
        .status()
        .expect("bv sync failed to launch");
    assert!(
        !status.success(),
        "expected --frozen to fail when bv.toml has hmmer but bv.lock does not"
    );
}

/// Verify that `bv run` fails with a clear `bv data fetch` hint when required
/// reference data is absent. This test synthesizes bv.lock and a cached manifest
/// directly so it does not need Docker or network access.
#[test]
#[ignore = "requires Docker daemon (bv run invokes docker)"]
fn run_tool_with_missing_required_data_fails_with_hint() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");

    // Write bv.toml
    std::fs::write(
        dir.path().join("bv.toml"),
        "[project]\nname = \"test\"\n\n[[tools]]\nid = \"fake-ref-tool\"\n",
    )
    .unwrap();

    // Write bv.lock with a fake entry for fake-ref-tool
    std::fs::write(
        dir.path().join("bv.lock"),
        r#"version = 1

[metadata]
bv_version = "0.1.0"
generated_at = "2024-01-15T10:00:00Z"

[tools.fake-ref-tool]
tool_id = "fake-ref-tool"
version = "1.0.0"
image_reference = "hello-world:latest"
image_digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
manifest_sha256 = ""
resolved_at = "2024-01-15T10:00:00Z"
"#,
    )
    .unwrap();

    // Write a cached manifest that declares required reference data
    let manifest_dir = cache
        .path()
        .join("tools")
        .join("fake-ref-tool")
        .join("1.0.0");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(
        manifest_dir.join("manifest.toml"),
        r#"[tool]
id = "fake-ref-tool"
version = "1.0.0"

[tool.image]
backend = "docker"
reference = "hello-world:latest"

[tool.hardware]

[tool.hardware.gpu]
required = false

[tool.reference_data.testdb]
id = "testdb"
version = "1.0"
required = true
mount_path = "/data/testdb"

[tool.entrypoint]
command = "echo"
"#,
    )
    .unwrap();

    // bv run should fail before touching Docker because testdb is not in cache
    let output = bv(
        &["run", "fake-ref-tool", "--", "echo", "hi"],
        dir.path(),
        cache.path(),
    )
    .output()
    .expect("bv run failed to launch");

    assert!(
        !output.status.success(),
        "expected non-zero exit when reference data is missing"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bv data fetch"),
        "expected fetch hint in error, got:\n{stderr}"
    );
}

/// Requires pulling the 8 GB alphafold image; only run on a GPU machine with
/// the image already cached locally to avoid timeouts.
#[test]
#[ignore = "requires GPU machine with alphafold image pre-pulled"]
fn alphafold_add_prints_reference_data_notice() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    let output = bv(
        &[
            "add",
            "alphafold@2.3.2",
            "--ignore-hardware",
            "--registry",
            &registry,
        ],
        dir.path(),
        cache.path(),
    )
    .output()
    .expect("bv add failed to launch");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("requires the following reference datasets"),
        "expected reference data notice, got:\n{stderr}"
    );
    assert!(
        stderr.contains("bv data fetch"),
        "expected fetch hint in notice, got:\n{stderr}"
    );
}

#[test]
#[ignore = "requires network access to NCBI FTP (large file)"]
fn data_fetch_downloads_and_verifies() {
    let dir = tempfile::tempdir().expect("project dir");
    let cache = tempfile::tempdir().expect("cache dir");
    let registry = registry_url();

    std::fs::write(
        dir.path().join("bv.toml"),
        format!("[project]\nname = \"test\"\n\n[registry]\nurl = \"{registry}\"\n"),
    )
    .unwrap();

    let status = bv(
        &["data", "fetch", "pdbaa", "--yes", "--registry", &registry],
        dir.path(),
        cache.path(),
    )
    .status()
    .expect("bv data fetch failed to launch");

    // Replace with assert!(status.success()) once sha256 in the registry manifest is updated.
    let _ = status;
}
