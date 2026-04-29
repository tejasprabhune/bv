use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use bv_core::manifest::{
    EntrypointSpec, GpuSpec, HardwareSpec, ImageSpec, IoSpec, Manifest, Tier, ToolManifest,
};
use bv_types::Cardinality;
use inquire::{Confirm, CustomType, Select, Text};
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
    /// Optional subcommand map: name -> argv prefix (space-separated string in
    /// TOML, e.g. `train = "python genie/train.py"`).
    #[serde(default)]
    pub subcommands: HashMap<String, String>,
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
    /// Empty string means "no entrypoint" — only valid when `subcommands` is non-empty.
    pub entrypoint_command: String,
    pub args_template: Option<String>,
    pub subcommands: HashMap<String, Vec<String>>,
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
                entrypoint: if self.entrypoint_command.is_empty() {
                    None
                } else {
                    Some(EntrypointSpec {
                        command: self.entrypoint_command.clone(),
                        args_template: self.args_template.clone(),
                        env: Default::default(),
                    })
                },
                subcommands: self.subcommands.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                cache_paths: vec![],
                binaries: None,
                smoke: None,
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
    image_workdir: Option<&str>,
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
                 Set it in bv-publish.toml or pass --tool-version"
            )
        })?;

    let default_meta = PublishMeta::default();
    let m = meta.unwrap_or(&default_meta);

    let inputs = parse_io_specs(&m.inputs).context("inputs")?;
    let outputs = parse_io_specs(&m.outputs).context("outputs")?;

    let mut subcommands = parse_subcommand_strings(&m.subcommands)?;
    if let Some(wd) = image_workdir {
        for argv in subcommands.values_mut() {
            rewrite_relative_script_paths(argv, wd);
        }
    }
    let entrypoint_command = m.entrypoint.command.clone().unwrap_or_default();
    if entrypoint_command.is_empty() && subcommands.is_empty() {
        anyhow::bail!(
            "either entrypoint.command or [publish.subcommands] must be set in bv-publish.toml \
             (or supplied interactively)"
        );
    }

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
        entrypoint_command,
        args_template: m.entrypoint.args_template.clone(),
        subcommands,
    })
}

fn parse_subcommand_strings(
    raw: &HashMap<String, String>,
) -> anyhow::Result<HashMap<String, Vec<String>>> {
    raw.iter()
        .map(|(k, v)| {
            let argv: Vec<String> =
                shell_split(v).with_context(|| format!("subcommand '{k}': bad shell quoting"))?;
            if argv.is_empty() {
                anyhow::bail!("subcommand '{k}': command must not be empty");
            }
            Ok((k.clone(), argv))
        })
        .collect()
}

/// Tiny POSIX-ish shell split. Handles single/double quotes and backslash
/// escapes; no env expansion. Sufficient for `python genie/sample.py --foo "bar baz"`.
fn shell_split(s: &str) -> anyhow::Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                    has_token = true;
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_token {
                    out.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if in_single || in_double {
        anyhow::bail!("unterminated quote");
    }
    if has_token {
        out.push(cur);
    }
    Ok(out)
}

/// Common SPDX identifiers offered in the License picker. Final entry triggers
/// a free-form Text prompt; "(none)" leaves the field unset.
const SPDX_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-3-Clause",
    "BSD-2-Clause",
    "GPL-3.0-only",
    "GPL-2.0-only",
    "LGPL-3.0-only",
    "MPL-2.0",
    "AGPL-3.0-only",
    "Unlicense",
    "Proprietary",
    "(none)",
    "Custom…",
];

/// Working state for the review-and-edit interactive flow.
///
/// Sequential prompts are subject to paste cascade: when a user pastes
/// multi-line content into one field, embedded `\n`s submit subsequent
/// prompts and silently scatter their data across fields. This struct holds
/// pre-filled defaults and is mutated by individual edit actions invoked
/// from a top-level menu — there is no auto-advance, so cascade can't
/// happen.
struct Form {
    name: String,
    version: String,
    description: String,
    homepage: String,
    license: String,
    cpu_cores: u32,
    ram_gb: f64,
    disk_gb: f64,
    needs_gpu: bool,
    inputs: Vec<IoSpec>,
    outputs: Vec<IoSpec>,
    entrypoint_command: String,
    args_template: String,
    subcommands: HashMap<String, Vec<String>>,
}

