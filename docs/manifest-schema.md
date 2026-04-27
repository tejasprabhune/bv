# bv Manifest JSON Schema (v1.0)

`bv show <tool> --json` produces a stable, versioned JSON object. This document describes that schema.

## Output format

```json
{
  "schema_version": "1.0",
  "tool": {
    "id": "blast",
    "version": "2.15.0",
    "description": "BLAST+ Basic Local Alignment Search Tool",
    "homepage": "https://blast.ncbi.nlm.nih.gov/Blast.cgi",
    "license": "Public Domain",
    "image": {
      "backend": "docker",
      "reference": "ncbi/blast:2.15.0"
    },
    "inputs": [
      {
        "name": "query",
        "type": "fasta",
        "cardinality": "one",
        "description": "Query sequences in FASTA format"
      },
      {
        "name": "db",
        "type": "blast_db",
        "cardinality": "one",
        "description": "BLAST database directory"
      }
    ],
    "outputs": [
      {
        "name": "output",
        "type": "blast_tab",
        "cardinality": "one",
        "description": "Tabular BLAST alignment results (outfmt 6)"
      }
    ]
  }
}
```

## Field reference

### Top level

| Field | Type | Always present | Description |
|---|---|---|---|
| `schema_version` | string | yes | `"1.0"`; bump when shape changes |
| `tool` | object | yes | Tool descriptor |

### `tool`

| Field | Type | Always present |
|---|---|---|
| `id` | string | yes |
| `version` | string | yes |
| `description` | string | no |
| `homepage` | string | no |
| `license` | string | no |
| `image` | object | yes |
| `inputs` | array | yes (empty if untyped) |
| `outputs` | array | yes (empty if untyped) |

### I/O entry (inputs and outputs use the same shape)

| Field | Type | Always present | Description |
|---|---|---|---|
| `name` | string | yes | Port identifier |
| `type` | string | yes | Type from bv-types vocabulary; may include params e.g. `"fasta[protein]"` |
| `cardinality` | string | yes | `"one"`, `"many"`, or `"optional"` |
| `description` | string | no | Human-readable description |
| `mount` | string | no | Absolute container path |

## Other formats

```sh
bv show blast --format mcp         # MCP tool descriptor (name + inputSchema)
bv show blast --format json-schema # JSON Schema for inputs (draft-07)
```

### MCP tool descriptor

```json
{
  "name": "blast",
  "description": "BLAST+ Basic Local Alignment Search Tool",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "description": "Query sequences in FASTA format (fasta)" },
      "db":    { "type": "string", "description": "BLAST database directory (blast_db)" }
    },
    "required": ["query", "db"]
  }
}
```

### JSON Schema for inputs

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "blast inputs",
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "description": "Query sequences in FASTA format",
      "x-bv-type": "fasta"
    },
    "db": {
      "type": "string",
      "description": "BLAST database directory",
      "x-bv-type": "blast_db"
    }
  },
  "required": ["query", "db"]
}
```

## Versioning policy

`schema_version` is a string, not a semver. It will increment (`"1.0"` -> `"2.0"`) only when the shape is not backward compatible. Adding optional fields to `tool` or I/O entries is not a breaking change and does not bump the version.

Snapshot tests in `bv-cli/src/commands/show.rs` lock the exact output; any shape change is visible in PR diffs.
