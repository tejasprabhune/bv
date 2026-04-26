# bv

**A `uv`-style tool manager for bioinformatics.**

`bv` installs bioinformatics tools as Docker containers, pins them to exact digests in a lockfile, and makes any analysis environment reproducible with a single `bv sync`. Think `uv` for Python, but for BLAST, HMMER, AlphaFold, and their reference databases.

```
bv add blast hmmer          # pull tools into a project
bv run blast -- blastn -version
bv sync                     # reproduce the exact environment anywhere
```

---

## Quickstart

Requires Docker and `git`. No other dependencies.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/mlberkeley/bv/main/install.sh | sh
```

Or with Cargo:

```sh
cargo install biov
```

### Five commands to a reproducible analysis

```sh
# 1. Check your environment
bv doctor

# 2. Add tools (pulls Docker images, writes bv.toml and bv.lock)
bv add blast hmmer

# 3. Run a tool
bv run blast -- blastn -version

# 4. See what is installed
bv list

# 5. On any other machine with Docker: reproduce the exact environment
bv sync
```

### Example: protein sequence search from scratch

`bv run` mounts your current directory as `/workspace` inside the container.
Any file you put in the project folder is accessible at `/workspace/<filename>`.

```sh
mkdir protein-project && cd protein-project

# Download a sample sequence (human p53 tumor suppressor, ~400 aa)
curl -sL "https://rest.uniprot.org/uniprotkb/P04637.fasta" -o p53.fasta

# Add BLAST
bv add blast

# Build a local BLAST database from the sequence
bv run blast -- makeblastdb \
    -in /workspace/p53.fasta \
    -dbtype prot \
    -out /workspace/p53_db

# Search p53 against that database
bv run blast -- blastp \
    -query /workspace/p53.fasta \
    -db /workspace/p53_db \
    -out /workspace/results.txt \
    -outfmt 6

cat results.txt
```

Your project directory now looks like:

```
protein-project/
  bv.toml        # declares blast
  bv.lock        # pinned image digest
  p53.fasta      # your input
  p53_db.*       # generated database files
  results.txt    # output
```

Commit the project files so collaborators can reproduce the exact environment:

```sh
git add bv.toml bv.lock
git commit -m "pin analysis environment"
```

A collaborator on a different machine:

```sh
git clone <your-repo> && cd protein-project
bv sync                  # pulls the pinned blast image by digest
bv run blast -- blastp \ # identical binary, identical results
    -query /workspace/p53.fasta \
    -db /workspace/p53_db \
    -out /workspace/results.txt \
    -outfmt 6
```

---

## Project files

`bv add` creates two files that belong in version control:

**`bv.toml`** - what you declare:

```toml
[project]
name = "protein-project"

[registry]
url = "https://github.com/mlberkeley/bv-registry"

[[tools]]
id = "blast"
version = "=2.15.0"

[[tools]]
id = "hmmer"
```

**`bv.lock`** - what bv pins:

```toml
version = 1

[tools.blast]
tool_id = "blast"
version = "2.15.0"
image_reference = "ncbi/blast:2.15.0"
image_digest = "sha256:abc123..."
manifest_sha256 = "sha256:def456..."
resolved_at = "2024-01-15T10:00:00Z"
```

`bv run` always uses the pinned digest, not a mutable tag - so `bv sync` + `bv run` are bit-for-bit reproducible.

---

## Reference data

For tools that need large reference databases:

```sh
# bv add tells you what data a tool needs
bv add alphafold

# Download reference data (opt-in - sizes range from MB to TB)
bv data fetch pdbaa --yes

# bv run auto-mounts the data at the path the tool expects
bv run alphafold -- ...

