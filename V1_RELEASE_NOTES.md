# v1 release prep: known limitations and sanity tests

> Temp doc. Delete when shipped.

## Part 1: things to revisit for v2

Your list (kept):

- **SIF file hash not stable across apptainer versions/build flags.** `bv-runtime-apptainer/src/cache.rs::file_sha256` returns the SIF file's sha256, which changes per host even when the upstream OCI image is identical. Cross-machine `bv sync --frozen` will misreport drift. Fix: store the registry manifest digest in `bv.lock` (resolve via `skopeo inspect` or a tiny HTTP HEAD), keep file sha256 only as a local content-address.
- **Mirror datasets** behind `bv data fetch`. Today it points straight at upstream URLs in the manifest; if the FTP server changes or rate-limits, every user breaks at once.
- **Hosted index** instead of a `git pull` of the registry. The current `IndexBackend = git` works but is slow on the first `bv add` and has no rate-limiting story for a popular registry.
- **Hosted website with login** so all images land in one canonical GHCR namespace instead of the publisher's personal one (the v1 default we just shipped).

Things I noticed while reading through:

### Security and trust

- **`--require-signed` is a stub.** `bv-cli/src/commands/add.rs::verify_signature` only checks that the manifest carries `signatures.image == "sigstore"`. It never invokes `cosign verify`. Either wire up real verification or remove the flag for v1 to avoid implying a guarantee that doesn't hold.
- **Image durability for community-tier tools.** With publishers pushing to `ghcr.io/<their-username>/...`, if the publisher deletes the repo or leaves GitHub, every user breaks on `bv sync`. Need a mirror-on-PR-merge CI job (probably `skopeo copy` to `ghcr.io/bv-registry-mirror/...`) and have `bv lock` rewrite `image.reference` to the mirror at lock time.
- **No PR rate-limiting / first-PR review on bv-registry.** Anyone can spam-open PRs. Enable GitHub's "require approval for first-time contributors" and consider CODEOWNERS so only declared maintainers can update an existing tool's directory.
- **No integrity check on `bv data fetch`.** Manifests declare `size_bytes` but not a sha256. A swapped dataset goes undetected.

### Correctness

- **`base_image_ref` is heuristic.** `bv-cli/src/ops.rs:225` uses `rfind(':')` to strip tags and tolerates port numbers, but a malformed lockfile entry could surprise it. Worth a few more unit tests with edge cases (port + tag + digest, empty path components).
- **Concurrent `bv` invocations in the same project** can race `.bv/bin/` regeneration. No advisory lock today. First user to hit it loses shims or gets a half-written file.
- **`bv lock --check` on apptainer** will spuriously report drift the moment SIF file hashes diverge across machines (same root cause as the v2 hash issue).

### Feature gaps users will hit fast

- **No `bv update <tool>` / `bv upgrade`.** To bump versions you have to remove and re-add. Cargo has `cargo update` + `cargo upgrade`; ours doesn't.
- **No `bv cache prune` / GC.** `~/.cache/bv/<tool>/` and the SIF cache grow unbounded. A user who has tried 30 tools is paying disk for 25 they don't use.
- **Single-platform builds.** `bv publish --platform amd64` only. ARM laptops + x86 HPC means we either need multi-arch manifests or per-arch tags. Today nothing warns the user when an x86-only image won't run.
- **GHCR anonymous pull rate-limit.** A fresh CI runner doing `bv add` for many tools will hit GHCR's anonymous limit. Need either a "log in to ghcr first" hint in `bv doctor` or auth on by default in CI mode.
- **Conformance is smoke-only.** `bv conformance` today just probes binaries with `--version` / `--help` etc. No canonical inputs, no typed output validation. Build out a hosted test-fixture pipeline (downloadable on demand: `test://fasta-nucleotide`, `test://blast-db-nt-tiny`, ...) and gate `core` promotion on real I/O conformance passing on both backends.
- **bv-ingest produces no typed I/O.** Every promotion requires a human to read upstream docs and label inputs. Heuristics on bioconda recipe `test.commands` (`*.fa`, `*.fastq`, `*.bam`) would auto-suggest types and cut review time by half.