impl Form {
    fn from_defaults(
        meta: Option<&PublishMeta>,
        fetched: &FetchedSource,
        name_override: Option<&str>,
        version_override: Option<&str>,
    ) -> anyhow::Result<Self> {
        let name = name_override
            .map(|s| s.to_string())
            .or_else(|| meta.and_then(|m| m.name.clone()))
            .unwrap_or_else(|| fetched.name_hint.clone());

        let version = version_override
            .map(|s| s.to_string())
            .or_else(|| meta.and_then(|m| m.version.clone()))
            .or_else(|| fetched.version_hint.clone())
            .unwrap_or_else(|| "0.1.0".to_string());

        let inputs = parse_io_specs(meta.map(|m| m.inputs.as_slice()).unwrap_or(&[]))?;
        let outputs = parse_io_specs(meta.map(|m| m.outputs.as_slice()).unwrap_or(&[]))?;
        let subcommands = meta
            .map(|m| parse_subcommand_strings(&m.subcommands))
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            name,
            version,
            description: meta.and_then(|m| m.description.clone()).unwrap_or_default(),
            homepage: meta
                .and_then(|m| m.homepage.clone())
                .unwrap_or_else(|| fetched.source_url.clone()),
            license: meta.and_then(|m| m.license.clone()).unwrap_or_default(),
            cpu_cores: meta.and_then(|m| m.hardware.cpu_cores).unwrap_or(4),
            ram_gb: meta.and_then(|m| m.hardware.ram_gb).unwrap_or(8.0),
            disk_gb: meta.and_then(|m| m.hardware.disk_gb).unwrap_or(2.0),
            needs_gpu: meta.and_then(|m| m.hardware.needs_gpu).unwrap_or(false),
            inputs,
            outputs,
            entrypoint_command: meta
                .and_then(|m| m.entrypoint.command.clone())
                .unwrap_or_default(),
            args_template: meta
                .and_then(|m| m.entrypoint.args_template.clone())
                .unwrap_or_default(),
            subcommands,
        })
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.name.is_empty() {
            return Err("tool name is required");
        }
        if self.version.is_empty() {
            return Err("version is required");
        }
        if self.entrypoint_command.is_empty() && self.subcommands.is_empty() {
            return Err("declare either an entrypoint command or at least one subcommand");
        }
        Ok(())
    }

    fn into_result(self) -> ScaffoldResult {
        ScaffoldResult {
            name: self.name,
            version: self.version,
            description: non_empty(self.description),
            homepage: non_empty(self.homepage),
            license: non_empty(self.license),
            cpu_cores: self.cpu_cores,
            ram_gb: self.ram_gb,
            disk_gb: self.disk_gb,
            needs_gpu: self.needs_gpu,
            inputs: self.inputs,
            outputs: self.outputs,
            entrypoint_command: self.entrypoint_command,
            args_template: non_empty(self.args_template),
            subcommands: self.subcommands,
        }
    }
}

