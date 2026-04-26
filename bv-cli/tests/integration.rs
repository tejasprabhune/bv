/// Integration tests for the `bv` CLI.
///
/// These tests require a running Docker daemon and network access.
/// Run with:
///   cargo test --test integration -- --include-ignored
///
/// Or run a single test:
///   cargo test --test integration add_and_run -- --include-ignored
use std::env;
use std::path::PathBuf;
use std::process::Command;

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

/// Path to the local bv-registry used in tests.
fn registry_path() -> String {
    // Relative to this file: ../../../bv-registry
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

#[test]
#[ignore = "requires Docker daemon and network access"]
fn add_blast_creates_toml_and_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry_path();
    let bv = bv_bin();

    let status = Command::new(&bv)
        .args(["add", "blast@2.14.0", "--registry", &registry])
        .current_dir(dir.path())
        .status()
        .expect("bv add failed to launch");

    assert!(status.success(), "bv add exited non-zero: {status}");

    // bv.toml must exist and declare blast.
    let bv_toml_raw = std::fs::read_to_string(dir.path().join("bv.toml")).expect("bv.toml missing");
    assert!(bv_toml_raw.contains("blast"), "bv.toml doesn't mention blast");

    // bv.lock must exist and contain the digest.
    let bv_lock_raw = std::fs::read_to_string(dir.path().join("bv.lock")).expect("bv.lock missing");
    assert!(bv_lock_raw.contains("blast"), "bv.lock doesn't mention blast");
    assert!(bv_lock_raw.contains("2.14.0"), "bv.lock doesn't have version 2.14.0");
    assert!(bv_lock_raw.contains("sha256:"), "bv.lock doesn't contain a digest");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn add_is_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry_path();
    let bv = bv_bin();

    let s1 = Command::new(&bv)
        .args(["add", "blast@2.14.0", "--registry", &registry])
        .current_dir(dir.path())
        .status()
        .expect("first bv add failed");
    assert!(s1.success());

    // Second add should succeed without re-pulling.
    let s2 = Command::new(&bv)
        .args(["add", "blast@2.14.0", "--registry", &registry])
        .current_dir(dir.path())
        .status()
        .expect("second bv add failed");
    assert!(s2.success(), "idempotent re-add failed: {s2}");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn run_blast_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry_path();
    let bv = bv_bin();

    // First add blast.
    let add_status = Command::new(&bv)
        .args(["add", "blast@2.14.0", "--registry", &registry])
        .current_dir(dir.path())
        .status()
        .expect("bv add failed");
    assert!(add_status.success(), "bv add failed: {add_status}");

    // Then run blastn -version.
    let run_status = Command::new(&bv)
        .args(["run", "blast", "--", "blastn", "-version"])
        .current_dir(dir.path())
        .status()
        .expect("bv run failed to launch");

    assert_eq!(run_status.code(), Some(0), "bv run exited non-zero: {run_status}");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn unknown_tool_gives_clear_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = registry_path();
    let bv = bv_bin();

    let output = Command::new(&bv)
        .args(["add", "fakename", "--registry", &registry])
        .current_dir(dir.path())
        .output()
        .expect("bv add failed to launch");

    assert!(!output.status.success(), "expected non-zero exit for unknown tool");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("fakename"),
        "expected informative error, got: {stderr}"
    );
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn run_without_add_gives_clear_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bv = bv_bin();

    let output = Command::new(&bv)
        .args(["run", "blast", "--", "blastn", "-version"])
        .current_dir(dir.path())
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
    let registry = registry_path();
    let bv = bv_bin();

    // Phase 1: create project and add two tools.
    let dir_a = tempfile::tempdir().expect("tempdir A");
    let add_status = Command::new(&bv)
        .args(["add", "blast@2.14.0", "hmmer", "--registry", &registry])
        .current_dir(dir_a.path())
        .status()
        .expect("bv add failed");
    assert!(add_status.success(), "bv add in dir_a failed");

    // Copy only bv.toml + bv.lock to a fresh directory.
    let dir_b = tempfile::tempdir().expect("tempdir B");
    std::fs::copy(dir_a.path().join("bv.toml"), dir_b.path().join("bv.toml")).unwrap();
    std::fs::copy(dir_a.path().join("bv.lock"), dir_b.path().join("bv.lock")).unwrap();

    // Phase 2: sync in dir_b — should pull (or find cached) the same images.
    let sync_status = Command::new(&bv)
        .args(["sync"])
        .current_dir(dir_b.path())
        .status()
        .expect("bv sync failed to launch");
    assert!(sync_status.success(), "bv sync in dir_b failed");

    // Phase 3: lock --check should pass (bv.lock matches what bv.toml resolves to).
    let check_status = Command::new(&bv)
        .args(["lock", "--check", "--registry", &registry])
        .current_dir(dir_b.path())
        .status()
        .expect("bv lock --check failed to launch");
    assert!(check_status.success(), "bv lock --check failed (lockfile drifted)");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn lock_check_detects_out_of_date() {
    let registry = registry_path();
    let bv = bv_bin();

    let dir = tempfile::tempdir().expect("tempdir");

    // Create bv.toml declaring blast but without a bv.lock.
    std::fs::write(
        dir.path().join("bv.toml"),
        "[project]\nname = \"test\"\n\n[registry]\nurl = \"/nonexistent\"\n\n[[tools]]\nid = \"blast\"\n",
    )
    .unwrap();

    // bv lock --check should fail since there's no bv.lock.
    let status = Command::new(&bv)
        .args(["lock", "--check", "--registry", &registry])
        .current_dir(dir.path())
        .status()
        .expect("bv lock --check failed to launch");
    assert!(!status.success(), "expected non-zero exit when bv.lock missing");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn remove_updates_both_files() {
    let registry = registry_path();
    let bv = bv_bin();

    let dir = tempfile::tempdir().expect("tempdir");

    // Add blast.
    let add_status = Command::new(&bv)
        .args(["add", "blast@2.14.0", "--registry", &registry])
        .current_dir(dir.path())
        .status()
        .expect("bv add failed");
    assert!(add_status.success());

    // Remove blast.
    let rm_status = Command::new(&bv)
        .args(["remove", "blast"])
        .current_dir(dir.path())
        .status()
        .expect("bv remove failed to launch");
    assert!(rm_status.success(), "bv remove failed");

    // bv.toml and bv.lock should no longer mention blast.
    let toml_raw = std::fs::read_to_string(dir.path().join("bv.toml")).unwrap();
    let lock_raw = std::fs::read_to_string(dir.path().join("bv.lock")).unwrap();
    assert!(!toml_raw.contains("blast"), "bv.toml still mentions blast");
    assert!(!lock_raw.contains("blast"), "bv.lock still mentions blast");
}

#[test]
#[ignore = "requires Docker daemon and network access"]
fn sync_frozen_fails_on_mismatch() {
    let registry = registry_path();
    let bv = bv_bin();

    let dir = tempfile::tempdir().expect("tempdir");

    // Add blast.
    let add_status = Command::new(&bv)
        .args(["add", "blast@2.14.0", "--registry", &registry])
        .current_dir(dir.path())
        .status()
        .expect("bv add failed");
    assert!(add_status.success());

    // Manually add hmmer to bv.toml without locking.
    let toml_path = dir.path().join("bv.toml");
    let mut toml_raw = std::fs::read_to_string(&toml_path).unwrap();
    toml_raw.push_str("\n[[tools]]\nid = \"hmmer\"\n");
    std::fs::write(&toml_path, toml_raw).unwrap();

    // bv sync --frozen should fail because bv.lock doesn't have hmmer.
    let status = Command::new(&bv)
        .args(["sync", "--frozen"])
        .current_dir(dir.path())
        .status()
        .expect("bv sync failed to launch");
    assert!(!status.success(), "expected --frozen to fail on mismatch");
}
