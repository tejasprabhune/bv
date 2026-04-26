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
- Color output uses `owo_colors` with `if_supports_color` — this strips ANSI in CI automatically
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
