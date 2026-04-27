# Protein folding with `bv`

Work with protein folding in under 3 min with `bv`. You'll need a GPU for this demo!

## Prerequisites

- macOS or Linux with Docker (running) or Apptainer.
- `bv` installed (`cargo install biov` or from a release binary)
- A machine with an NVIDIA GPU (>= 8 GB VRAM, CUDA 12) for ColabFold

## tl;dr

```sh
# install bv (one of the following)
curl -fsSL https://raw.githubusercontent.com/mlberkeley/bv/main/install.sh | sh
cargo install biov

mkdir protein-demo && cd protein-demo
bv add colabfold
# copy fold.py from this docs dir into protein-demo/
bv exec python3 fold.py

git add bv.toml bv.lock
git commit -m "Add reproducible experiment"
git push origin main

# now anyone who pulls your repo can run:
bv sync
```

## Part 1: Add sequence tools and test them

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
  note: no bv.toml found, creating one
  Updating index done
⠙ Pulling blast@2.15.0
  Pulled ncbi/blast:2.15.0
  Added blast 2.15.0  77a24a340683
```

The blast tool exposes many binaries. Call one directly by name:

```sh
bv run blastn -version
blastn: 2.15.0+
 Package: blast 2.15.0, build Nov 21 2023 21:05:41
```

`bv run` looked up `blastn` and `makeblastdb` in `bv.lock`'s binary index and routed both to the blast container.
You can also name the tool explicitly: `bv run blast -- blastn ...`

## Part 2: Add ColabFold

```sh
bv add colabfold
```

Expected:

```
  Updating index  done
  Pulling  colabfold@1.6.0
  Added    colabfold 1.6.0  ...  ~4 GB
```

## Part 3: Inspect binaries

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

## Part 4: Show project files

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
binaries = ["blastn", "blastp", "makeblastdb", "tblastn", "tblastx"]

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
# ... (one entry per binary)
colabfold_batch = "colabfold"
```

Both files go into git. Any collaborator with a GPU can run `bv sync` to reproduce the exact same images by digest, and the binary index is rebuilt from the lock automatically.

## Part 5: Interactive session with bv shell

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
$
```

Exiting returns to the original shell cleanly. No deactivation needed.

## Part 6: Run the fold script with bv exec

Scripts and pipelines use `bv exec` to get the same PATH injection without an interactive shell:

```sh
bv exec python3 fold.py
```

`fold.py` (under `docs/fold.py`) writes the FASTA, calls `colabfold_batch` via subprocess (which resolves through the shim), and prints per-residue confidence from the result JSON.

Expected output:

```
Running ColabFold on trp-cage (20 aa)...
Output directory: output/

2026-04-27 19:27:16,481 Running colabfold 1.6.0
Downloading alphafold2_ptm weights to /cache/colabfold: 100%|██████████| 3.47G/3.47G [00:14<00:00, 259MB/s]

...

Results:
  trp-cage_unrelaxed_rank_001_alphafold2_ptm_model_5_seed_000.pdb
  ...
  trp-cage_scores_rank_003_alphafold2_ptm_model_3_seed_000.json

pLDDT scores (per residue):
  N   87.8
  ...
  S   89.2

Mean pLDDT: 94.9  (> 70 is considered confident)

Top structure written to: output/trp-cage_unrelaxed_rank_001_alphafold2_ptm_model_5_seed_000.pdb
```

`bv exec` uses `exec(2)` on Unix, so the Python process replaces bv in the process table. Signals, exit codes, and HPC schedulers all see Python directly.

## Part 7: Reproduction on another machine (45 s)

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

## Part 8: CI validation (optional)

```yaml
- run: bv sync --frozen      # asserts bv.toml and bv.lock are in sync
- run: bv lock --check       # asserts bv.lock would not change if re-generated
- run: bv exec snakemake --cores 4
```

`--frozen` catches a tool added to `bv.toml` but not locked. `--check` catches a registry manifest that changed (e.g., a CVE patch bumped the digest). `bv exec` gives Snakemake access to all project binaries without modifying the CI environment's PATH.
