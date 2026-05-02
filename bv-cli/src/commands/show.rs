use std::path::PathBuf;

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};
use serde::Serialize;

use bv_core::cache::CacheLayout;
use bv_core::manifest::{IoSpec, Manifest, ToolManifest};
use bv_core::project::BvLock;

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ShowFormat {
    Json,
    Mcp,
    JsonSchema,
}

pub fn run(tool: &str, format: Option<ShowFormat>) -> anyhow::Result<()> {
    let manifest = load_manifest(tool)?;

    match format {
        None => print_human(&manifest.tool),
        Some(ShowFormat::Json) => print_json(&manifest)?,
        Some(ShowFormat::Mcp) => print_mcp(&manifest.tool)?,
        Some(ShowFormat::JsonSchema) => print_json_schema(&manifest.tool)?,
    }

    Ok(())
}

fn load_manifest(tool: &str) -> anyhow::Result<Manifest> {
    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");

    // Try bv.lock + cache path first.
    if bv_lock_path.exists() {
        let lockfile = BvLock::from_path(&bv_lock_path).context("failed to read bv.lock")?;
        if let Some(entry) = lockfile.tools.get(tool) {
            let cache = CacheLayout::new();
            let manifest_path = cache.manifest_path(tool, &entry.version);
            if manifest_path.exists() {
                let s = std::fs::read_to_string(&manifest_path)?;
                return Manifest::from_toml_str(&s)
                    .with_context(|| format!("failed to parse manifest for '{tool}'"));
            }
        }
    }

    // Fall back to the local index clone.
    let cache = CacheLayout::new();
    let index_dir = cache.index_dir("default");
    let tool_dir = index_dir.join("tools").join(tool);
    if tool_dir.exists() {
        let best = find_latest_version_in_dir(&tool_dir)?;
        let s = std::fs::read_to_string(&best)
            .with_context(|| format!("failed to read {}", best.display()))?;
        return Manifest::from_toml_str(&s)
            .with_context(|| format!("failed to parse manifest for '{tool}'"));
    }

    anyhow::bail!(
        "Tool '{tool}' is not installed and not found in the local index.\n\
         Run `bv add {tool}` first."
    )
}

fn find_latest_version_in_dir(dir: &PathBuf) -> anyhow::Result<PathBuf> {
    // Sort by parsed semver::Version so 2.10.1 beats 2.9.0 (lexicographic
    // sort places "2.10" before "2.9" because of the leading character).
    // Files whose stems don't parse as semver are skipped silently.
    let mut versioned: Vec<(semver::Version, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem().and_then(|s| s.to_str())?;
            let v = semver::Version::parse(stem).ok()?;
            Some((v, path))
        })
        .collect();
    versioned.sort_by(|a, b| a.0.cmp(&b.0));
    versioned
        .pop()
        .map(|(_, p)| p)
        .ok_or_else(|| anyhow::anyhow!("no manifest files found in {}", dir.display()))
}

fn print_human(tool: &ToolManifest) {
    println!("Tool:      {}", tool.id);
    println!("Version:   {}", tool.version);
    if let Some(desc) = &tool.description {
        println!("About:     {desc}");
    }
    if let Some(hp) = &tool.homepage {
        println!("Homepage:  {hp}");
    }
    println!("Image:     {}", tool.image.reference);

    if tool.has_typed_io() {
        println!();
        if !tool.inputs.is_empty() {
            println!("Inputs:");
            for spec in &tool.inputs {
                print_io_line("  ", spec);
            }
        }
        if !tool.outputs.is_empty() {
            println!("Outputs:");
            for spec in &tool.outputs {
                print_io_line("  ", spec);
            }
        }
    } else {
        eprintln!(
            "  {} no typed I/O declared; experimental tools aren't validated against workflow integrations",
            "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string())
        );
    }

    if let Some(ep) = &tool.entrypoint {
        println!();
        println!("Entrypoint: {}", ep.command);
        if let Some(tmpl) = &ep.args_template {
            println!("Args template: {tmpl}");
        }
    }

    if !tool.subcommands.is_empty() {
        println!();
        println!("Subcommands:");
        let mut entries: Vec<(&String, &Vec<String>)> = tool.subcommands.iter().collect();
        entries.sort_by_key(|(k, _)| k.as_str());
        let max_name = entries.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (name, cmd) in entries {
            println!("  {:width$}  {}", name, cmd.join(" "), width = max_name);
        }
    }
}

fn print_io_line(indent: &str, spec: &IoSpec) {
    let card = spec.cardinality.to_string();
    let type_str = spec.r#type.to_string();
    let desc = spec.description.as_deref().unwrap_or("");
    println!(
        "{indent}{name}  [{type_str}] ({card}){sep}{desc}",
        name = spec.name,
        sep = if desc.is_empty() { "" } else { "  " },
    );
}

/// Stable JSON output. This is a public API surface; version it here.
#[derive(Serialize)]
struct JsonOutput<'a> {
    schema_version: &'static str,
    tool: JsonTool<'a>,
}

#[derive(Serialize)]
struct JsonTool<'a> {
    id: &'a str,
    version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    homepage: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<&'a str>,
    image: JsonImage<'a>,
    inputs: Vec<JsonIo<'a>>,
    outputs: Vec<JsonIo<'a>>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    subcommands: std::collections::BTreeMap<&'a str, &'a Vec<String>>,
}

