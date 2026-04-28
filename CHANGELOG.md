# Changelog

All notable changes to `bv` are documented here. Follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.1.9] - 2026-04-28

### Added

- `PostDownloadAction::Decompress`: gunzip a single `.gz` payload into the cache (previously the only post-download verbs were `noop` and `extract`, which forced gzipped data manifests to be misclassified).
- `bv data fetch` now downloads every entry in `source_urls`, not just the first. The primary file is sha256-verified and the post-download action is applied to it; remaining files (for example, a `.tbi` index alongside a `.vcf.gz`) are placed in the cache directory as-is.
- The `Decompress` action uses `MultiGzDecoder` so bgzipped payloads (such as ClinVar's tabix-indexed VCF, where the gzip stream is many concatenated members) are fully decompressed instead of stopping at the first member.

## [0.1.0] - 2026-04-26

Initial release.

### Added

- `bv add <tool>[@version]`: resolve and pull tools from a git-backed registry with parallel pulls (tokio + semaphore)
- `bv remove <tool>`: atomic removal from `bv.toml` and `bv.lock`
- `bv run <tool> -- <args>`: run by pinned digest with `$PWD:/workspace` mount; reference data mounted read-only at declared paths
- `bv list`: tabular view of installed tools with digest, size, and date
- `bv lock [--check]`: regenerate `bv.lock` from `bv.toml`; `--check` exits 1 with diff in CI
- `bv sync [--frozen]`: pull locked images by digest; `--frozen` validates consistency before pulling
- `bv data fetch <dataset>[@ver] [--yes]`: download reference datasets with progress bar, sha256 verification, and atomic placement
- `bv data list`: list locally cached datasets
- `bv doctor`: Docker version, CPU/RAM/disk/GPU, cache size, reference data, index state, project status
- `bv cache prune`: stub pointing to `docker image prune` (full implementation deferred)
- Hardware requirement checking with `--ignore-hardware` override
- Lockfile v1 schema with `manifest_sha256` for drift detection
- TTL-based index refresh (5-minute cache for `bv data fetch` and `bv sync` drift check)
- Built-in default registry (`https://github.com/mlberkeley/bv-registry`): no configuration needed for most users
- `BV_REGISTRY` environment variable and `--registry` flag for private registries
- `BV_CACHE_DIR` for test isolation

### Registry (mlberkeley/bv-registry)

- blast 2.14.0, 2.15.0
- hmmer 3.3.2
- mmseqs2 14.7564.0
- alphafold 2.3.2 (GPU, requires reference data)
- proteinmpnn 1.0.1 (GPU)
- pdbaa 2024_01 reference dataset (~70 MB)
