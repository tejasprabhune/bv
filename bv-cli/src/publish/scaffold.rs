use std::path::{Path, PathBuf};

use anyhow::Context;
use bv_core::manifest::{
    EntrypointSpec, GpuSpec, HardwareSpec, ImageSpec, IoSpec, Manifest, Tier, ToolManifest,
};
use bv_types::Cardinality;
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use owo_colors::{OwoColorize, Stream};

use super::source::FetchedSource;

/// Contents of an optional `bv-publish.toml` at the repo root.
#[derive(serde::Deserialize, Default)]
pub struct PublishConfig {
    #[serde(default)]
    pub publish: PublishMeta,
}

#[derive(serde::Deserialize, Default)]
pub struct PublishMeta {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    #[serde(default)]
    pub hardware: HardwareMeta,
    #[serde(default)]
    pub inputs: Vec<IoMeta>,
    #[serde(default)]
    pub outputs: Vec<IoMeta>,
    #[serde(default)]
    pub entrypoint: EntrypointMeta,
}

#[derive(serde::Deserialize, Default)]
pub struct HardwareMeta {
    pub cpu_cores: Option<u32>,
    pub ram_gb: Option<f64>,
    pub disk_gb: Option<f64>,
    pub needs_gpu: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct IoMeta {
    pub name: String,
    #[serde(rename = "type")]
    pub type_str: String,
    #[serde(default)]
    pub cardinality: String,
    pub mount: Option<String>,
    pub description: Option<String>,
}

#[derive(serde::Deserialize, Default)]
pub struct EntrypointMeta {
    pub command: Option<String>,
    pub args_template: Option<String>,
}

/// Final result from scaffolding, ready to be turned into a manifest.
pub struct ScaffoldResult {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub cpu_cores: u32,
    pub ram_gb: f64,
    pub disk_gb: f64,
    pub needs_gpu: bool,
    pub inputs: Vec<IoSpec>,
    pub outputs: Vec<IoSpec>,
    pub entrypoint_command: String,
    pub args_template: Option<String>,
}

impl ScaffoldResult {
    pub fn to_manifest_toml(&self, image_ref: &str, digest: &str) -> anyhow::Result<String> {
        let m = Manifest {
            tool: ToolManifest {
                id: self.name.clone(),
                version: self.version.clone(),
                description: self.description.clone(),
                homepage: self.homepage.clone(),
                license: self.license.clone(),
                tier: Tier::Community,
                maintainers: vec![],
                deprecated: false,
                image: ImageSpec {
                    backend: "docker".to_string(),
                    reference: image_ref.to_string(),
                    digest: if digest.is_empty() {
                        None
                    } else {
                        Some(digest.to_string())
                    },
                },
                hardware: HardwareSpec {
                    gpu: self.needs_gpu.then_some(GpuSpec {
                        required: true,
                        min_vram_gb: None,
                        cuda_version: None,
                    }),
                    cpu_cores: Some(self.cpu_cores),
                    ram_gb: Some(self.ram_gb),
                    disk_gb: Some(self.disk_gb),
                },
                reference_data: Default::default(),
                inputs: self.inputs.clone(),
                outputs: self.outputs.clone(),
                entrypoint: EntrypointSpec {
                    command: self.entrypoint_command.clone(),
                    args_template: self.args_template.clone(),
                    env: Default::default(),
                },
                cache_paths: vec![],
                binaries: None,
                test: None,
                signatures: None,
            },
        };
        m.to_toml_string().map_err(|e| anyhow::anyhow!("{}", e))
    }
}

/// Try to load `bv-publish.toml` from the given directory.
pub fn load_publish_config(dir: &Path) -> Option<PublishConfig> {
    let path = dir.join("bv-publish.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<PublishConfig>(&content) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            eprintln!(
                "  {} bv-publish.toml parse error: {}",
                "warning:".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string()),
                e
            );
            None
        }
    }
}

