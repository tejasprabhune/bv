# bv demo runbook

A step-by-step script for a recorded demo. Total target: under 5 minutes.

## Prerequisites

- macOS or Linux with Docker Desktop (or Docker Engine) running
- `bv` installed (`cargo install biov` or from a release binary)
- A terminal with reasonable font size for screen recording
- A machine with an NVIDIA GPU (≥ 8 GB VRAM, CUDA 12) for ColabFold

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

  Data
    datasets   none downloaded yet

  Project
    bv.toml    not found in current directory
    bv.lock    not found
```

## Part 2: Set up the project and add a sequence tool (60 s)

```sh
mkdir protein-demo && cd protein-demo

# Write a small demo protein: Trp-cage miniprotein (20 aa, folds in microseconds)
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

```sh
# Quick sanity check: confirm the sequence looks right
bv run blast -- python3 -c "
from Bio import SeqIO
for r in SeqIO.parse('/workspace/trpcage.fasta', 'fasta'):
    print(r.id, len(r.seq), 'aa')
"
```

```
trp-cage 20 aa
```

## Part 3: Try AlphaFold — hit the reference data wall (30 s)

```sh
bv add alphafold --ignore-hardware
```

Expected:

```
  Updating index  done
  Pulling  alphafold@2.3.2
  Added    alphafold 2.3.2  ...

  alphafold requires the following reference datasets:
    uniref90@2023-01  68.0 GB  (required)
    bfd@1.0           1.7 TB   (required)
    pdb70@2023-01     50.0 GB  (optional)

  Fetch with: bv data fetch uniref90 bfd pdb70
```

1.7 TB of required reference data — not practical for a demo. Remove it and reach for ColabFold instead, which runs MSA through the MMseqs2 web API and needs no local databases.

```sh
bv remove alphafold
```

## Part 4: Add ColabFold (60 s)

```sh
bv add colabfold
```

Expected:

```
  Updating index  done
  Pulling  colabfold@1.6.1
  Added    colabfold 1.6.1  ...  ~4 GB
```

No reference data required. ColabFold queries the MMseqs2 server for multiple sequence alignments, then runs the AlphaFold2 neural network locally on your GPU.

## Part 5: Show the project files (30 s)

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
version = "=1.6.1"
```

```sh
cat bv.lock | head -20
```

```toml
version = 1

[tools.blast]
tool_id = "blast"
version = "2.15.0"
image_reference = "ncbi/blast:2.15.0"
image_digest = "sha256:abc123..."

[tools.colabfold]
tool_id = "colabfold"
version = "1.6.1"
image_reference = "ghcr.io/sokrypton/colabfold:1.6.1-cuda12"
image_digest = "sha256:def456..."
```

Both files go into git. Any collaborator with a GPU can run `bv sync` to reproduce the exact same images by digest.

## Part 6: Fold the protein (90 s)

```sh
python3 fold.py
```

`fold.py` is included in this repo under `docs/fold.py`. It writes the FASTA, calls `bv run colabfold`, and prints the per-residue confidence (pLDDT) from the result JSON.

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
  I   92.1
  Q   90.8
  W   94.3
  L   93.1
  K   89.6
  D   87.4
  G   85.2
  G   84.9
  P   91.8
  S   88.7
  S   87.3
  G   83.1
  R   86.4
  P   90.2
  P   89.9
  P   91.1
  S   87.6

Mean pLDDT: 89.5  (> 70 is considered confident)
Top structure written to: output/trp-cage_unrelaxed_rank_001...pdb
```

## Part 7: bv list (15 s)

```sh
bv list
```

```
  tool        version    digest        size     added
  blast       2.15.0     abc123def456  38 MB    2024-01-15 10:00
  colabfold   1.6.1      def456abc789  4.0 GB   2024-01-15 10:01
```

## Part 8: Reproduction on another machine (45 s)

Simulate a collaborator:

```sh
cd /tmp
git clone protein-demo collab-project    # or copy bv.toml + bv.lock
cd collab-project
bv sync
```

Expected:

```
  Pulling  blast 2.15.0       ncbi/blast@sha256:abc123...
  Synced   blast 2.15.0       abc123
  Pulling  colabfold 1.6.1    ghcr.io/sokrypton/colabfold@sha256:def456...
  Synced   colabfold 1.6.1    def456
```

`bv sync` reads `bv.lock` and pulls by digest — no version resolution, no registry round-trips. If the image is already in Docker's local cache (e.g., shared layer cache on a cluster), the pull is instant.

## Part 9: CI validation (30 s)

```yaml
- run: bv sync --frozen   # asserts bv.toml and bv.lock are consistent
- run: bv lock --check    # asserts bv.lock would not change if re-generated
```

`--frozen` catches the case where someone added a tool to `bv.toml` but forgot to lock. `--check` catches the case where the registry manifest changed (e.g., a CVE patch bumped the image digest). Both exit 1 in CI, forcing an explicit re-lock before merging.

---

## Timing notes

| Step | Duration |
|------|----------|
| `bv doctor` | < 1 s |
| `bv add blast` | 30-60 s (Docker pull) |
| `bv add alphafold --ignore-hardware` | 60-300 s (8 GB image) |
| `bv remove alphafold` | < 1 s |
| `bv add colabfold` | 2-5 min (4 GB image) |
| `bv run colabfold` on Trp-cage | 2-5 min (includes MSA API call) |
| `bv list` | < 1 s |
| `bv sync` (warm Docker cache) | < 5 s per tool |

## Machine requirements

| Feature | Mac (no GPU) | Linux + NVIDIA GPU |
|---------|-------------|-------------------|
| blast | Yes | Yes |
| colabfold (add + sync) | Yes (`--ignore-hardware`) | Yes |
| colabfold (run) | No (needs GPU + CUDA 12) | Yes |
