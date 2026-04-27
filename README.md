# bv

**A `uv`-style tool manager for bioinformatics.**

`bv` installs bioinformatics tools as containers, pins them to exact digests in a lockfile, and makes any analysis environment reproducible with a single `bv sync`. Works with Docker on laptops and Apptainer/Singularity on HPC clusters -- the same manifest, the same lockfile, either backend.

```sh
bv add blast hmmer mmseqs2      # resolve from registry, pull images
bv run blast -- blastn -version
bv sync                          # reproduce the exact environment anywhere
```

---

## Quickstart

Requires Docker or Apptainer/Singularity and `git`. No other dependencies.

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
bv doctor                    # check environment and available runtimes

bv add blast hmmer           # pull tools, write bv.toml and bv.lock

bv run blast -- blastn -version
bv run hmmer -- hmmbuild -h

bv list                      # show installed tools with tier and digest

bv sync                      # on any other machine: reproduce exactly
```

### Example: homology search pipeline (two tools)

`bv run` mounts your current directory as `/workspace` inside the container.

```sh
mkdir homology-project && cd homology-project

# Download a sample protein sequence (human p53, ~400 aa)
curl -sL "https://rest.uniprot.org/uniprotkb/P04637.fasta" -o p53.fasta

# Add both tools at once
bv add blast hmmer

# Step 1 -- build a BLAST protein database
bv run blast -- makeblastdb \
    -in /workspace/p53.fasta \
    -dbtype prot \
    -out /workspace/p53_db

# Step 2 -- BLAST search (tabular output)
bv run blast -- blastp \
    -query /workspace/p53.fasta \
    -db    /workspace/p53_db \
    -out   /workspace/blast_hits.tsv \
    -outfmt 6

# Step 3 -- build an HMM profile from the BLAST hits
bv run hmmer -- hmmbuild /workspace/p53.hmm /workspace/p53.fasta

# Step 4 -- search with the HMM profile
bv run hmmer -- hmmsearch \
    /workspace/p53.hmm \
    /workspace/p53.fasta \
    > /workspace/hmmer_hits.txt

cat blast_hits.tsv
```

Your project directory:

```
homology-project/
  bv.toml          # declares blast + hmmer
  bv.lock          # pinned image digests
  p53.fasta
  p53_db.*         # BLAST database files
  blast_hits.tsv
  p53.hmm
  hmmer_hits.txt
```

Commit the project files; collaborators reproduce the exact environment:

```sh
git add bv.toml bv.lock
git commit -m "pin analysis environment"

# On another machine:
git clone <your-repo> && cd homology-project
bv sync          # pulls exact pinned images by digest
bv run blast -- blastp -query /workspace/p53.fasta ...
```

---

## Discovery: `bv search` and the registry website

```sh
# Search for tools by name, description, or I/O type
bv search blast
bv search fasta                # find tools that accept FASTA input
bv search --tier core          # only core-tier tools
bv search alphafold --tier all # include experimental tier

# Browse the full registry with filters at:
# https://mlberkeley.github.io/bv-registry/
```

Each tool in the registry carries a `tier`:

| Tier | Meaning |
|------|---------|
| `core` | Typed I/O complete, from a recognized publisher, actively maintained |
| `community` | Typed I/O present, basic checks pass |
| `experimental` | Basic checks pass; may lack typed I/O. Hidden by default. |

---

## Typed I/O and tool introspection

Manifests declare typed inputs and outputs from the `bv-types` vocabulary. This powers composition, validation, and integrations.

```sh
# Human-readable schema
bv show blast

# Stable JSON output (for scripting)
bv show blast --format json

# MCP tool descriptor (for Claude and other AI assistants)
bv show blast --format mcp

# JSON Schema for the tool's inputs
bv show blast --format json-schema
```

Example MCP output:
```json
{
  "name": "blast",
  "description": "BLAST+ Basic Local Alignment Search Tool",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "description": "FASTA file path" },
      "db":    { "type": "string", "description": "BLAST database directory" }
    }
  }
}
```

---

## Backend selection: Docker and Apptainer

`bv` auto-detects the available runtime. Docker is preferred on laptops; Apptainer is preferred on HPC clusters where Docker is unavailable.

```sh
bv doctor                         # shows which runtimes are available

