# Using bv in scripts and pipelines

## How it works

`bv exec` and `bv shell` both work by prepending `<project>/.bv/bin/` to `PATH`.
That directory contains one shim per exposed binary. Each shim is a tiny shell script:

```sh
#!/bin/sh
exec bv run "$(basename "$0")" "$@"
```

The shim looks up the binary name in `bv.lock`'s `[binary_index]`, routes to the
right container image, and passes all arguments through.

Shims are regenerated automatically on `bv add`, `bv remove`, `bv sync`, and `bv lock`.
The `.bv/` directory is listed in `.gitignore` automatically when first created.

---

## bv exec

Run any command with bv-managed binaries on PATH. Useful in CI and scripts.

```sh
bv exec blastn -query query.fa -db nr -out hits.tsv -outfmt 6
bv exec python fold.py
bv exec bash analysis.sh
bv exec -- bash -c "blastn -query foo.fa | sort -k11 -n"
```

On Unix, `bv exec` uses `exec(2)` to replace itself with the child process.
Signals, exit codes, and process tree all behave as if you called the binary
directly — no wrapper layer in `ps`.

### CI usage

```yaml
- run: bv sync --frozen
- run: bv exec snakemake --cores 4
```

### Makefile

```make
results.tsv: query.fa db.phr
	bv exec blastn -query $< -db db -out $@ -outfmt 6

aligned.bam: reads.fastq.gz
	bv exec bwa mem -t 8 ref.fa $< | bv exec samtools sort -o $@
```

### Snakemake

```python
rule align:
    input:  "reads.fastq.gz"
    output: "aligned.bam"
    shell:
        "bv exec bwa mem -t {threads} ref.fa {input} "
        "| bv exec samtools sort -o {output}"
```

### Nextflow

```nextflow
process ALIGN {
    input:  path reads
    output: path "aligned.bam"
    script:
    """
    bv exec bwa mem -t $task.cpus ref.fa $reads | bv exec samtools sort -o aligned.bam
    """
}
```

---

## bv shell

Start an interactive subshell with bv-managed binaries on PATH.

```sh
bv shell                  # uses $SHELL
bv shell --shell zsh      # explicit shell
```

The prompt changes to `(bv:<project-name>)` so you always know the environment
is active. Exiting the subshell (`exit` or Ctrl-D) returns cleanly to your
original environment — no deactivation step needed.

```
$ cd my-project
$ bv shell
(bv:my-project) $ blastn -query query.fa -db nr -out hits.tsv
(bv:my-project) $ hmmscan --tblout pfam.txt Pfam-A.hmm seqs.fa
(bv:my-project) $ exit
$
```

The environment variable `BV_ACTIVE` is set to the project name while inside
the shell. Scripts can check `$BV_ACTIVE` to detect whether they are running
in a bv-activated context.

---

## bv run (binary routing)

`bv run` now accepts binary names directly, without naming the tool:

```sh
bv run blastn -query foo.fa       # resolves blastn -> blast tool
bv run makeblastdb -in seqs.fa    # same blast image
bv run blast -- blastn -query foo  # original form still works
```

---

## bv list --binaries

Show the full binary routing table for the current project:

```sh
bv list --binaries
```

```
  Binary          Tool
  ---------------------------------
  blastn          blast 2.15.0
  blastp          blast 2.15.0
  makeblastdb     blast 2.15.0
  hmmscan         hmmer 3.3.2
  hmmsearch       hmmer 3.3.2
  mmseqs          mmseqs2 17.0.0
  samtools        samtools 1.21
```

---

## Collision resolution

If two tools expose the same binary name, `bv add` and `bv lock` will fail
with a clear error. Resolve it in `bv.toml`:

```toml
[binary_overrides]
samtools = "samtools"       # "samtools" tool wins over any other that exposes it
python   = "colabfold"      # colabfold's python shim wins
```

After editing, run `bv lock` to rebuild the binary index and regenerate shims.