/// Fully non-interactive path: requires bv-publish.toml or explicit overrides.
pub fn from_config(
    config: Option<&PublishConfig>,
    fetched: &FetchedSource,
    name_override: Option<&str>,
    version_override: Option<&str>,
) -> anyhow::Result<ScaffoldResult> {
    let meta = config.map(|c| &c.publish);

    let name = name_override
        .map(|s| s.to_string())
        .or_else(|| meta.and_then(|m| m.name.clone()))
        .unwrap_or_else(|| fetched.name_hint.clone());

    let version = version_override
        .map(|s| s.to_string())
        .or_else(|| meta.and_then(|m| m.version.clone()))
        .or_else(|| fetched.version_hint.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "version is required in non-interactive mode\n  \
                 Set it in bv-publish.toml or pass --version"
            )
        })?;

    let default_meta = PublishMeta::default();
    let m = meta.unwrap_or(&default_meta);

    let inputs = parse_io_specs(&m.inputs).context("inputs")?;
    let outputs = parse_io_specs(&m.outputs).context("outputs")?;

    Ok(ScaffoldResult {
        name,
        version,
        description: m.description.clone(),
        homepage: m.homepage.clone(),
        license: m.license.clone(),
        cpu_cores: m.hardware.cpu_cores.unwrap_or(4),
        ram_gb: m.hardware.ram_gb.unwrap_or(8.0),
        disk_gb: m.hardware.disk_gb.unwrap_or(2.0),
        needs_gpu: m.hardware.needs_gpu.unwrap_or(false),
        inputs,
        outputs,
        entrypoint_command: m.entrypoint.command.clone().unwrap_or_default(),
        args_template: m.entrypoint.args_template.clone(),
    })
}