/// Interactive path: pre-fills from config + hints, then drops into a
/// review-and-edit menu so the user can revisit any field before
/// confirming. Each edit is taken in isolation, which sidesteps multi-line
/// paste cascade entirely (there's no next-prompt to silently swallow
/// embedded newlines).
pub fn interactive(
    config: Option<&PublishConfig>,
    fetched: &FetchedSource,
    name_override: Option<&str>,
    version_override: Option<&str>,
    image_workdir: Option<&str>,
) -> anyhow::Result<ScaffoldResult> {
    let meta = config.map(|c| &c.publish);
    let mut form = Form::from_defaults(meta, fetched, name_override, version_override)?;

    eprintln!();
    loop {
        let menu = build_menu(&form);
        let choice = Select::new("Edit any field, then choose Confirm:", menu)
            .with_page_size(20)
            .prompt()?;

        match choice.action {
            Action::Confirm => match form.validate() {
                Ok(()) => {
                    // Safety net: rewrite any subcommand argv that was carried
                    // over from bv-publish.toml without re-edit. Idempotent
                    // (already-absolute paths are skipped).
                    if let Some(wd) = image_workdir {
                        for argv in form.subcommands.values_mut() {
                            rewrite_relative_script_paths(argv, wd);
                        }
                    }
                    return Ok(form.into_result());
                }
                Err(msg) => eprintln!(
                    "  {} {msg}",
                    "error:".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string())
                ),
            },
            Action::Cancel => anyhow::bail!("publish cancelled"),
            Action::Edit(field) => edit_field(&mut form, field, image_workdir)?,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Version,
    Description,
    Homepage,
    License,
    CpuCores,
    RamGb,
    DiskGb,
    NeedsGpu,
    Inputs,
    Outputs,
    Entrypoint,
    Subcommands,
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Confirm,
    Cancel,
    Edit(Field),
}

#[derive(Debug, Clone)]
struct MenuItem {
    label: String,
    action: Action,
}

impl std::fmt::Display for MenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn build_menu(form: &Form) -> Vec<MenuItem> {
    fn row(label: &str, value: impl AsRef<str>, action: Action) -> MenuItem {
        let v = value.as_ref();
        let display = if v.is_empty() {
            "(none)".to_string()
        } else {
            v.to_string()
        };
        MenuItem {
            label: format!("{label:<14}  {display}"),
            action,
        }
    }

    let inputs_summary = if form.inputs.is_empty() {
        "(none)".to_string()
    } else {
        format!(
            "{} declared: {}",
            form.inputs.len(),
            form.inputs
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let outputs_summary = if form.outputs.is_empty() {
        "(none)".to_string()
    } else {
        format!(
            "{} declared: {}",
            form.outputs.len(),
            form.outputs
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let entrypoint_summary = if form.entrypoint_command.is_empty() {
        "(none)".to_string()
    } else if form.args_template.is_empty() {
        form.entrypoint_command.clone()
    } else {
        format!("{} {}", form.entrypoint_command, form.args_template)
    };
    let subs_summary = if form.subcommands.is_empty() {
        "(none)".to_string()
    } else {
        let mut names: Vec<&str> = form.subcommands.keys().map(|s| s.as_str()).collect();
        names.sort();
        format!("{} declared: {}", form.subcommands.len(), names.join(", "))
    };

    vec![
        MenuItem {
            label: "Confirm and continue".into(),
            action: Action::Confirm,
        },
        row("Tool name", &form.name, Action::Edit(Field::Name)),
        row("Version", &form.version, Action::Edit(Field::Version)),
        row(
            "Description",
            &form.description,
            Action::Edit(Field::Description),
        ),
        row("Homepage", &form.homepage, Action::Edit(Field::Homepage)),
        row("License", &form.license, Action::Edit(Field::License)),
        row(
            "CPU cores",
            form.cpu_cores.to_string(),
            Action::Edit(Field::CpuCores),
        ),
        row(
            "RAM (GB)",
            format!("{}", form.ram_gb),
            Action::Edit(Field::RamGb),
        ),
        row(
            "Disk (GB)",
            format!("{}", form.disk_gb),
            Action::Edit(Field::DiskGb),
        ),
        row(
            "GPU required",
            if form.needs_gpu { "yes" } else { "no" },
            Action::Edit(Field::NeedsGpu),
        ),
        row("Inputs", inputs_summary, Action::Edit(Field::Inputs)),
        row("Outputs", outputs_summary, Action::Edit(Field::Outputs)),
        row(
            "Entrypoint",
            entrypoint_summary,
            Action::Edit(Field::Entrypoint),
        ),
        row(
            "Subcommands",
            subs_summary,
            Action::Edit(Field::Subcommands),
        ),
        MenuItem {
            label: "Cancel".into(),
            action: Action::Cancel,
        },
    ]
}

fn edit_field(form: &mut Form, field: Field, image_workdir: Option<&str>) -> anyhow::Result<()> {
    match field {
        Field::Name => form.name = text("Tool name", &form.name, false)?,
        Field::Version => form.version = text("Version", &form.version, false)?,
        Field::Description => form.description = text("Description", &form.description, true)?,
        Field::Homepage => form.homepage = text("Homepage URL", &form.homepage, true)?,
        Field::License => form.license = pick_license(&form.license)?,
        Field::CpuCores => form.cpu_cores = number("CPU cores", form.cpu_cores)?,
        Field::RamGb => form.ram_gb = number("RAM (GB)", form.ram_gb)?,
        Field::DiskGb => form.disk_gb = number("Disk (GB)", form.disk_gb)?,
        Field::NeedsGpu => {
            form.needs_gpu = Confirm::new("Needs GPU?")
                .with_default(form.needs_gpu)
                .prompt()?;
        }
        Field::Inputs => edit_io_list(&mut form.inputs, "input")?,
        Field::Outputs => edit_io_list(&mut form.outputs, "output")?,
        Field::Entrypoint => edit_entrypoint(form)?,
        Field::Subcommands => edit_subcommands(&mut form.subcommands, image_workdir)?,
    }
    Ok(())
}

fn text(prompt: &str, current: &str, allow_empty: bool) -> anyhow::Result<String> {
    let mut t = Text::new(prompt);
    if !current.is_empty() {
        t = t.with_initial_value(current);
    }
    let v = t.prompt()?;
    if !allow_empty && v.is_empty() {
        anyhow::bail!("{prompt} must not be empty");
    }
    Ok(v)
}

fn number<T>(prompt: &str, current: T) -> anyhow::Result<T>
where
    T: Clone + std::fmt::Display + std::str::FromStr,
    T::Err: std::fmt::Debug + std::fmt::Display,
{
    let v = CustomType::<T>::new(prompt)
        .with_default(current)
        .with_error_message("please enter a valid number")
        .prompt()?;
    Ok(v)
}

fn pick_license(current: &str) -> anyhow::Result<String> {
    let cursor = SPDX_LICENSES
        .iter()
        .position(|x| *x == current)
        .unwrap_or(SPDX_LICENSES.len() - 2); // (none) by default
    let chosen = Select::new("License", SPDX_LICENSES.to_vec())
        .with_starting_cursor(cursor)
        .prompt()?;

    Ok(match chosen {
        "(none)" => String::new(),
        "Custom…" => Text::new("Custom SPDX identifier (or full text)")
            .with_initial_value(current)
            .prompt()?,
        other => other.to_string(),
    })
}

fn edit_entrypoint(form: &mut Form) -> anyhow::Result<()> {
    let want = Confirm::new("Declare an entrypoint command?")
        .with_default(!form.entrypoint_command.is_empty())
        .with_help_message(if form.subcommands.is_empty() {
            "required when no subcommands are declared"
        } else {
            "optional — subcommands cover this tool"
        })
        .prompt()?;

    if !want {
        form.entrypoint_command.clear();
        form.args_template.clear();
        return Ok(());
    }
    let default_cmd = if form.entrypoint_command.is_empty() {
        form.name.clone()
    } else {
        form.entrypoint_command.clone()
    };
    form.entrypoint_command = Text::new("Command")
        .with_initial_value(&default_cmd)
        .prompt()?;
    let tmpl = Text::new("Args template")
        .with_help_message("use {port_name}, {cpu_cores}; leave blank to skip")
        .with_initial_value(&form.args_template)
        .prompt()?;
    form.args_template = tmpl;
    Ok(())
}

fn edit_io_list(specs: &mut Vec<IoSpec>, label: &str) -> anyhow::Result<()> {
    loop {
        #[derive(Debug, Clone)]
        struct Row {
            label: String,
            kind: RowKind,
        }
        #[derive(Debug, Clone)]
        enum RowKind {
            Done,
            Add,
            Edit(usize),
            Remove(usize),
        }
        impl std::fmt::Display for Row {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.label)
            }
        }

        let mut rows: Vec<Row> = vec![Row {
            label: "Done".into(),
            kind: RowKind::Done,
        }];
        rows.push(Row {
            label: format!("Add {label}"),
            kind: RowKind::Add,
        });
        for (i, spec) in specs.iter().enumerate() {
            rows.push(Row {
                label: format!(
                    "Edit: {} [{}] ({})",
                    spec.name, spec.r#type, spec.cardinality
                ),
                kind: RowKind::Edit(i),
            });
            rows.push(Row {
                label: format!("Remove: {}", spec.name),
                kind: RowKind::Remove(i),
            });
        }

        let pick = Select::new(&format!("{label}s"), rows)
            .with_page_size(20)
            .prompt()?;
        match pick.kind {
            RowKind::Done => return Ok(()),
            RowKind::Add => {
                let s = prompt_io_spec(label, None)?;
                specs.push(s);
            }
            RowKind::Edit(i) => {
                let s = prompt_io_spec(label, Some(&specs[i]))?;
                specs[i] = s;
            }
            RowKind::Remove(i) => {
                specs.remove(i);
            }
        }
    }
}

fn prompt_io_spec(label: &str, existing: Option<&IoSpec>) -> anyhow::Result<IoSpec> {
    let initial_name = existing.map(|s| s.name.clone()).unwrap_or_default();
    let name = Text::new(&format!("{label} name"))
        .with_initial_value(&initial_name)
        .prompt()?;
    if name.is_empty() {
        anyhow::bail!("name must not be empty");
    }

    let initial_type = existing.map(|s| s.r#type.to_string()).unwrap_or_default();
    let type_str = prompt_type(&initial_type)?;

    let cardinalities = vec!["one", "many", "optional"];
    let cursor = existing
        .map(|s| match s.cardinality {
            Cardinality::One => 0,
            Cardinality::Many => 1,
            Cardinality::Optional => 2,
        })
        .unwrap_or(0);
    let cardinality = match Select::new("Cardinality", cardinalities)
        .with_starting_cursor(cursor)
        .prompt()?
    {
        "many" => Cardinality::Many,
        "optional" => Cardinality::Optional,
        _ => Cardinality::One,
    };

    let default_mount = existing
        .and_then(|s| s.mount.as_ref().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| format!("/workspace/{name}"));
    let mount = Text::new("Mount path in container")
        .with_initial_value(&default_mount)
        .prompt()?;

    let initial_desc = existing
        .and_then(|s| s.description.clone())
        .unwrap_or_default();
    let description = Text::new("Description (optional)")
        .with_initial_value(&initial_desc)
        .prompt()?;

    Ok(IoSpec {
        name,
        r#type: type_str.parse().map_err(|e| anyhow::anyhow!("{}", e))?,
        cardinality,
        mount: Some(PathBuf::from(mount)),
        description: non_empty(description),
        default: None,
    })
}

fn edit_subcommands(
    subs: &mut HashMap<String, Vec<String>>,
    image_workdir: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(wd) = image_workdir {
        eprintln!(
            "  {} subcommand script paths are inside the image; relative paths \
             will be resolved against the image's source dir ({wd})",
            "note:".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
        );
    }
    loop {
        #[derive(Debug, Clone)]
        struct Row {
            label: String,
            kind: SubKind,
        }
        #[derive(Debug, Clone)]
        enum SubKind {
            Done,
            Add,
            Edit(String),
            Remove(String),
        }
        impl std::fmt::Display for Row {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.label)
            }
        }

        let mut rows: Vec<Row> = vec![
            Row {
                label: "Done".into(),
                kind: SubKind::Done,
            },
            Row {
                label: "Add subcommand".into(),
                kind: SubKind::Add,
            },
        ];
        let mut names: Vec<&String> = subs.keys().collect();
        names.sort();
        for n in names {
            let cmd = subs[n].join(" ");
            rows.push(Row {
                label: format!("Edit: {n} = {cmd}"),
                kind: SubKind::Edit(n.clone()),
            });
            rows.push(Row {
                label: format!("Remove: {n}"),
                kind: SubKind::Remove(n.clone()),
            });
        }

        let pick = Select::new("Subcommands", rows)
            .with_page_size(20)
            .with_help_message("for multi-script tools, e.g. genie2 train, genie2 sample")
            .prompt()?;
        match pick.kind {
            SubKind::Done => return Ok(()),
            SubKind::Add => {
                let (n, argv) = prompt_subcommand("", &[], image_workdir)?;
                subs.insert(n, argv);
            }
            SubKind::Edit(name) => {
                let existing = subs.get(&name).cloned().unwrap_or_default();
                let (new_name, argv) = prompt_subcommand(&name, &existing, image_workdir)?;
                if new_name != name {
                    subs.remove(&name);
                }
                subs.insert(new_name, argv);
            }
            SubKind::Remove(name) => {
                subs.remove(&name);
            }
        }
    }
}

fn prompt_subcommand(
    initial_name: &str,
    initial_cmd: &[String],
    image_workdir: Option<&str>,
) -> anyhow::Result<(String, Vec<String>)> {
    let name = Text::new("Subcommand name")
        .with_help_message("e.g. train, sample_unconditional")
        .with_initial_value(initial_name)
        .prompt()?;
    if name.is_empty() || name.starts_with('-') {
        anyhow::bail!("name must be non-empty and not start with '-'");
    }
    let initial_cmd_str = initial_cmd.join(" ");
    let help = match image_workdir {
        Some(wd) => format!("e.g. python genie/train.py (resolved against {wd})"),
        None => "e.g. python genie/train.py".to_string(),
    };
    let cmd_str = Text::new("Command")
        .with_help_message(&help)
        .with_initial_value(&initial_cmd_str)
        .prompt()?;
    let mut argv = shell_split(&cmd_str)?;
    if argv.is_empty() {
        anyhow::bail!("command must not be empty");
    }
    if let Some(wd) = image_workdir {
        rewrite_relative_script_paths(&mut argv, wd);
    }
    Ok((name, argv))
}

/// Rewrite tokens that look like relative script paths to be image-absolute.
///
/// A token is rewritten only when ALL of the following hold:
///   - doesn't start with `-` (rules out `--config`, `-m`, etc.)
///   - doesn't start with `/` (already absolute)
///   - contains no `=` and no `:` (rules out flag-with-value like
///     `--config=cfg/x.yaml`, URLs like `s3://bucket/key`, and env-style
///     `VAR=val/x`)
///   - AND either ends with a known script extension (`.py`, `.sh`, `.R`,
///     `.pl`, `.js`, `.ts`), OR contains a `/` and the basename has an
///     extension (so `data/` and `genie/sample` aren't rewritten).
fn rewrite_relative_script_paths(argv: &mut [String], image_workdir: &str) {
    const SCRIPT_EXTS: &[&str] = &[".py", ".sh", ".R", ".pl", ".js", ".ts"];
    let wd = image_workdir.trim_end_matches('/');
    for token in argv.iter_mut() {
        if token.is_empty() {
            continue;
        }
        if token.starts_with('-') || token.starts_with('/') {
            continue;
        }
        if token.contains('=') || token.contains(':') {
            continue;
        }
        let ends_with_script_ext = SCRIPT_EXTS.iter().any(|ext| token.ends_with(ext));
        let basename_has_ext = token.contains('/') && {
            let base = token.rsplit('/').next().unwrap_or("");
            base.rsplit_once('.')
                .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
        };
        if ends_with_script_ext || basename_has_ext {
            *token = format!("{wd}/{token}");
        }
    }
}

const CUSTOM_TYPE_OPTION: &str = "(custom or parametric type, e.g. fasta[alphabet=dna]…)";

fn prompt_type(initial: &str) -> anyhow::Result<String> {
    let mut ids: Vec<&'static str> = bv_types::known_type_ids().collect();
    ids.sort_unstable();

    let display: Vec<String> = ids
        .iter()
        .map(|id| {
            let desc = bv_types::lookup(id)
                .map(|d| d.description.as_str())
                .unwrap_or("");
            if desc.is_empty() {
                (*id).to_string()
            } else {
                format!(
                    "{id:<14} {}",
                    desc.if_supports_color(Stream::Stderr, |t| t.dimmed().to_string())
                )
            }
        })
        .chain(std::iter::once(CUSTOM_TYPE_OPTION.to_string()))
        .collect();

    // Pre-select the existing/initial type when re-editing.
    let initial_base = initial.split('[').next().unwrap_or("");
    let cursor = ids
        .iter()
        .position(|id| *id == initial_base)
        .unwrap_or(display.len() - 1);

    let picked = Select::new("Type", display)
        .with_starting_cursor(cursor)
        .with_page_size(15)
        .prompt()?;

    if picked == CUSTOM_TYPE_OPTION {
        return prompt_custom_type(initial);
    }

    let id = picked.split_whitespace().next().unwrap_or("file").to_string();
    Ok(id)
}

fn prompt_custom_type(initial: &str) -> anyhow::Result<String> {
    loop {
        let input = Text::new("Custom type (id or id[param=value])")
            .with_initial_value(initial)
            .prompt()?;
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
                "  {} unknown type '{}'",
                "hint:".if_supports_color(Stream::Stderr, |t| t.yellow().to_string()),
                base
            );
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rewrites_relative_script_paths() {
        let mut a = argv(&["python", "genie/train.py"]);
        rewrite_relative_script_paths(&mut a, "/app/genie2");
        assert_eq!(a, argv(&["python", "/app/genie2/genie/train.py"]));
    }

    #[test]
    fn leaves_absolute_paths_alone() {
        let mut a = argv(&["python", "/usr/local/bin/run.py"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["python", "/usr/local/bin/run.py"]));
    }

    #[test]
    fn leaves_command_names_alone() {
        let mut a = argv(&["python", "-m", "scripts.train"]);
        rewrite_relative_script_paths(&mut a, "/app");
        // -m and dotted module path don't look like file paths.
        assert_eq!(a, argv(&["python", "-m", "scripts.train"]));
    }

    #[test]
    fn rewrites_bare_script_filename() {
        let mut a = argv(&["bash", "setup.sh"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["bash", "/app/setup.sh"]));
    }

    #[test]
    fn idempotent_when_already_absolute() {
        let mut a = argv(&["python", "/app/x.py"]);
        rewrite_relative_script_paths(&mut a, "/app");
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["python", "/app/x.py"]));
    }

    #[test]
    fn does_not_rewrite_flag_with_value() {
        let mut a = argv(&["python", "train.py", "--config=cfg/x.yaml"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(
            a,
            argv(&["python", "/app/train.py", "--config=cfg/x.yaml"])
        );
    }

    #[test]
    fn does_not_rewrite_url_like_token() {
        let mut a = argv(&["aws", "s3", "cp", "s3://bucket/key", "out"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["aws", "s3", "cp", "s3://bucket/key", "out"]));
    }

    #[test]
    fn does_not_rewrite_https_url() {
        let mut a = argv(&["curl", "https://example.com/x"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["curl", "https://example.com/x"]));
    }

    #[test]
    fn does_not_rewrite_env_style_var() {
        let mut a = argv(&["VAR=val/x", "python", "train.py"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["VAR=val/x", "python", "/app/train.py"]));
    }

    #[test]
    fn does_not_rewrite_flag_starting_with_dash() {
        let mut a = argv(&["python", "train.py", "--num-gpus=4"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["python", "/app/train.py", "--num-gpus=4"]));
    }

    #[test]
    fn does_not_rewrite_data_dir_with_trailing_slash() {
        let mut a = argv(&["train", "--data-dir=data/"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["train", "--data-dir=data/"]));
    }

    #[test]
    fn rewrites_relative_script_with_subpath() {
        let mut a = argv(&["python", "genie/train.py"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["python", "/app/genie/train.py"]));
    }

    #[test]
    fn rewrites_bare_setup_sh() {
        let mut a = argv(&["setup.sh"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["/app/setup.sh"]));
    }

    #[test]
    fn does_not_rewrite_plain_command_python() {
        let mut a = argv(&["python"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["python"]));
    }

    #[test]
    fn does_not_rewrite_absolute_script_path() {
        let mut a = argv(&["/abs/path.py"]);
        rewrite_relative_script_paths(&mut a, "/app");
        assert_eq!(a, argv(&["/abs/path.py"]));
    }
}