# See what is cached locally
bv data list
```

---

## Reproducibility workflow

| Step | Command | Who runs it |
|------|---------|-------------|
| Set up environment | `bv add blast hmmer` | Author |
| Commit project files | `git add bv.toml bv.lock` | Author |
| Reproduce environment | `bv sync` | Collaborator / CI |
| Run in locked environment | `bv run blast -- ...` | Anyone |
| Validate in CI | `bv lock --check` | CI |

```yaml
# In GitHub Actions:
- run: bv sync --frozen   # fails if bv.toml and bv.lock are inconsistent
- run: bv lock --check    # fails if bv.lock would change
```

---

## Commands

| Command | Description |
|---------|-------------|
| `bv add <tool>[@version]` | Add tools and pull their images |
| `bv remove <tool>` | Remove a tool from the project |
| `bv run <tool> -- <args>` | Run a tool in its container |
| `bv list` | List installed tools with versions and digests |
| `bv lock [--check]` | Regenerate bv.lock; `--check` exits 1 if anything changed |
| `bv sync [--frozen]` | Pull all locked images; `--frozen` validates consistency |
| `bv data fetch <dataset>[@ver]` | Download a reference dataset |
| `bv data list` | List locally cached reference datasets |
| `bv doctor` | Check Docker, hardware, cache, and project state |

---

## The registry

Tools are defined in [mlberkeley/bv-registry](https://github.com/mlberkeley/bv-registry), a plain git repo of TOML manifests:

```
bv-registry/
  tools/
    blast/2.14.0.toml   2.15.0.toml
    hmmer/3.3.2.toml
    alphafold/2.3.2.toml
  data/
    pdbaa/2024_01.toml
```

Each manifest declares the Docker image, hardware requirements, optional reference data, and entrypoint:

```toml
[tool]
id = "blast"
version = "2.15.0"
description = "NCBI BLAST+ sequence alignment tools"

[tool.image]
backend = "docker"
reference = "ncbi/blast:2.15.0"

[tool.hardware]
cpu_cores = 4
ram_gb = 8.0

[tool.entrypoint]
command = "blastn"
```

The default registry (`https://github.com/mlberkeley/bv-registry`) is used automatically. Override with `--registry <url>` or `BV_REGISTRY=<url>` for private registries.

### Contributing a manifest

1. Fork [mlberkeley/bv-registry](https://github.com/mlberkeley/bv-registry)
2. Add `tools/<name>/<version>.toml` (use an existing manifest as a template)
3. Verify the image is publicly accessible on Docker Hub or GHCR
4. Open a PR - one tool per PR

---

## Comparison to alternatives

| | bv | conda/mamba | Docker alone | Nextflow / Snakemake |
|---|---|---|---|---|
| Reproducible by digest | Yes | No | Requires scripting | Partial |
| Binary isolation | Yes | Partial | Yes | Yes |
| Project-scoped lockfile | Yes | No | No | Config-level |
| Hardware requirement checks | Yes | No | No | No |
| Reference data management | Yes | No | No | No |
| Parallel multi-tool pull | Yes | No | No | N/A |
| Single static binary | Yes | No | No | No |

---

## Install methods

### Curl installer (recommended)

```sh
curl -fsSL https://raw.githubusercontent.com/mlberkeley/bv/main/install.sh | sh
```

Installs the latest release binary to `~/.local/bin/bv`.

### Cargo

```sh
cargo install biov
```

### Build from source

```sh
git clone https://github.com/mlberkeley/bv
cd bv
cargo build --release
cp target/release/bv ~/.local/bin/
```

---

## Development

```sh
git clone https://github.com/mlberkeley/bv
cd bv
cargo build                          # debug build
cargo test                           # unit tests
cargo test --test integration -- --include-ignored   # integration tests (needs Docker)
```

Workspace layout:

| Crate | Role |
|---|---|
| `bv-cli` | Binary, clap CLI, commands |
| `bv-core` | Manifest/lockfile types, cache layout, errors |
| `bv-runtime` | `ContainerRuntime` trait + Docker implementation |
| `bv-index` | `IndexBackend` trait + Git registry implementation |

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines.

---

## Roadmap

Shipped in v0.1:
- Project-scoped tool management (add, remove, run, lock, sync)
- Docker backend with image digest pinning
- Reference data download and auto-mount
- Git-backed registry with hardware requirement checking

Coming later:
- `bv search` - search registry from the CLI
- Apptainer/Singularity backend for HPC clusters without Docker
- Hosted registry with mirrored images
- Manifest generator from Biocontainers metadata
- Global install mode (`bv install` without a project directory)
- Resume interrupted reference data downloads
- Cache pruning (`bv cache prune`)

---

## License

Apache-2.0. See [LICENSE](LICENSE).