#[derive(Serialize)]
struct JsonImage<'a> {
    backend: &'a str,
    reference: &'a str,
}

#[derive(Serialize)]
struct JsonIo<'a> {
    name: &'a str,
    r#type: String,
    cardinality: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mount: Option<String>,
}

fn to_json_io(spec: &IoSpec) -> JsonIo<'_> {
    JsonIo {
        name: &spec.name,
        r#type: spec.r#type.to_string(),
        cardinality: spec.cardinality.to_string(),
        description: spec.description.as_deref(),
        mount: spec
            .mount
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    }
}

fn print_json(manifest: &Manifest) -> anyhow::Result<()> {
    let t = &manifest.tool;
    let out = JsonOutput {
        schema_version: "1.0",
        tool: JsonTool {
            id: &t.id,
            version: &t.version,
            description: t.description.as_deref(),
            homepage: t.homepage.as_deref(),
            license: t.license.as_deref(),
            image: JsonImage {
                backend: &t.image.backend,
                reference: &t.image.reference,
            },
            inputs: t.inputs.iter().map(to_json_io).collect(),
            outputs: t.outputs.iter().map(to_json_io).collect(),
            subcommands: t.subcommands.iter().map(|(k, v)| (k.as_str(), v)).collect(),
        },
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// MCP tool descriptor format.
#[derive(Serialize)]
struct McpTool<'a> {
    name: &'a str,
    description: &'a str,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

fn print_mcp(tool: &ToolManifest) -> anyhow::Result<()> {
    let desc = tool.description.as_deref().unwrap_or("");
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for spec in &tool.inputs {
        let mut prop = serde_json::Map::new();
        prop.insert("type".into(), "string".into());
        let type_str = spec.r#type.to_string();
        let desc_text = match spec.description.as_deref() {
            Some(d) => format!("{d} ({type_str})"),
            None => type_str,
        };
        prop.insert("description".into(), desc_text.into());
        properties.insert(spec.name.clone(), prop.into());

        if matches!(
            spec.cardinality,
            bv_types::Cardinality::One | bv_types::Cardinality::Many
        ) {
            required.push(serde_json::Value::String(spec.name.clone()));
        }
    }

    let input_schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });

    let mcp = McpTool {
        name: &tool.id,
        description: desc,
        input_schema,
    };
    println!("{}", serde_json::to_string_pretty(&mcp)?);
    Ok(())
}

fn print_json_schema(tool: &ToolManifest) -> anyhow::Result<()> {
    let title = format!("{} inputs", tool.id);
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for spec in &tool.inputs {
        let mut prop = serde_json::Map::new();
        prop.insert("type".into(), "string".into());
        if let Some(d) = &spec.description {
            prop.insert("description".into(), d.clone().into());
        }
        prop.insert("x-bv-type".into(), spec.r#type.to_string().into());
        properties.insert(spec.name.clone(), prop.into());

        if matches!(
            spec.cardinality,
            bv_types::Cardinality::One | bv_types::Cardinality::Many
        ) {
            required.push(serde_json::Value::String(spec.name.clone()));
        }
    }

    let schema = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": title,
        "type": "object",
        "properties": properties,
        "required": required,
    });
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bv_core::manifest::Manifest;

    const BLAST_MANIFEST: &str = r#"
[tool]
id = "blast"
version = "2.15.0"
description = "BLAST+ Basic Local Alignment Search Tool"
homepage = "https://blast.ncbi.nlm.nih.gov/Blast.cgi"
license = "Public Domain"

[tool.image]
backend = "docker"
reference = "ncbi/blast:2.15.0"

[tool.hardware]
cpu_cores = 4
ram_gb = 8.0

[[tool.inputs]]
name = "query"
type = "fasta"
cardinality = "one"
description = "Query sequences in FASTA format"

[[tool.inputs]]
name = "db"
type = "blast_db"
cardinality = "one"
description = "BLAST database directory"

[[tool.outputs]]
name = "output"
type = "blast_tab"
cardinality = "one"
description = "Tabular BLAST results"

[tool.entrypoint]
command = "blastn"
args_template = "-query {query} -db {db} -out {output} -num_threads {cpu_cores}"
"#;

    #[test]
    fn find_latest_version_in_dir_uses_semver_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        for v in ["2.9.0", "2.10.0", "2.10.1"] {
            std::fs::write(tmp.path().join(format!("{v}.toml")), "").unwrap();
        }
        let path = super::find_latest_version_in_dir(&tmp.path().to_path_buf()).unwrap();
        assert_eq!(path.file_stem().unwrap().to_str().unwrap(), "2.10.1");
    }

    #[test]
    fn json_output_snapshot() {
        let manifest = Manifest::from_toml_str(BLAST_MANIFEST).unwrap();
        let t = &manifest.tool;
        let out = JsonOutput {
            schema_version: "1.0",
            tool: JsonTool {
                id: &t.id,
                version: &t.version,
                description: t.description.as_deref(),
                homepage: t.homepage.as_deref(),
                license: t.license.as_deref(),
                image: JsonImage {
                    backend: &t.image.backend,
                    reference: &t.image.reference,
                },
                inputs: t.inputs.iter().map(to_json_io).collect(),
                outputs: t.outputs.iter().map(to_json_io).collect(),
                subcommands: t.subcommands.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            },
        };
        let json = serde_json::to_string_pretty(&out).unwrap();
        insta::assert_snapshot!(json);
    }
}
