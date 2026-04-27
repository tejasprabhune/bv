# Contributing to bv

## Development setup

```sh
git clone https://github.com/mlberkeley/bv
cd bv
cargo build
cargo test
```

Integration tests require a running Docker daemon:

```sh
cargo test --test integration -- --include-ignored
```

For faster iteration, set `BV_CACHE_DIR` to a temp directory so each test run gets a clean cache:

```sh
BV_CACHE_DIR=/tmp/bv-test cargo test --test integration -- --include-ignored
```

## Workspace layout

| Crate | Purpose |
|-------|---------|
| `bv-core` | Shared types: manifests, lockfile, cache layout, errors |
| `bv-index` | `IndexBackend` trait + `GitIndex` implementation |
| `bv-runtime` | `ContainerRuntime` trait + `DockerRuntime` implementation |
| `bv-cli` | Binary entry point, all commands |

## Code conventions

- Rust edition 2024; let chains are used throughout
- All user-visible output goes to `stderr`; only table data (`bv list`) goes to `stdout`
- Color output uses `owo_colors` with `if_supports_color`; strips ANSI in CI automatically
- No em-dashes in comments or strings; no `// ----` separator blocks
- No multi-line comments where a well-named function or type suffices

## Adding a command

1. Add a variant to the relevant `Commands` or sub-command enum in `bv-cli/src/cli.rs`
2. Add a `pub (async) fn run(...)` in `bv-cli/src/commands/<name>.rs`
3. Wire it up in `bv-cli/src/commands/mod.rs` and `bv-cli/src/main.rs`
4. Add an integration test in `bv-cli/tests/integration.rs` (mark `#[ignore]` if it needs Docker)

## Adding a tool to the registry

See [mlberkeley/bv-registry](https://github.com/mlberkeley/bv-registry) for the registry contribution guide.

## PR conventions

- One logical change per PR
- Integration tests must pass (`cargo test --test integration -- --include-ignored`)
- `cargo clippy -- -D warnings` must be clean
- `cargo fmt --check` must pass

## Releasing

### 1. Prepare

Bump the version in `Cargo.toml` (workspace root). All crates inherit it via `[workspace.package]`, so one edit is enough.

Update `CHANGELOG.md`: add a `## [x.y.z] - YYYY-MM-DD` heading and summarise what changed.

Run the full test suite one last time:

```sh
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

Commit both files:

```sh
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "Release vx.y.z"
```

### 2. Publish to crates.io

The workspace crates must be published in dependency order because each one must exist on crates.io before dependents can reference it. Run these in sequence, waiting for each to finish:

```sh
cargo publish -p bv-types
cargo publish -p bv-core
cargo publish -p bv-runtime
cargo publish -p bv-runtime-apptainer
cargo publish -p bv-index
cargo publish -p bv-conformance
cargo publish -p biov          # the bv-cli crate (binary is named bv)
```

If `cargo publish` flags a crate as already up to date (unchanged since last release), skip it.

### 3. Tag and push

```sh
git tag vx.y.z
git push origin main --tags
```

Pushing the tag triggers the `release.yml` workflow, which builds binaries for all four targets (x86_64/aarch64, Linux/macOS) and creates a GitHub release with them attached. Release notes are generated automatically from commits.

### 4. Verify

- Check the [Actions tab](https://github.com/mlberkeley/bv/actions) to confirm the release workflow passed.
- Confirm the new version appears at `https://crates.io/crates/biov`.
- Run `cargo install biov` on a clean machine (or `cargo install biov --version x.y.z`) to sanity-check the published binary.
- On a Linux x86_64 machine, run `install.sh` and confirm it downloads the musl binary and that `bv --version` works.