### Nice-to-haves

- **`bv doctor` covers runtimes; doesn't cover GHCR auth or registry reachability.** Add probes for: registry git clone works, GHCR pull works, `~/.cache/bv` writable, NVIDIA toolkit if GPU declared.
- **`bv exec` doesn't tell the user when a binary is shadowed.** If both `samtools` from `samtools` tool and `samtools` from `bcftools` exist, the silent winner is whoever happens to be first. We do error in `bv lock`, but `bv exec` should print which container resolved a binary on `--verbose`.
- **No `bv why <binary>`** that says "this shim resolves to colabfold@1.6.0 because of bv.toml line N". Helpful when shims surprise people.

---

## Part 2: sanity tests per feature

Goal: each test is self-contained and runnable from a clean shell. Where a test needs network or GPU, it's flagged. Run them in this order; each builds on the previous.

Most tests assume:

```sh
cargo install --path bv-cli --locked
mkdir -p /tmp/bv-tests && cd /tmp/bv-tests
```

### 1. `bv doctor` — environment probe

```sh
mkdir t-doctor && cd t-doctor && bv doctor
```

Pass: prints both runtimes' versions if present, flags missing ones, exits 0. Should not crash if neither is installed.

### 2. `bv add` + `bv run` — minimal install loop *(network, docker)*

```sh
mkdir t-add && cd t-add
bv add blast                      # small, no GPU, ~700MB
bv list                           # blast 2.15.0 visible
bv list --binaries                # blastn, blastp, makeblastdb, etc.
bv run blastn -version            # prints "blastn: 2.15.0+"
test -f bv.toml && test -f bv.lock || echo FAIL
cd ..
```

Pass: `blastn -version` runs; `bv.toml` and `bv.lock` are created; `.bv/bin/blastn` exists.

### 3. `bv exec` — PATH injection

```sh
cd t-add
bv exec which blastn              # prints .../t-add/.bv/bin/blastn
bv exec blastn -version           # same as bv run, but via PATH
bv exec bash -c 'echo $BV_PROJECT_ROOT'   # prints cwd
cd ..
```

Pass: `which` resolves to the shim; bash sees `BV_PROJECT_ROOT` set.

### 4. `bv shell` — interactive subshell *(manual)*

```sh
cd t-add
bv shell
# inside the subshell:
echo $BV_ACTIVE          # should print "t-add"
which blastn             # shim path
exit
cd ..
```

Pass: prompt changes (e.g. `(bv:t-add)`); `$BV_ACTIVE` set; `exit` returns cleanly.

### 5. `bv sync` — reproduce on a new machine *(network)*

```sh
cp -r t-add t-sync && cd t-sync
rm -rf .bv                        # nuke shims
bv sync                           # should re-pull and rebuild shims
bv run blastn -version
cd ..
```

Pass: shims regenerated; `blastn` works.

### 6. `bv lock --check` — drift detection

```sh
cd t-add
bv lock --check                   # exit 0
echo '[[tools]]' >> bv.toml
echo 'id = "hmmer"' >> bv.toml
bv lock --check                   # exit 1 with a clear diff
git checkout bv.toml 2>/dev/null  # if you initialized git
cd ..
```

Pass: clean state exits 0; out-of-date state exits non-zero with diff.

### 7. Manifest + user `[[cache]]` mounts *(apptainer; or docker to verify the no-op path)*

