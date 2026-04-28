# bv

**A `uv`-style tool manager for bioinformatics.**

`bv` installs bioinformatics tools as containers, pins them to exact digests in a lockfile, and makes any analysis environment reproducible with a single `bv sync`. Works with Docker on laptops and Apptainer/Singularity on HPC clusters; the same manifest, the same lockfile, either backend.

```sh
bv add blast hmmer mmseqs2      # resolve from registry, pull images
bv run blastn -version          # call any binary by name directly
bv exec snakemake --cores 4     # run scripts with all tools on PATH
bv sync                         # reproduce the exact environment anywhere
```

---

## Quickstart

Requires Docker or Apptainer/Singularity and `git`. No other dependencies.

### Install a runtime

Pick whichever fits your machine. Docker is typical on laptops; Apptainer is typical on shared HPC nodes.

```sh
# Docker (rootless, Linux). On a GPU box you'll also want nvidia-container-toolkit.
curl -fsSL https://get.docker.com/rootless | sh
systemctl --user enable --now docker

# Apptainer (no root needed, works on most HPC clusters)
conda install -c conda-forge apptainer
```

### Install bv

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

bv run blastn -version       # call binaries directly by name
bv run hmmbuild -h

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

# Step 1: build a BLAST protein database
bv run makeblastdb \
    -in /workspace/p53.fasta \
    -dbtype prot \
    -out /workspace/p53_db

# Step 2: BLAST search (tabular output)
bv run blastp \
    -query /workspace/p53.fasta \
    -db    /workspace/p53_db \
    -out   /workspace/blast_hits.tsv \
    -outfmt 6

# Step 3: build an HMM profile from the BLAST hits
bv run hmmbuild /workspace/p53.hmm /workspace/p53.fasta

# Step 4: search with the HMM profile
bv run hmmsearch \
    /workspace/p53.hmm \
    /workspace/p53.fasta \
    > /workspace/hmmer_hits.txt

cat blast_hits.tsv
```

`bv run <binary>` looks up the binary name in the project's binary index and routes to the right container automatically. No need to specify the tool name.

Your project directory:

```
homology-project/
  bv.toml          # declares blast + hmmer
  bv.lock          # pinned image digests and binary index
  .bv/bin/         # generated shims (gitignored)
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
bv sync          # pulls exact pinned images by digest, regenerates shims
bv run blastp -query /workspace/p53.fasta ...
```

---

## Using tools from scripts and pipelines

### bv exec

`bv exec` runs any command with all project binaries prepended to `PATH`. It is the right form for scripts, Makefiles, and CI.

```sh
bv exec python3 pipeline.py
bv exec snakemake --cores 4
bv exec -- bash -c "blastn -query foo.fa | sort -k11 -n"
```

On Unix, `bv exec` replaces itself with the child process via `exec(2)`. Signals, exit codes, and HPC schedulers see the child directly; there is no extra layer in `ps`.

Makefile:

```make
results.tsv: query.fa db.phr
	bv exec blastn -query $< -db db -out $@ -outfmt 6
```

Snakemake:

```python
rule align:
    input:  "reads.fastq.gz"
    output: "aligned.bam"
    shell:
        "bv exec bwa mem -t {threads} ref.fa {input} "
        "| bv exec samtools sort -o {output}"
```

### bv shell

`bv shell` starts an interactive subshell with all project binaries on PATH. The prompt changes to show the active project.

```sh
bv shell
(bv:homology-project) $ blastn -query p53.fasta -db p53_db -out hits.tsv -outfmt 6
(bv:homology-project) $ hmmsearch p53.hmm p53.fasta > hmmer_hits.txt
(bv:homology-project) $ exit
$
```

Exiting the subshell returns to the original environment cleanly. `BV_ACTIVE` is set to the project name while inside, so scripts can detect activation.

```sh
bv shell --shell zsh    # explicit shell choice
```

### Binary routing

Every binary a tool exposes is listed in `bv.lock` and gets a shim in `.bv/bin/`. `bv run <binary>` and `bv exec <binary>` both route through this index.

```sh
bv list --binaries
```

```
  Binary        Tool
  ----------------------------
  blastn        blast 2.15.0
  blastp        blast 2.15.0
  makeblastdb   blast 2.15.0
  tblastn       blast 2.15.0
  hmmbuild      hmmer 3.3.2
  hmmsearch     hmmer 3.3.2
  hmmscan       hmmer 3.3.2
