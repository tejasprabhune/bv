# bv demo runbook

A step-by-step script for a recorded demo. Total target: under 5 minutes.

## Prerequisites

- macOS or Linux with Docker Desktop (or Docker Engine) running
- `bv` installed (`cargo install biov` or from a release binary)
- A terminal with reasonable font size for screen recording

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
    gpu        none detected

  Cache
    path       /Users/you/.cache/bv
    size       0 B  0 images

  Data
    datasets   none downloaded yet

  Project
    bv.toml    not found in current directory
    bv.lock    not found
```

**Talking points:** `bv doctor` is the first thing to run on a new machine. Shows Docker is up, hardware is fine, and the project is clean. Like `uv run --dry-run` but for the whole environment.

## Part 2: Add a fast tool and run a real analysis (90 s)

`bv run` mounts your project directory as `/workspace` inside the container,
so any file you put in the folder is accessible at `/workspace/<filename>`.

```sh
mkdir demo-project && cd demo-project

# Download a sample protein sequence (human p53, ~400 aa)
curl -sL "https://rest.uniprot.org/uniprotkb/P04637.fasta" -o p53.fasta

bv add blast
```

Expected:

```
  Updating index  done
  Pulling  blast@2.15.0
  Added    blast 2.15.0  abc123def456  38 MB
```

```sh
# Build a local BLAST database
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

**Talking points:** `bv add` pulls from the registry, locks to a digest, writes `bv.toml` and `bv.lock`. `bv run` mounts `$PWD` as `/workspace`; files you put in the project dir are immediately available inside the container without any Docker flags.

## Part 3: Add multiple tools in parallel (30 s)

```sh
bv add hmmer mmseqs2
```

Expected (order may vary, pulls happen concurrently):

```
  Updating index  done
  Pulling  hmmer@3.3.2
  Pulling  mmseqs2@17.0.0
  Added    hmmer 3.3.2   ...  120 MB
  Added    mmseqs2 17.0.0  ...  500 MB
```

**Talking points:** Pulls are concurrent (tokio + semaphore, max 3). You don't wait for one image before the next starts.

## Part 4: Show the project files (30 s)

```sh
cat bv.toml
```

```toml
[project]
name = "demo-project"

[registry]
url = "https://github.com/mlberkeley/bv-registry"

[[tools]]
id = "blast"
version = "=2.15.0"

[[tools]]
id = "hmmer"

[[tools]]
id = "mmseqs2"
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
```

**Talking points:** These two files capture the full environment. Commit both to git and anyone can reproduce this exact setup.

## Part 5: GPU tool and reference data notice (45 s)

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

**Talking points:** `bv add` warns about reference data but does not auto-download; these are terabyte-scale databases. The user opts in with `bv data fetch`. On a machine with a GPU, drop `--ignore-hardware`.

## Part 6: Reference data (30 s, GPU machine or skip)

```sh
bv data fetch pdbaa --yes
```

Expected:

```
  Fetching pdbaa@2024_01
  [============================>   ] 58 MB/70 MB  00:02
  Fetched  pdbaa@2024_01  /Users/you/.cache/bv/data/pdbaa/2024_01
```

```sh
bv data list
```

```
  dataset                version         size
  pdbaa                  2024_01         70 MB
```

## Part 7: bv list (15 s)

```sh
bv list
```

```
  tool       version       digest        size     added
  alphafold  2.3.2         a1b2c3d4e5f6  8.0 GB   2024-01-15 10:03
  blast      2.15.0        abc123def456  38 MB    2024-01-15 10:00
  hmmer      3.3.2         deadbeef1234  120 MB   2024-01-15 10:01
  mmseqs2    14.7564.0     cafebabe9876  500 MB   2024-01-15 10:01
```

## Part 8: Reproduction on another machine (45 s)

Simulate a collaborator:

```sh
cd /tmp
git clone demo-project collab-project    # or copy bv.toml + bv.lock
cd collab-project
bv sync
```

Expected:

```
  Present  alphafold 2.3.2  (already in Docker cache)
  Pulling  blast 2.15.0  ncbi/blast@sha256:abc123...
  Synced   blast 2.15.0  abc123
  Synced   hmmer 3.3.2   deadbeef
  Synced   mmseqs2 14.7564.0  cafebabe
```

**Talking points:** `bv sync` does not re-resolve versions or pull from the registry; it reads `bv.lock` and pulls by digest. If the image is already in Docker's local cache (e.g., on the same machine or a shared layer cache), it skips the pull. This is what makes `bv sync` fast in practice.

## Part 9: CI validation (30 s)

Show the CI commands:

```yaml
- run: bv sync --frozen   # asserts bv.toml and bv.lock are consistent
- run: bv lock --check    # asserts bv.lock would not change if re-generated
```

**Talking points:** `--frozen` catches the case where someone added a tool to `bv.toml` but forgot to lock. `--check` catches the case where the registry manifest changed (e.g., a CVE patch bumped the image digest). Both exit 1 in CI, forcing an explicit re-lock before merging.

---

## Timing notes

| Step | Duration |
|------|----------|
| `bv doctor` | < 1 s |
| `bv add blast` | 30-60 s (Docker pull) |
| `bv run blast -- blastn -version` | 2-5 s |
| `bv add hmmer mmseqs2` | 30-90 s (parallel pulls) |
| `bv add alphafold --ignore-hardware` | 60-300 s (8 GB image) |
| `bv data fetch pdbaa --yes` | 30-120 s (70 MB) |
| `bv list` | < 1 s |
| `bv sync` (warm Docker cache) | < 5 s per tool |

For a 5-minute recording, use blast + hmmer (small images) and skip alphafold/reference data unless on a machine that already has the images cached.

## Machine requirements

| Feature | Mac (no GPU) | Linux + NVIDIA GPU |
|---------|-------------|-------------------|
| blast, hmmer, mmseqs2 | Yes | Yes |
| alphafold (add + sync) | Yes (--ignore-hardware) | Yes |
| alphafold (run) | No (needs GPU) | Yes |
| bv data fetch | Yes | Yes |