bv add blast --backend apptainer  # pull as a SIF file instead of a Docker image
bv run blast --backend apptainer -- blastn -version
bv sync       --backend apptainer
```

Pin the backend in `bv.toml`:

```toml
[runtime]
backend = "apptainer"             # docker | apptainer | auto (default)
```

Or use the `BV_BACKEND` environment variable:

```sh
export BV_BACKEND=apptainer
bv add blast && bv run blast -- blastn -version
```

GPU support works on both backends:

| Backend | GPU flag |
|---------|----------|
| Docker | `--gpus all` (nvidia-container-toolkit required) |
| Apptainer | `--nv` (uses host NVIDIA libraries) |

The manifest declares the GPU requirement; the runtime handles the flag automatically.

---

## Conformance testing

Every tool can carry a `[tool.test]` block. The `bv conformance` command runs the tool with canonical tiny inputs and verifies the outputs match their declared types.

```sh
bv conformance blast            # pull + run + verify outputs
bv conformance hmmer --backend apptainer
```

Conformance runs in CI on every PR to bv-registry. A tool cannot be promoted to `core` until conformance passes.

---

## Publishing a tool

```sh
# From a local directory with a Dockerfile
bv publish ./my-tool

# From a GitHub repo (auto-clones it)
bv publish github:ohuelab/QuickVina2
bv publish github:user/repo@v2.1.0

# Non-interactive (reads bv-publish.toml)
bv publish . --non-interactive

# Build and inspect the manifest without pushing
bv publish . --no-push --no-pr
```

Interactive example with a new Python tool:

```sh
mkdir my-docking-tool && cd my-docking-tool
cat > requirements.txt << 'EOF'
rdkit
numpy
EOF

bv publish .
#  Detected  requirements.txt (Python)
#  Generated Dockerfile.bv
#
#  Tool name [my-docking-tool]:
#  Version [0.1.0]:
#  Description: Fast molecular docking
#
#  Inputs
#    Add input? [y/n]: y
#    Name: ligand
#    Type (? to list): pdb
#    Mount path [/workspace/ligand]: /workspace/ligand.pdb
#    Add another? [y/n]: n
#
#  Building image as ghcr.io/bv-registry/my-docking-tool:0.1.0 ...
#  PR opened: https://github.com/mlberkeley/bv-registry/pull/143
```

For automated publishing on every GitHub release, add to `.github/workflows/bv-publish.yml`:

```yaml
on:
  release:
    types: [published]
jobs:
  publish:
    uses: mlberkeley/bv/.github/workflows/bv-publish.yml@main
    with:
      tool-name: my-docking-tool
    secrets:
      GHCR_TOKEN: ${{ secrets.GHCR_TOKEN }}
      BV_REGISTRY_TOKEN: ${{ secrets.BV_REGISTRY_TOKEN }}
```

---

## Auto-ingestion from Bioconda: `bv-ingest`

`bv-ingest` scrapes Bioconda recipes and auto-generates draft manifests for any tool that has a BioContainers image.

```sh
# Ingest 10 tools from Bioconda (dry run)
bv-ingest run --dry-run --limit 10

# Ingest a specific tool
bv-ingest run --tool samtools

# Review manifests that need typed I/O
bv-ingest review --staging-dir ./staging

# Promote a reviewed manifest to the main registry
bv-ingest promote samtools 1.20
```

The nightly GitHub Actions workflow runs automatically and opens PRs to `bv-registry` for newly discovered tools.

---

## Reference data

For tools that need large reference databases:

```sh
bv add alphafold          # bv add prints what data the tool requires
bv data fetch pdbaa --yes # download (sizes range from MB to TB)
bv run alphafold -- ...   # bv run auto-mounts the data
bv data list              # see what is cached locally
```

---

## Project files

**`bv.toml`** declares what you want:

```toml
[project]
name = "homology-project"