```

If two tools expose the same binary name, `bv lock` fails with a clear error. Resolve it in `bv.toml`:

```toml
[binary_overrides]
samtools = "samtools"   # this tool wins when multiple tools expose samtools
```

---

## Discovery: `bv search` and the registry website

```sh
# Search for tools by name, description, or I/O type
bv search blast
bv search fasta                # find tools that accept FASTA input
bv search --tier core          # only core-tier tools
bv search colabfold --tier all # include experimental tier

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
bv run blastn --backend apptainer -version
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
bv add blast && bv run blastn -version
```

GPU support works on both backends:

| Backend | GPU flag |
|---------|----------|
| Docker | `--gpus all` (nvidia-container-toolkit required) |
| Apptainer | `--nv` (uses host NVIDIA libraries) |

The manifest declares the GPU requirement; the runtime handles the flag automatically.

### Cache mounts

Apptainer runs containers with a read-only root filesystem, so any tool that downloads model weights or scratches to disk inside the image will fail (e.g. ColabFold writing to `/cache/colabfold`). bv binds writable host directories into the container for paths that the tool needs to write. The set of paths is resolved in three layers:

1. **Tool manifest** (`cache_paths` in the registry entry) — the tool author's authoritative list. ColabFold's manifest declares `cache_paths = ["/cache/colabfold"]`.
2. **User overrides** (`[[cache]]` in `bv.toml`) — point any container path at a different host directory (e.g. a shared NFS cache).
3. **Apptainer fallbacks** — for tools whose manifest hasn't declared any cache paths yet, bv auto-binds the well-known `/cache` and `/root/.cache` so common bioconda images don't fail outright.

Each container path defaults to a host directory under `~/.cache/bv/<tool>/`. Docker skips the apptainer fallbacks (writable upper layer covers the same need); manifest and user entries apply on both backends.

User overrides in `bv.toml`:

```toml
# applies to every tool; {tool} is replaced with the tool id
[[cache]]
match = "*"
container_path = "/cache"
host_path = "~/.cache/bv/{tool}"

# tool-specific: redirect colabfold weights to a shared cache
[[cache]]
match = "colabfold"
container_path = "/cache/colabfold"
host_path = "/srv/shared/colabfold-weights"
```

Tool authors declare what their image needs in the registry manifest:

```toml
[tool]
id = "colabfold"
# ...
cache_paths = ["/cache/colabfold"]
```

---

## Conformance testing

`bv conformance <tool>` pulls the tool's image and smoke-tests every binary it exposes. For each binary in `[tool.binaries]`, bv tries `--version`, `-version`, `--help`, `-h`, `-v`, `version` (in that order) and considers the binary alive if any of them exits 0. This catches broken images, missing shared libraries, and binaries that segfault on startup.

```sh
bv conformance blast
bv conformance hmmer --backend apptainer
```

Most tools need no extra config. For unusual binaries, add a `[tool.smoke]` block to the manifest:

```toml
[tool.smoke]
probes = { weird-tool = "--check" }   # pin a specific probe arg
skip   = ["server-daemon"]            # binaries with no safe probe arg
```

Conformance runs in CI on every PR to bv-registry. Today it's a smoke check only; running tools on canonical inputs and validating typed outputs is on the v2 roadmap.

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

`bv-ingest` scrapes Bioconda recipes and auto-generates draft manifests for any tool that has a BioContainers image. Binary names are extracted from recipe `test.commands` and `build.run_exports` and written into `[tool.binaries]` automatically.

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
bv add kraken2            # bv add prints what data the tool requires
bv data fetch pdbaa --yes # download (sizes range from MB to TB)
bv run kraken2 ...        # bv run auto-mounts the data
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

**`bv.lock`** pins the exact state, including the binary routing index:

```toml
version = 1

[tools.blast]
tool_id = "blast"
version = "2.15.0"
image_reference = "ncbi/blast:2.15.0"
image_digest = "sha256:abc123..."
manifest_sha256 = "sha256:def456..."
resolved_at = "2024-01-15T10:00:00Z"
binaries = ["blastn", "blastp", "makeblastdb", "tblastn", "tblastx"]

[tools.hmmer]
tool_id = "hmmer"
version = "3.3.2"
image_reference = "quay.io/biocontainers/hmmer:3.3.2--h87f3376_2"
image_digest = "sha256:789abc..."
binaries = ["hmmbuild", "hmmsearch", "hmmscan", "jackhmmer", "phmmer"]

[binary_index]
blastn = "blast"
blastp = "blast"
makeblastdb = "blast"
hmmbuild = "hmmer"
hmmsearch = "hmmer"
hmmscan = "hmmer"
```

Both files belong in version control. `bv run` always uses the pinned digest. `.bv/` (the generated shim directory) is gitignored automatically.

---

## Reproducibility in CI

```yaml
- run: bv sync --frozen    # fails if bv.toml and bv.lock are inconsistent
- run: bv lock --check     # fails if bv.lock would change
- run: bv exec snakemake --cores 4
```

---

## Commands

| Command | Description |
|---------|-------------|
| `bv add <tool>[@ver]` | Add tools and pull their images |
| `bv remove <tool>` | Remove a tool |
| `bv run <binary|tool> [<args>]` | Run a binary or tool in its container |
| `bv exec <command>` | Run a command with all project binaries on PATH |
| `bv shell [--shell <sh>]` | Start an interactive subshell with binaries on PATH |
| `bv list` | Show installed tools with tier, digest, and size |
| `bv list --binaries` | Show the binary routing table |
| `bv search <query>` | Search the registry (text, type, tier filters) |
| `bv show <tool>` | Show typed I/O schema and metadata |
| `bv info <tool>` | Show lockfile-level detail |
| `bv lock [--check]` | Regenerate bv.lock; `--check` exits 1 if anything changed |
| `bv sync [--frozen]` | Pull all locked images and regenerate shims |
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
    colabfold/1.6.0.toml
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

[tool.binaries]
exposed = [
  "blastn", "blastp", "tblastn", "tblastx",
  "makeblastdb", "blastdbcmd", "blastdb_aliastool",
]

```

The `[tool.smoke]` block is optional and only needed for unusual binaries. Most manifests omit it.

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
