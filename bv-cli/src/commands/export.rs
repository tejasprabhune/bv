use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, bail};
use owo_colors::{OwoColorize, Stream};

use bv_core::lockfile::Lockfile;
use bv_core::project::BvToml;

/// Entry point for `bv export`.
///
/// Today we only know how to emit a conda env YAML. Anything else is a hard
/// error so we don't silently produce a useless file when we add more formats.
pub fn run(format: &str, output: Option<&Path>) -> anyhow::Result<()> {
    if format != "conda" {
        bail!("only --format conda is supported today; pixi/dockerfile coming soon");
    }

    let cwd = std::env::current_dir()?;
    let bv_lock_path = cwd.join("bv.lock");
    let bv_toml_path = cwd.join("bv.toml");

    if !bv_lock_path.exists() {
        bail!("no bv.lock found in current directory; run `bv lock` first");
    }

    let lockfile = Lockfile::from_toml_str(
        &fs::read_to_string(&bv_lock_path).context("failed to read bv.lock")?,
    )
    .context("failed to parse bv.lock")?;

    let project_name = if bv_toml_path.exists() {
        BvToml::from_path(&bv_toml_path)
            .ok()
            .map(|t| t.project.name)
            .unwrap_or_else(|| "bv-export".to_string())
    } else {
        "bv-export".to_string()
    };

    let (yaml, exported, skipped) = render_conda_yaml(&project_name, &lockfile);

    if let Some(path) = output {
        let mut f = fs::File::create(path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        f.write_all(yaml.as_bytes())?;
    } else {
        print!("{}", yaml);
    }

    eprintln!(
        "  {} {} tools as bioconda packages",
        "Exported".if_supports_color(Stream::Stderr, |t| t.green().bold().to_string()),
        exported.len(),
    );
    if !skipped.is_empty() {
        let ids: Vec<&str> = skipped.iter().map(|(id, _)| id.as_str()).collect();
        eprintln!(
            "  {} {} tools without conda equivalents: {} (see comment block in output)",
            "Skipped".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string()),
            skipped.len(),
            ids.join(", "),
        );
    }

    Ok(())
}

/// Parse a biocontainers-style image reference into `(name, version)`.
///
/// Recognized form: `quay.io/biocontainers/<name>:<version>(--<build_hash>)?`.
/// The build hash suffix is dropped because conda specs don't carry it.
/// Returns `None` for any reference that doesn't fit the pattern.
fn parse_biocontainers_ref(image_ref: &str) -> Option<(String, String)> {
    let rest = image_ref.strip_prefix("quay.io/biocontainers/")?;
    let (name, ver_and_hash) = rest.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    let version = match ver_and_hash.split_once("--") {
        Some((v, _hash)) => v,
        None => ver_and_hash,
    };
    if version.is_empty() {
        return None;
    }
    Some((name.to_string(), version.to_string()))
}

/// Build the conda env YAML plus accounting (which tools were exported, which
/// were skipped). Returned as a tuple so the caller can write the YAML and
/// print the stderr summary in one pass.
fn render_conda_yaml(
    project_name: &str,
    lockfile: &Lockfile,
) -> (String, Vec<(String, String)>, Vec<(String, String)>) {
    let mut exported: Vec<(String, String)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();

    let mut tools: Vec<_> = lockfile.tools.values().collect();
    tools.sort_by(|a, b| a.tool_id.cmp(&b.tool_id));

    for entry in &tools {
        match parse_biocontainers_ref(&entry.image_reference) {
            Some((name, version)) => {
                exported.push((name, version));
            }
            None => {
                skipped.push((entry.tool_id.clone(), entry.image_reference.clone()));
            }
        }
    }

    let mut out = String::new();
    out.push_str(&format!("name: {}\n", project_name));
    out.push_str("channels:\n");
    out.push_str("  - conda-forge\n");
    out.push_str("  - bioconda\n");
    out.push_str("dependencies:\n");
    if exported.is_empty() {
        // Emit no dependency lines; conda accepts an empty list under the key.
    } else {
        for (name, version) in &exported {
            out.push_str(&format!("  - bioconda::{}={}\n", name, version));
        }
    }

    if !skipped.is_empty() {
        out.push('\n');
        out.push_str(
            "# Tools that have no known conda/bioconda equivalent. These come from\n",
        );
        out.push_str(
            "# custom OCI images and would need to be installed by hand:\n",
        );
        for (id, image) in &skipped {
            out.push_str(&format!("#   - {} (image: {})\n", id, image));
        }
    }

    (out, exported, skipped)
}