```sh
mkdir t-cache && cd t-cache
bv add colabfold --backend apptainer    # GPU not needed for the mount check
ls ~/.cache/bv/colabfold/                # cache-colabfold/ should exist (manifest layer)
ls ~/.cache/bv/colabfold/cache/          # apptainer fallback layer
ls ~/.cache/bv/colabfold/root-.cache/    # apptainer fallback layer

# Add a user override:
cat >> bv.toml <<'EOF'
[[cache]]
match = "colabfold"
container_path = "/cache/colabfold"
host_path = "~/.cache/bv-test-override/{tool}"
EOF

# Trigger a run that would inspect mounts (--num-recycle 0 keeps it short):
bv exec bash -c 'colabfold_batch --help | head -1' || true

ls ~/.cache/bv-test-override/colabfold/  # the override directory was created
cd ..
```

Pass: all four directories exist; the override path is created on first invocation. (The `bv exec` will fail with "no such input" but mount setup happens first.)

### 8. `bv search` + `bv show`

```sh
bv search blast                    # finds the tool in the index
bv show colabfold                  # human-readable
bv show colabfold --format json | jq .tool.id    # "colabfold"
bv show colabfold --format mcp | jq .name        # "colabfold"
```

Pass: search returns matches; all three formats parse.

### 9. Backend selection

```sh
mkdir t-backend && cd t-backend
bv add blast --backend docker
grep 'backend = "docker"' bv.toml || true   # not necessarily set; just check no crash
BV_BACKEND=apptainer bv run blastn -version || echo "expected on docker-only host"
cd ..
```

Pass: Docker run works; switching backend prints a clean error if the other isn't installed.

### 10. `bv conformance` *(network, docker; ~1 min)*

```sh
bv conformance blast
bv conformance hmmer --backend apptainer || true   # if apptainer present
```

Pass: prints "ok" per binary plus per-output type-check; exit 0 on success.

### 11. `bv data fetch` *(network, big — only if you have the bandwidth)*

```sh
bv data list                       # empty initially
# Use a tiny dataset, e.g. one of the BLAST nt subsets if registered.
# If no small dataset exists yet, skip this test for v1.
```

Pass: download completes; `bv data list` shows the cached entry; subsequent `bv add` of a tool that needs it doesn't re-download.

### 12. `bv publish` — dry run *(no network)*

Build a tiny test tool to publish:

```sh
mkdir -p /tmp/bv-tests/hello-bio && cd /tmp/bv-tests/hello-bio
cat > Dockerfile <<'EOF'
FROM python:3.11-slim
RUN echo '#!/usr/bin/env python3\nimport sys; print("hello,", sys.argv[1] if len(sys.argv)>1 else "world")' > /usr/local/bin/hellobio && chmod +x /usr/local/bin/hellobio
WORKDIR /workspace
EOF
cat > bv-publish.toml <<'EOF'
name = "hello-bio"
version = "0.1.0"
description = "A tiny test tool"
homepage = "https://example.com"
license = "MIT"
needs_gpu = false
cpu_cores = 1
ram_gb = 0.5
disk_gb = 0.1
entrypoint_command = "hellobio"
EOF

bv publish . --no-push --no-pr --non-interactive
```

Pass: prints a draft manifest with `image.reference = ghcr.io/<your-github-username>/hello-bio:0.1.0` (or `<your-github-username>` placeholder if no token) and exits 0 without touching the network.

### 13. `bv publish` — real push to your own namespace *(network, docker, GitHub PAT)*

```sh
cd /tmp/bv-tests/hello-bio
# You need a token with `repo` and `write:packages`:
#   https://github.com/settings/tokens/new?scopes=repo,write:packages&description=bv-publish
export GITHUB_TOKEN=ghp_...

bv publish . --no-pr             # builds, pushes to ghcr.io/<you>/hello-bio:0.1.0
docker pull ghcr.io/<you>/hello-bio:0.1.0    # confirm it landed
```

Pass: image visible at `https://github.com/<you>?tab=packages`; manifest digest printed.

### 14. `bv publish` — `--push-to` override *(network, docker)*