/// Interactive path: prompts for all fields, pre-filling from config + hints.
pub fn interactive(
    config: Option<&PublishConfig>,
    fetched: &FetchedSource,
    name_override: Option<&str>,
    version_override: Option<&str>,
) -> anyhow::Result<ScaffoldResult> {
    let meta = config.map(|c| &c.publish);
    let theme = ColorfulTheme::default();

    let name_default = name_override
        .map(|s| s.to_string())
        .or_else(|| meta.and_then(|m| m.name.clone()))
        .unwrap_or_else(|| fetched.name_hint.clone());

    let version_default = version_override
        .map(|s| s.to_string())
        .or_else(|| meta.and_then(|m| m.version.clone()))
        .or_else(|| fetched.version_hint.clone())
        .unwrap_or_else(|| "0.1.0".to_string());

    eprintln!();
    let name: String = Input::with_theme(&theme)
        .with_prompt("Tool name")
        .default(name_default)
        .interact_text()?;

    let version: String = Input::with_theme(&theme)
        .with_prompt("Version")
        .default(version_default)
        .interact_text()?;

    let description: String = Input::with_theme(&theme)
        .with_prompt("Description")
        .default(meta.and_then(|m| m.description.clone()).unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;

    let homepage: String = Input::with_theme(&theme)
        .with_prompt("Homepage URL")
        .default(
            meta.and_then(|m| m.homepage.clone())
                .unwrap_or_else(|| fetched.source_url.clone()),
        )
        .allow_empty(true)
        .interact_text()?;

    let license: String = Input::with_theme(&theme)
        .with_prompt("License")
        .default(meta.and_then(|m| m.license.clone()).unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;

    eprintln!(
        "\n  {}",
        "Hardware".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );

    let cpu_cores: u32 = Input::with_theme(&theme)
        .with_prompt("CPU cores")
        .default(meta.and_then(|m| m.hardware.cpu_cores).unwrap_or(4))
        .interact_text()?;

    let ram_gb: f64 = Input::with_theme(&theme)
        .with_prompt("RAM (GB)")
        .default(meta.and_then(|m| m.hardware.ram_gb).unwrap_or(8.0))
        .interact_text()?;

    let disk_gb: f64 = Input::with_theme(&theme)
        .with_prompt("Disk (GB)")
        .default(meta.and_then(|m| m.hardware.disk_gb).unwrap_or(2.0))
        .interact_text()?;

    let needs_gpu = Confirm::with_theme(&theme)
        .with_prompt("Needs GPU?")
        .default(meta.and_then(|m| m.hardware.needs_gpu).unwrap_or(false))
        .interact()?;

    let config_inputs = meta.map(|m| m.inputs.as_slice()).unwrap_or(&[]);
    let config_outputs = meta.map(|m| m.outputs.as_slice()).unwrap_or(&[]);

    eprintln!(
        "\n  {}",
        "Inputs".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
    let inputs = collect_io_specs(&theme, config_inputs, "input")?;

    eprintln!(
        "\n  {}",
        "Outputs".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
    let outputs = collect_io_specs(&theme, config_outputs, "output")?;

    eprintln!(
        "\n  {}",
        "Entrypoint".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );

    let default_cmd = meta
        .and_then(|m| m.entrypoint.command.clone())
        .unwrap_or_else(|| name.clone());
    let entrypoint_command: String = Input::with_theme(&theme)
        .with_prompt("Command")
        .default(default_cmd)
        .interact_text()?;

    let args_template: String = Input::with_theme(&theme)
        .with_prompt("Args template (use {port_name}, {cpu_cores}; leave blank to skip)")
        .default(
            meta.and_then(|m| m.entrypoint.args_template.clone())
                .unwrap_or_default(),
        )
        .allow_empty(true)
        .interact_text()?;

    Ok(ScaffoldResult {
        name,
        version,
        description: non_empty(description),
        homepage: non_empty(homepage),
        license: non_empty(license),
        cpu_cores,
        ram_gb,
        disk_gb,
        needs_gpu,
        inputs,
        outputs,
        entrypoint_command,
        args_template: non_empty(args_template),
    })
}

fn collect_io_specs(
    theme: &ColorfulTheme,
    prefilled: &[IoMeta],
    label: &str,
) -> anyhow::Result<Vec<IoSpec>> {
    let mut specs = parse_io_specs(prefilled)?;

    if !prefilled.is_empty() {
        for s in &specs {
            eprintln!(
                "  {} {} ({})",
                "Pre-filled".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
                s.name,
                s.r#type
            );
        }
    }

    loop {
        let prompt = if specs.is_empty() {
            format!("Add {label}?")
        } else {
            format!("Add another {label}?")
        };

        if !Confirm::with_theme(theme)
            .with_prompt(prompt)
            .default(specs.is_empty())
            .interact()?
        {
            break;
        }

        let name: String = Input::with_theme(theme)
            .with_prompt("Name")
            .interact_text()?;

        let type_str = prompt_type(theme)?;

        let cardinality_idx = Select::with_theme(theme)
            .with_prompt("Cardinality")
            .items(&["one", "many", "optional"])
            .default(0)
            .interact()?;
        let cardinality = match cardinality_idx {
            1 => Cardinality::Many,
            2 => Cardinality::Optional,
            _ => Cardinality::One,
        };

        let default_mount = format!("/workspace/{}", name);
        let mount: String = Input::with_theme(theme)
            .with_prompt("Mount path in container")
            .default(default_mount)
            .interact_text()?;

        let description: String = Input::with_theme(theme)
            .with_prompt("Description (optional)")
            .allow_empty(true)
            .interact_text()?;

        specs.push(IoSpec {
            name,
            r#type: type_str.parse().map_err(|e| anyhow::anyhow!("{}", e))?,
            cardinality,
            mount: Some(PathBuf::from(mount)),
            description: non_empty(description),
            default: None,
        });
    }

    Ok(specs)
}

fn prompt_type(theme: &ColorfulTheme) -> anyhow::Result<String> {
    loop {
        let input: String = Input::with_theme(theme)
            .with_prompt("Type (enter ? to list all types)")
            .interact_text()?;

        if input == "?" {
            print_type_list();
            continue;
        }

        let base = input.split('[').next().unwrap_or(&input);
        if bv_types::lookup(base).is_some() {
            return Ok(input);
        }
        if let Some(suggestion) = bv_types::suggest(base) {
            eprintln!(
                "  {} unknown type '{}', did you mean '{}'?",
                "hint:".if_supports_color(Stream::Stderr, |t| t.yellow().to_string()),
                base,
                suggestion
            );
        } else {
            eprintln!(
                "  {} unknown type '{}'; enter ? to list all types",
                "hint:".if_supports_color(Stream::Stderr, |t| t.yellow().to_string()),
                base
            );
        }
    }
}

fn print_type_list() {
    let mut ids: Vec<&str> = bv_types::known_type_ids().collect();
    ids.sort_unstable();

    eprintln!(
        "\n  {}",
        "Available types:".if_supports_color(Stream::Stderr, |t| t.bold().to_string())
    );
    for id in ids {
        if let Some(def) = bv_types::lookup(id) {
            eprintln!("    {:20} {}", id, def.description);
        }
    }
    eprintln!();
}

fn parse_io_specs(metas: &[IoMeta]) -> anyhow::Result<Vec<IoSpec>> {
    metas
        .iter()
        .map(|m| {
            let type_ref = m
                .type_str
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid type '{}': {}", m.type_str, e))?;
            let cardinality = match m.cardinality.as_str() {
                "many" => Cardinality::Many,
                "optional" => Cardinality::Optional,
                _ => Cardinality::One,
            };
            Ok(IoSpec {
                name: m.name.clone(),
                r#type: type_ref,
                cardinality,
                mount: m.mount.as_deref().map(PathBuf::from),
                description: m.description.clone(),
                default: None,
            })
        })
        .collect()
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}