[registry]
url = "https://github.com/mlberkeley/bv-registry"

[runtime]
backend = "auto"          # optional; defaults to auto-detect

[[tools]]
id = "blast"
version = "=2.15.0"

[[tools]]
id = "hmmer"
```

**`bv.lock`** pins the exact state:

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

Both files belong in version control. `bv run` always uses the pinned digest.

---

## Reproducibility in CI

```yaml
- run: bv sync --frozen    # fails if bv.toml and bv.lock are inconsistent
- run: bv lock --check     # fails if bv.lock would change
```

---

## Commands

| Command | Description |
|---------|-------------|
| `bv add <tool>[@ver]` | Add tools and pull their images |
| `bv remove <tool>` | Remove a tool |
| `bv run <tool> -- <args>` | Run a tool in its container |
| `bv list` | Show installed tools with tier, digest, and size |
| `bv search <query>` | Search the registry (text, type, tier filters) |
| `bv show <tool>` | Show typed I/O schema and metadata |
| `bv info <tool>` | Show lockfile-level detail |
| `bv lock [--check]` | Regenerate bv.lock; `--check` exits 1 if anything changed |
| `bv sync [--frozen]` | Pull all locked images |
| `bv conformance <tool>` | Run the conformance test suite for a tool |
| `bv publish <source>` | Build and publish a tool to bv-registry |
| `bv data fetch <dataset>` | Download a reference dataset |
| `bv data list` | List locally cached datasets |
| `bv doctor` | Check runtimes, hardware, cache, and project state |

---

## The registry

Tools live in [mlberkeley/bv-registry](https://github.com/mlberkeley/bv-registry), a plain git repo of TOML manifests:

```
bv-registry/
  tools/
    blast/2.14.0.toml   2.15.0.toml
    hmmer/3.3.2.toml
    mmseqs2/17.0.0.toml
    alphafold/2.3.2.toml
    proteinmpnn/1.0.1.toml
  data/
    pdbaa/2024_01.toml
  index.json             # generated search index
```

Browse and filter at **https://mlberkeley.github.io/bv-registry/**

A full manifest:

```toml
[tool]
id = "blast"
version = "2.15.0"
description = "BLAST+ Basic Local Alignment Search Tool"
homepage = "https://blast.ncbi.nlm.nih.gov/Blast.cgi"
license = "Public Domain"
tier = "core"
maintainers = ["github:ncbi"]

[tool.image]
backend = "docker"
reference = "ncbi/blast:2.15.0"

[tool.hardware]
cpu_cores = 4
ram_gb = 8.0
disk_gb = 2.0

[[tool.inputs]]
name = "query"
type = "fasta"
cardinality = "one"
description = "Query sequences in FASTA format"

[[tool.outputs]]
name = "output"
type = "blast_tab"
cardinality = "one"
description = "Tabular alignment results (outfmt 6)"

[tool.entrypoint]
command = "blastn"
args_template = "-query {query} -db {db} -out {output} -num_threads {cpu_cores}"

[tool.test]
inputs = { query = "test://fasta-nucleotide" }
expected_outputs = ["output"]
timeout_seconds = 60
```

The default registry is used automatically. Override with `--registry <url>` or `BV_REGISTRY=<url>` for private registries.

---

## Workspace layout

| Crate | Role |
|---|---|
| `bv-cli` | Binary, clap CLI, command implementations |
| `bv-core` | Manifest/lockfile types, cache layout, errors |
| `bv-runtime` | `ContainerRuntime` trait + Docker implementation |
| `bv-runtime-apptainer` | Apptainer/Singularity implementation |
| `bv-index` | `IndexBackend` trait + Git registry implementation |
| `bv-types` | Bioinformatics type vocabulary (20 types) |
| `bv-conformance` | Conformance test runner for registry manifests |

---

## Development

```sh
git clone https://github.com/mlberkeley/bv
cd bv
cargo build
cargo test
cargo test --test integration -- --include-ignored   # needs Docker or Apptainer
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines.

---

## License

Apache-2.0. See [LICENSE](LICENSE).
