# bv: a `uv` for bioinformatics

`bv` is a fast, project-scoped tool manager for bioinformatics pipelines. It manages containerised tools (Docker/Apptainer), reference data, and hardware requirements in a single `bv.toml` manifest.

## Naming

The installed binary is `bv`. The published crate is `bio-bv`.

## Workspace layout

| Crate | Role |
|---|---|
| `bv-cli` | Binary entry point, clap CLI |
| `bv-core` | Manifest/lockfile types, cache layout, error types |
| `bv-runtime` | `ContainerRuntime` trait + Docker implementation |
| `bv-index` | `IndexBackend` trait + Git-based registry implementation |

## Quick start

```sh
cargo build --release
./target/release/bv --help
```