```sh
bv publish . --no-pr --push-to my-lab-ghcr-org
# only succeeds if you have write:packages on `my-lab-ghcr-org`; otherwise expect 403.
```

Pass: succeeds with org access; clean error message on 403.

### 15. `bv publish` — github source *(network)*

Pick any small public repo with a Dockerfile (or one with a `requirements.txt` to exercise the auto-Dockerfile path):

```sh
bv publish github:psf/requests@v2.31.0 --no-push --no-pr --non-interactive \
    --tool-name requests --tool-version 2.31.0
```

Pass: clones the repo, detects `pyproject.toml`, prints a draft manifest. (Real publishing of upstream tools you don't own should not actually go to PR; `--no-pr` keeps you safe.)

### 16. `bv-ingest run --dry-run` *(network: bioconda + quay)*

```sh
cd /tmp/bv-tests
bv-ingest run --dry-run --limit 5 --tool samtools
```

Pass: prints what it would do (recipe parsed, image resolved, manifest drafted), opens no PRs.

### 17. `bv-ingest review` + `promote` *(needs an existing staging clone)*

```sh
git clone https://github.com/mlberkeley/bv-registry-staging /tmp/bv-tests/staging
bv-ingest review --staging-dir /tmp/bv-tests/staging
bv-ingest review --staging-dir /tmp/bv-tests/staging --show samtools/1.20

# Edit /tmp/bv-tests/staging/tools/samtools/1.20.toml to add typed I/O,
# commit, push the branch.
bv-ingest promote samtools 1.20 --staging-dir /tmp/bv-tests/staging
```

Pass: review lists drafts missing typed I/O; promote opens a PR against `bv-registry`.

### 18. Cache mount precedence — unit test (already in repo)

```sh
cargo test -p biov mounts
```

Pass: 9 tests green. Specifically `user_overrides_manifest_host_path` confirms layer precedence.

### 19. Smoke test the full demo end-to-end *(GPU + apptainer)*

The protein-fold demo from `docs/demo.md`. If this passes you've covered: registry, lockfile, runtime-select, apptainer pull, SIF caching, three-layer mounts, GPU passthrough, `bv exec`, manifest `cache_paths`.

```sh
mkdir /tmp/bv-tests/demo && cd /tmp/bv-tests/demo
bv add colabfold
cat > fold.py <<'EOF'
import subprocess, json, shutil
from pathlib import Path
Path("trpcage.fasta").write_text(">trp-cage\nNLYIQWLKDGGPSSGRPPPS\n")
Path("output").mkdir(exist_ok=True)
cmd = ["colabfold_batch"] if shutil.which("colabfold_batch") else ["bv", "run", "colabfold_batch"]
subprocess.run(cmd + ["--num-recycle", "3", "/workspace/trpcage.fasta", "/workspace/output"], check=True)
scores = sorted(Path("output").glob("*scores*.json"))[0]
plddt = json.loads(scores.read_text())["plddt"]
print(f"mean pLDDT: {sum(plddt)/len(plddt):.1f}")
EOF
bv exec python3 fold.py
```

Pass: prints `mean pLDDT: 9X.X` after weights download (~3.5GB) and the fold completes.

---

## Part 3: pre-release checklist

- [ ] Run tests 1–6 on a Linux host with Docker.
- [ ] Run tests 1–6 on macOS with Docker Desktop.
- [ ] Run tests 7, 19 on a Linux GPU host with apptainer.
- [ ] Run tests 12–14 with a real GitHub PAT and verify the package shows up under your account.
- [ ] Tag a release: `git tag v0.2.0 && git push --tags`.
- [ ] Publish `biov` to crates.io: `cargo publish -p bv-types -p bv-core -p bv-runtime -p bv-runtime-apptainer -p bv-index -p bv-conformance -p biov` (in dependency order).
- [ ] Update install.sh release URL pointer.
- [ ] Cut a discussion / issue tracking the v2 list above.
