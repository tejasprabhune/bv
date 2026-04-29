use std::path::{Path, PathBuf};

use anyhow::Context;
use owo_colors::{OwoColorize, Stream};

pub enum BuildSystem {
    Dockerfile,
    CondaEnv { file: PathBuf },
    Pyproject,
    Requirements,
    CargoToml,
    Makefile,
    Unknown,
}

impl BuildSystem {
    pub fn description(&self) -> &str {
        match self {
            BuildSystem::Dockerfile => "Dockerfile at root",
            BuildSystem::CondaEnv { .. } => "conda environment file",
            BuildSystem::Pyproject => "pyproject.toml (Python package)",
            BuildSystem::Requirements => "requirements.txt (Python)",
            BuildSystem::CargoToml => "Cargo.toml (Rust)",
            BuildSystem::Makefile => "Makefile (C/C++)",
            BuildSystem::Unknown => "no build system detected",
        }
    }
}

pub fn detect(dir: &Path) -> BuildSystem {
    if dir.join("Dockerfile").exists() {
        return BuildSystem::Dockerfile;
    }
    if dir.join("environment.yml").exists() {
        return BuildSystem::CondaEnv {
            file: dir.join("environment.yml"),
        };
    }
    if dir.join("environment.yaml").exists() {
        return BuildSystem::CondaEnv {
            file: dir.join("environment.yaml"),
        };
    }
    if dir.join("pyproject.toml").exists() && has_build_system(dir) {
        return BuildSystem::Pyproject;
    }
    if dir.join("requirements.txt").exists() {
        return BuildSystem::Requirements;
    }
    if dir.join("Cargo.toml").exists() {
        return BuildSystem::CargoToml;
    }
    if dir.join("Makefile").exists() {
        return BuildSystem::Makefile;
    }
    BuildSystem::Unknown
}

/// Return the path to the Dockerfile to use, generating one if needed.
pub fn ensure_dockerfile(sys: &BuildSystem, dir: &Path) -> anyhow::Result<PathBuf> {
    let existing = dir.join("Dockerfile");
    if matches!(sys, BuildSystem::Dockerfile) {
        return Ok(existing);
    }

    let content = match sys {
        BuildSystem::CondaEnv { file } => {
            let filename = file.file_name().unwrap_or_default().to_string_lossy();
            conda_dockerfile(&filename)
        }
        BuildSystem::Pyproject => pyproject_dockerfile(),
        BuildSystem::Requirements => requirements_dockerfile(),
        BuildSystem::CargoToml => cargo_dockerfile(),
        BuildSystem::Makefile => makefile_dockerfile(),
        BuildSystem::Unknown | BuildSystem::Dockerfile => {
            anyhow::bail!(
                "no supported build system found and no Dockerfile present\n  \
                 Add a Dockerfile to the directory or create bv-publish.toml"
            );
        }
    };

    let path = dir.join("Dockerfile.bv");
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    eprintln!(
        "  {} {}",
        "Generated".if_supports_color(Stream::Stderr, |t| t.dimmed().to_string()),
        path.display()
    );
    Ok(path)
}

/// Return the directory where the upstream source lives inside the built image.
/// Scans every `WORKDIR` directive in the Dockerfile and returns the last one
/// that isn't `/workspace` (which our scaffolds use for runtime cwd).
///
/// Returns `None` if the Dockerfile can't be read or has no usable WORKDIR.
pub fn detect_image_workdir(dockerfile_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(dockerfile_path).ok()?;
    let mut last_build_dir: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        // Strip the WORKDIR keyword (case-insensitive).
        let rest = trimmed
            .strip_prefix("WORKDIR ")
            .or_else(|| trimmed.strip_prefix("workdir "));
        let Some(value) = rest else { continue };
        let value = value.trim();
        if value == "/workspace" || value.is_empty() {
            continue;
        }
        last_build_dir = Some(value.to_string());
    }
    last_build_dir
}

fn has_build_system(dir: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(dir.join("pyproject.toml")) else {
        return false;
    };
    content.contains("[build-system]")
}

fn conda_dockerfile(env_file: &str) -> String {
    format!(
        "FROM mambaorg/micromamba:1.5\n\
         USER root\n\
         COPY {env_file} /tmp/environment.yml\n\
         RUN micromamba install -y -n base -f /tmp/environment.yml && \\\n\
             micromamba clean --all --yes\n\
         WORKDIR /workspace\n"
    )
}

fn pyproject_dockerfile() -> String {
    "FROM python:3.11-slim\n\
     WORKDIR /app\n\
     COPY pyproject.toml .\n\
     COPY . .\n\
     RUN pip install --no-cache-dir .\n\
     WORKDIR /workspace\n"
        .to_string()
}

fn requirements_dockerfile() -> String {
    "FROM python:3.11-slim\n\
     WORKDIR /app\n\
     COPY requirements.txt .\n\
     RUN pip install --no-cache-dir -r requirements.txt\n\
     COPY . .\n\
     WORKDIR /workspace\n"
        .to_string()
}

fn cargo_dockerfile() -> String {
    "FROM rust:1.75 AS builder\n\
     WORKDIR /build\n\
     COPY . .\n\
     RUN cargo build --release\n\
     \n\
     FROM debian:bookworm-slim\n\
     RUN apt-get update && apt-get install -y --no-install-recommends \\\n\
         libssl3 ca-certificates && \\\n\
         rm -rf /var/lib/apt/lists/*\n\
     COPY --from=builder /build/target/release/ /build-out/\n\
     RUN find /build-out -maxdepth 1 -type f -executable \\\n\
         -exec mv {} /usr/local/bin/ \\;\n\
     WORKDIR /workspace\n"
        .to_string()
}

fn makefile_dockerfile() -> String {
    "FROM debian:bookworm-slim\n\
     RUN apt-get update && apt-get install -y --no-install-recommends \\\n\
         build-essential && \\\n\
         rm -rf /var/lib/apt/lists/*\n\
     WORKDIR /build\n\
     COPY . .\n\
     RUN make\n\
     WORKDIR /workspace\n"
        .to_string()
}
