# Changelog

All notable changes to `bv` are documented here. Follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.1.12] - 2026-04-28

### Added

- `bv conformance` (no tool arg): walks every tool in the registry. Filters: `--filter <substr>`, `--skip-gpu`, `--skip-reference-data`, `--skip-deprecated`. Concurrent via `--jobs N` (default 4). Prints PASS/FAIL/ERR/SKIP per tool plus a summary table.
- `bv data verify`: HEAD-checks every dataset's primary URL, compares declared `size_bytes` to server's `Content-Length` (configurable tolerance). Concurrent via `--jobs N` (default 8).
- `RunSpec.capture_output`: when true, the runtime captures stdout/stderr instead of inheriting to the host. `bv-conformance` uses this so probe output doesn't flood the terminal during walks.

### Changed

- `bv-conformance` smoke check now counts a probe as alive if exit code is 0 OR ≥30 bytes of output were captured. Catches Unix-convention tools (bwa, seqtk, fasttree) that print help to stderr and exit non-zero on unknown args.
- `DataEntry.sha256` and `DataEntry.size_bytes` are now `Option<>`. When `sha256` is absent, `bv data fetch` skips the integrity check; when `size_bytes` is absent, the progress bar uses the server's Content-Length.
- `[tool.deprecated = true]` manifests are skipped (not failed) in walker mode when `--skip-deprecated` is passed.

## [0.1.11] - 2026-04-28

### Added

- `[tool.subcommands]` manifest section: tool-namespaced launchers for multi-script tools (ML repos like genie2, AlphaFold). `bv run <toolid> <subcommand> ...args` runs the mapped argv prefix with the user's args appended verbatim. Names stay namespaced under the tool id, so generic names (`train`, `eval`) don't collide across tools and don't get global PATH shims.
- `bv run <toolid>` with no entrypoint and only subcommands prints the available subcommand list.
- `bv show` and `bv run --info` display the subcommand table when present.
- `[tool.entrypoint]` is now optional. A manifest must declare either an entrypoint, subcommands, or both.
- `bv publish` interactive scaffold prompts for subcommands and an optional default entrypoint.

### Changed

- `bv publish` interactive flow now uses a review-and-edit menu instead of linear prompts. All fields are pre-filled from `bv-publish.toml` and source hints; pick any field to edit, then choose "Confirm" to continue. Lets users go back and revise prior answers, and structurally prevents multi-line paste cascade (each edit is taken in isolation, no auto-advance).
- License field is now a Select with common SPDX identifiers (MIT, Apache-2.0, BSD-3-Clause, GPL-3.0-only, etc.) plus "Custom…" for free-form entry.
- Replaced `dialoguer` with `inquire` for input handling. Word-delete (Alt+Backspace, Ctrl+W), Home/End, and other readline-style shortcuts now work in publish prompts.

## [0.1.9] - 2026-04-28

### Added

- `PostDownloadAction::Decompress`: gunzip a single `.gz` payload into the cache (previously the only post-download verbs were `noop` and `extract`, which forced gzipped data manifests to be misclassified).
- `bv data fetch` now downloads every entry in `source_urls`, not just the first. The primary file is sha256-verified and the post-download action is applied to it; remaining files (for example, a `.tbi` index alongside a `.vcf.gz`) are placed in the cache directory as-is.
- The `Decompress` action uses `MultiGzDecoder` so bgzipped payloads (such as ClinVar's tabix-indexed VCF, where the gzip stream is many concatenated members) are fully decompressed instead of stopping at the first member.

### Fixed

- `bv run <tool-id> <args...>` now prepends the tool's `entrypoint.command` to the arguments. Previously the entrypoint was only consulted when no arguments followed, so `bv run bcftools view foo.vcf` invoked the container with `["view", "foo.vcf"]` as CMD, dropping `bcftools` and breaking biocontainers whose ENTRYPOINT expects to exec the first arg.

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
