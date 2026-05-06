# bv Type Vocabulary

The types below are the recognized identifiers for `[[tool.inputs]]` and `[[tool.outputs]]` in a manifest.

---

## Types

| Type | Kind | Parent | Parameters | Description |
|---|---|---|---|---|
| `file` | file | | | Any file |
| `dir` | directory | | | Any directory |
| `tabular` | file | `file` | | Tab-separated tabular data |
| `fasta` | file | `file` | `alphabet` | FASTA sequence file |
| `fastq` | file | `file` | | FASTQ sequence file with per-base quality scores |
| `sam` | file | `tabular` | | Sequence Alignment/Map format |
| `bam` | file | `file` | | Binary Alignment Map |
| `msa` | file | `file` | `format` | Multiple sequence alignment |
| `pdb` | file | `file` | | Protein Data Bank structure file |
| `mmcif` | file | `file` | | Macromolecular CIF structure file |
| `vcf` | file | `tabular` | | Variant Call Format |
| `bcf` | file | `file` | | Binary Variant Call Format |
| `blast_db` | directory | | | BLAST database directory (phr/pin/psq files) |
| `hmm_profile` | file | `file` | | HMMER profile HMM file |
| `blast_tab` | file | `tabular` | | BLAST tabular output (outfmt 6) |
| `hmmer_output` | file | `file` | | HMMER search output (text report) |
| `mmseqs_db` | directory | | | MMseqs2 database directory |
| `mmseqs_output` | file | `file` | | MMseqs2 search results |

`kind` is `file` or `directory`. Types with no `parent` are roots.

---

## Parameters

Some types accept a parameter in brackets:

```toml
type = "fasta[protein]"
type = "msa[stockholm]"
```

| Type | Parameter | Values |
|---|---|---|
| `fasta` | `alphabet` | `protein`, `dna`, `rna` |
| `msa` | `format` | `stockholm`, `clustal`, `fasta`, etc. |

Parameters are passed through to workflow integrations; bv does not validate the value.

---

## Usage in a manifest

```toml
[[tool.inputs]]
name = "query"
type = "fasta[protein]"
cardinality = "one"
description = "Input sequences"

[[tool.outputs]]
name = "hits"
type = "blast_tab"
cardinality = "one"
description = "Tabular alignment results"
```

Cardinality is `one`, `many`, or `optional`.

---

## Adding a type

Open a PR against [bv-registry](https://github.com/tejasprabhune/bv-registry) that adds an entry to `bv-types/types.toml`. Every type needs a `description`; roots need `kind`; non-roots need `parent`.
