# bv demo runbook

A step-by-step script for a recorded demo. Total target: under 5 minutes.

## Prerequisites

- macOS or Linux with Docker Desktop (or Docker Engine) running
- `bv` installed (`cargo install biov` or from a release binary)
- A terminal with reasonable font size for screen recording
- A machine with an NVIDIA GPU (>= 8 GB VRAM, CUDA 12) for ColabFold

## Part 1: Environment check (30 s)

```sh
bv doctor
```

Expected output (trimmed):

```
  Runtime
    docker     28.x.x  server 28.x.x

  Hardware
    cpu        14 logical cores
    ram        32.0 GB total
    disk       400.0 GB free
    gpu        NVIDIA RTX 4090 (24 GB VRAM  CUDA 12.4)

  Cache
    path       /Users/you/.cache/bv
    size       0 B  0 images

  Project
    bv.toml    not found in current directory
    bv.lock    not found
```

## Part 2: Add a sequence tool and call it by name (60 s)

```sh
mkdir protein-demo && cd protein-demo

cat > trpcage.fasta << 'EOF'
>trp-cage
NLYIQWLKDGGPSSGRPPPS
EOF

bv add blast
```

Expected:

```
  Updating index  done
  Pulling  blast@2.15.0
  Added    blast 2.15.0  abc123def456  38 MB
```

The blast tool exposes many binaries. Call one directly by name — no need to spell out the tool id:

```sh
bv run blastn -version
```

```
blastn: 2.15.0+
```

```sh
bv run makeblastdb -help | head -3
```

```
USAGE
  makeblastdb [-h] [-help] [-in input_file] ...
```

`bv run` looked up `blastn` and `makeblastdb` in `bv.lock`'s binary index and routed both to the blast container. You can also name the tool explicitly: `bv run blast -- blastn ...`

## Part 3: Add ColabFold (60 s)

```sh
bv add colabfold
```

Expected:

```
  Updating index  done
  Pulling  colabfold@1.6.0
  Added    colabfold 1.6.0  ...  ~4 GB
```

## Part 5: Inspect the binary routing table (15 s)

```sh
bv list --binaries
```

```
  Binary           Tool
  ----------------------------------
  blastn           blast 2.15.0
  blastp           blast 2.15.0
  blastdbcmd       blast 2.15.0
  makeblastdb      blast 2.15.0
  tblastn          blast 2.15.0
  tblastx          blast 2.15.0
  ...
  colabfold_batch  colabfold 1.6.0
```

Every entry becomes a shim in `.bv/bin/`. Both `bv run <binary>` and `bv exec <binary>` route through this table.

## Part 6: Show the project files (30 s)

```sh
cat bv.toml
```

```toml
[project]
name = "protein-demo"

[registry]
url = "https://github.com/mlberkeley/bv-registry"

[[tools]]
id = "blast"
version = "=2.15.0"

[[tools]]
id = "colabfold"
version = "=1.6.0"
```

```sh
cat bv.lock | head -30
```

```toml
version = 1

[tools.blast]
tool_id = "blast"
version = "2.15.0"
image_reference = "ncbi/blast:2.15.0"
image_digest = "sha256:abc123..."
binaries = ["blastn", "blastp", "makeblastdb", ...]

[tools.colabfold]
tool_id = "colabfold"
version = "1.6.0"
image_reference = "ghcr.io/sokrypton/colabfold:1.6.0-cuda12"
image_digest = "sha256:def456..."
binaries = ["colabfold_batch"]

[binary_index]
blastn = "blast"
blastp = "blast"
makeblastdb = "blast"
colabfold_batch = "colabfold"
...
```

Both files go into git. Any collaborator with a GPU can run `bv sync` to reproduce the exact same images by digest, and the binary index is rebuilt from the lock automatically.

## Part 7: Interactive session with bv shell (30 s)

```sh
bv shell
```

The prompt changes to signal the active environment:

```
(bv:protein-demo) $
```

All exposed binaries are now on PATH as shims:

```sh
blastn -query trpcage.fasta -subject trpcage.fasta -outfmt 6
colabfold_batch --help | head -5
exit
```

```
$
```

Exiting returns to the original shell cleanly. No deactivation needed.

## Part 8: Run the fold script with bv exec (90 s)

Scripts and pipelines use `bv exec` to get the same PATH injection without an interactive shell:

```sh
bv exec python3 fold.py
```

`fold.py` (under `docs/fold.py`) writes the FASTA, calls `colabfold_batch` via subprocess (which resolves through the shim), and prints per-residue confidence from the result JSON.

Expected output:

```
Running ColabFold on trp-cage (20 aa)...
Output directory: output/

Results:
  trp-cage_unrelaxed_rank_001_alphafold2_ptm_model_1_seed_000.pdb
  trp-cage_scores_rank_001_alphafold2_ptm_model_1_seed_000.json

pLDDT scores (per residue):
  N   88.4
  L   91.2
  Y   93.7
  ...
  S   87.6

Mean pLDDT: 89.5  (> 70 is considered confident)
Top structure written to: output/trp-cage_unrelaxed_rank_001...pdb
```

`bv exec` uses `exec(2)` on Unix, so the Python process replaces bv in the process table. Signals, exit codes, and HPC schedulers all see Python directly.

## Part 9: bv list (15 s)

```sh
bv list
```

```
  tool        version    digest        size     added
  blast       2.15.0     abc123def456  38 MB    2024-01-15 10:00
  colabfold   1.6.0      def456abc789  4.0 GB   2024-01-15 10:01
```

## Part 10: Reproduction on another machine (45 s)

```sh
cd /tmp
git clone protein-demo collab-project
cd collab-project
bv sync
```

Expected:

```
  Pulling  blast 2.15.0       ncbi/blast@sha256:abc123...
  Synced   blast 2.15.0       abc123
  Pulling  colabfold 1.6.0    ghcr.io/sokrypton/colabfold@sha256:def456...
  Synced   colabfold 1.6.0    def456
```

`bv sync` reads `bv.lock`, pulls by digest, and regenerates `.bv/bin/` so `bv exec` works immediately on the new machine.

## Part 11: CI validation (30 s)

```yaml
- run: bv sync --frozen      # asserts bv.toml and bv.lock are in sync
- run: bv lock --check       # asserts bv.lock would not change if re-generated
- run: bv exec snakemake --cores 4
```

`--frozen` catches a tool added to `bv.toml` but not locked. `--check` catches a registry manifest that changed (e.g., a CVE patch bumped the digest). `bv exec` gives Snakemake access to all project binaries without modifying the CI environment's PATH.

---

## Timing notes

| Step | Duration |
|------|----------|
| `bv doctor` | < 1 s |
| `bv add blast` | 30-60 s (Docker pull) |
| `bv add colabfold` | 2-5 min (4 GB image) |
| `bv list --binaries` | < 1 s |
| `bv shell` + exit | < 1 s |
| `bv exec python3 fold.py` on Trp-cage | 2-5 min (includes MSA API call) |
| `bv list` | < 1 s |
| `bv sync` (warm Docker cache) | < 5 s per tool |

## Machine requirements

| Feature | Mac (no GPU) | Linux + NVIDIA GPU |
|---------|-------------|-------------------|
| blast, binary routing, bv exec | Yes | Yes |
| bv shell | Yes | Yes |
| colabfold (add + sync) | Yes (`--ignore-hardware`) | Yes |
| colabfold (run) | No (needs GPU + CUDA 12) | Yes |
