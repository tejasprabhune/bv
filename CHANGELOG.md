# Changelog

All notable changes to `bv` are documented here. Follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.1.15] - 2026-04-29

### Fixed (correctness / data integrity)

- **Lockfile and bv.toml are now byte-deterministic.** The 0.1.13 BTreeMap fix only covered `Manifest`; `Lockfile.tools`, `Lockfile.binary_index`, `LockfileEntry.reference_data_pins`, `BvToml.data`, and `BvToml.binary_overrides` were still `HashMap`. They reordered between `bv lock` and `bv sync`, polluted `git diff`, and caused spurious `bv lock --check` failures in CI. All five are now `BTreeMap`. Regression test re-serializes 32× and asserts identical bytes plus lexicographic key order.
- **`bv data fetch` is now atomic.** Files download into a per-fetch staging dir under `cache.tmp_dir()`; `final_dir` is only created via a single `fs::rename` after all downloads succeed and verify. RAII guard removes the staging dir on any error. A killed fetch can no longer leave a partial cache that the next invocation reports as "already cached." Resume-from-partial support has been removed (its sha256 contract was broken — the hasher only saw newly-downloaded bytes against the full-file expected digest).
- **`bv sync` no longer silently swallows drift errors.** Failures from the drift-check path now surface as a yellow `warning:` line with the underlying error and a hint to run `bv lock`; sync still proceeds.
- **`bv sync` now respects `[registry]` in `bv.toml`** for the drift-check pass. Private-registry projects were silently drift-checking against the public default.
- **Apptainer SIF cache hits are re-verified.** `pull` previously returned the requested digest with no check when `<sif_dir>/<digest>.sif` existed. It now re-hashes the file and falls through to a fresh pull on mismatch. `file_sha256` streams 64 KiB chunks via `BufReader` instead of reading the whole multi-GB SIF into memory.
- **Apptainer no longer leaks host environment.** `apptainer run` is invoked with `--cleanenv --no-home`; manifest-declared env still passes through `--env`.
- **Containers run as the calling user under Docker.** `--user $(id):$(gid)` is set so files written into host-mounted dirs aren't root-owned. Apptainer already runs as the calling user.
- **`bv-index/git.rs` warns when dropping non-semver versions.** Both `list_versions` and `list_data_versions` now log a `tracing::warn!` listing dropped filenames and the semver-compat hint, instead of silently filtering them.
- **`Manifest::from_toml_str` now runs full `validate()`.** Library callers no longer bypass structural validation. `Lockfile` and `LockfileEntry` use `serde(deny_unknown_fields)` to catch typos.

### Fixed (UX)

- **`bv add tool@2` now means `^2`** (caret), matching Cargo. Previously it was exact `=2.0.0`. Bare digits-and-dots versions are treated as caret; explicit operators (`=`, `~`, `^`, `*`, `>=`) are preserved.
- **`bv sync` pulls in parallel** (cap 3), matching `bv add`. Previously sequential — 50 tools took 50× the time of `bv add`.
- **`bv lock --check` refreshes the registry index** (TTL-based), so CI no longer misses new versions.
- **`bv search` shows deprecated tools when `--tier all` is passed.** Previously they were silently filtered out regardless of tier.
- **Conformance probes now propagate `entrypoint.env`** from the manifest, eliminating false negatives for tools that depend on `PATH`/`LD_LIBRARY_PATH` set via env.
- **`bv run` keeps the OCI tag** on digest-pinned references, matching `bv sync`. Apptainer's tag-context SIF lookup needs both paths to agree.
- **VRAM check rounds to nearest GiB** instead of flooring. Cards reporting 24268 MiB no longer fail a `min_vram_gb = 24` requirement.
- **Disk-free check picks the disk backing the bv cache root.** Previously used `max(available_space)` across all mounted disks, giving false-positives on multi-volume systems.
- **`bv doctor` shows a spinner while computing cache size**; image count math fixed (was counting every top-level cache subdir as an image).
- **`bv-cli show.rs::find_latest_version_in_dir` uses semver order**, not lexicographic. (`2.10.1` > `2.9.0`.)
- **Conformance walker uses TTL-based refresh**, matching `bv add`/`sync`/`search`. No more network round-trip on every invocation.

### Fixed (publish flow)

- **Subcommand script-path rewriter is stricter.** `--config=cfg/x.yaml`, `s3://bucket/key`, env-style `KEY=val/x`, dash-prefixed flags, and URLs are no longer mangled into `/app/...`. Only relative tokens that look like script files (extension match or dir/file pattern) are rewritten.
- **`bv publish` accepts commit SHAs.** SHA-shaped refs (7–40 hex chars) use `git fetch origin <sha>` instead of the broken `--branch <sha>`.
- **Auto-generated Python Dockerfiles include `build-essential`, `libssl-dev`, `libffi-dev`, `pkg-config`** on `python:3.11-slim-bookworm`. numpy-from-source, lxml, openssl crates no longer fail at build.
- **`bv publish` PR rerun works.** A second publish for the same `tool@version` now updates the existing file (via the contents API's `sha` field) instead of 422-ing. Branch-collision detection uses the proper status code + body parse instead of substring matching.

### Fixed (parsing)

- **`OciRef::from_str` correctly handles port-qualified registries.** `localhost:5000/foo/bar`, `localhost:5000/foo/bar:1.0`, and bare `foo:1.0` all parse correctly.
- **`atomic_write`** uses `pid + nanos + in-process counter` for tmp filenames; concurrent `bv lock` invocations no longer race on the same staging file. Cleans up on rename failure.

### Internal

- `bv-runtime` exposes `DockerRuntime::pull_verified` (pull + digest-compare). Trait-level wiring for `bv run`/`bv conform` is deferred to a follow-up.
- `bv remove` documents its bv.toml-then-lock write order as a deliberate choice — if the second write fails, `bv lock` regenerates from the (correct) bv.toml, rather than the reverse.

## [0.1.14] - 2026-04-29

### Changed

- `bv publish` auto-prefixes relative script paths in `[tool.subcommands]` with the image's `WORKDIR`. Previously, a contributor entering `python genie/train.py` produced a manifest whose argv resolved to `/workspace/genie/train.py` (the host PWD mount) instead of the actual source location inside the image. The flow now reads the (possibly auto-generated) Dockerfile, picks the last non-`/workspace` `WORKDIR` directive, and rewrites argv tokens that look like script paths (contain `/`, or end in `.py`/`.sh`/`.R`/`.pl`/`.js`/`.ts`) to be image-absolute. Already-absolute paths, command names like `python`, flags, and dotted module paths (`scripts.train`) are left alone. Applies to both the interactive prompt and `bv-publish.toml` non-interactive flow.

## [0.1.13] - 2026-04-29

### Fixed

- `Manifest::to_toml_string` is now deterministic. The four serialized maps (`tool.env`, `tool.smoke.probes`, `tool.reference_data`, `tool.subcommands`) are stored as `BTreeMap` so iteration order is lexicographic instead of HashMap's randomized order. Previously, `bv lock` and `bv sync` could compute different `manifest_sha256` for the same on-disk manifest if any of these maps were non-empty, producing a spurious "manifest has changed since lock" warning on every `bv sync` (most visible for tools with `[tool.subcommands]` like genie2).

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