// Helper used by tests so we don't have to plumb a temp dir through `run`.
#[cfg(test)]
fn render_for_test(project_name: &str, lock_toml: &str) -> (String, usize, Vec<String>) {
    let lock = Lockfile::from_toml_str(lock_toml).expect("parse lock");
    let (yaml, exported, skipped) = render_conda_yaml(project_name, &lock);
    let skipped_ids = skipped.into_iter().map(|(id, _)| id).collect();
    (yaml, exported.len(), skipped_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_biocontainers_ref() {
        let got = parse_biocontainers_ref("quay.io/biocontainers/blast:2.15.0--xyz123");
        assert_eq!(got, Some(("blast".into(), "2.15.0".into())));
    }

    #[test]
    fn parses_biocontainers_no_hash() {
        let got = parse_biocontainers_ref("quay.io/biocontainers/blast:2.15.0");
        assert_eq!(got, Some(("blast".into(), "2.15.0".into())));
    }

    #[test]
    fn non_biocontainers_returns_none() {
        assert_eq!(parse_biocontainers_ref("ncbi/blast:2.15.0"), None);
        assert_eq!(
            parse_biocontainers_ref("ghcr.io/tejasprabhune/genie2:1.0.0"),
            None
        );
        assert_eq!(parse_biocontainers_ref("rosettacommons/proteinmpnn:0.1"), None);
    }

    #[test]
    fn emit_yaml_round_trips() {
        // Hand-rolled minimal lockfile with one bioconda tool and one custom
        // OCI image. The `resolved_at` field is required; any RFC3339 ts works.
        let lock_toml = r#"
version = 1

[metadata]
bv_version = "0.1.16"
generated_at = "2026-04-29T00:00:00Z"

[tools.blast]
tool_id = "blast"
version = "2.15.0"
image_reference = "quay.io/biocontainers/blast:2.15.0--pl5321h6f7f691_0"
image_digest = "sha256:aaa"
resolved_at = "2026-04-29T00:00:00Z"

[tools.hmmer]
tool_id = "hmmer"
version = "3.4"
image_reference = "quay.io/biocontainers/hmmer:3.4--hdbdd923_2"
image_digest = "sha256:bbb"
resolved_at = "2026-04-29T00:00:00Z"

[tools.genie2]
tool_id = "genie2"
version = "1.0.0"
image_reference = "ghcr.io/tejasprabhune/genie2:1.0.0"
image_digest = "sha256:ccc"
resolved_at = "2026-04-29T00:00:00Z"
"#;

        let (yaml, exported, skipped_ids) = render_for_test("my-proj", lock_toml);

        // Header.
        assert!(yaml.starts_with("name: my-proj\n"));
        assert!(yaml.contains("  - conda-forge\n"));
        assert!(yaml.contains("  - bioconda\n"));

        // Bioconda specs are emitted (sorted by tool_id, so blast then hmmer).
        assert!(yaml.contains("  - bioconda::blast=2.15.0\n"));
        assert!(yaml.contains("  - bioconda::hmmer=3.4\n"));

        // Custom image lands in the comment block, NOT as a dep.
        assert!(!yaml.contains("- bioconda::genie2"));
        assert!(yaml.contains("# Tools that have no known conda/bioconda equivalent"));
        assert!(yaml.contains("#   - genie2 (image: ghcr.io/tejasprabhune/genie2:1.0.0)"));

        assert_eq!(exported, 2);
        assert_eq!(skipped_ids, vec!["genie2".to_string()]);
    }
}
