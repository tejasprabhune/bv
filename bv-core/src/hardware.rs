use std::fmt;
use std::process::Command;

use sysinfo::{Disks, System};

use crate::manifest::CudaVersion;

// Detected hardware

#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub vram_mb: u64,
    pub driver_version: Option<String>,
    pub cuda_version: Option<CudaVersion>,
}

#[derive(Debug, Clone)]
pub struct DetectedHardware {
    pub cpu_cores: u32,
    pub ram_mb: u64,
    pub disk_free_mb: u64,
    pub gpus: Vec<GpuInfo>,
}

impl DetectedHardware {
    pub fn detect() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();

        let cpu_cores = sys.cpus().len() as u32;
        let ram_mb = sys.total_memory() / (1024 * 1024);

        let disk_free_mb = {
            let disks = Disks::new_with_refreshed_list();
            // Use the disk with the most free space as a conservative proxy.
            disks
                .iter()
                .map(|d| d.available_space() / (1024 * 1024))
                .max()
                .unwrap_or(0)
        };

        let gpus = detect_gpus();

        Self {
            cpu_cores,
            ram_mb,
            disk_free_mb,
            gpus,
        }
    }

    pub fn ram_gb(&self) -> f64 {
        self.ram_mb as f64 / 1024.0
    }

    pub fn disk_free_gb(&self) -> f64 {
        self.disk_free_mb as f64 / 1024.0
    }
}

// Hardware mismatch

#[derive(Debug, Clone)]
pub enum HardwareMismatch {
    NoGpu,
    InsufficientVram {
        required_gb: u32,
        available_gb: u32,
    },
    CudaTooOld {
        required: CudaVersion,
        available: CudaVersion,
    },
    NoCuda {
        required: CudaVersion,
    },
    InsufficientRam {
        required_gb: f64,
        available_gb: f64,
    },
    InsufficientDisk {
        required_gb: f64,
        available_gb: f64,
    },
}

impl fmt::Display for HardwareMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HardwareMismatch::NoGpu => {
                write!(f, "NVIDIA GPU required but none detected")
            }
            HardwareMismatch::InsufficientVram {
                required_gb,
                available_gb,
            } => {
                write!(
                    f,
                    "GPU requires ≥{required_gb} GB VRAM, but best available is {available_gb} GB"
                )
            }
            HardwareMismatch::CudaTooOld {
                required,
                available,
            } => {
                write!(f, "CUDA ≥{required} required, driver supports {available}")
            }
            HardwareMismatch::NoCuda { required } => {
                write!(f, "CUDA ≥{required} required but no CUDA driver detected")
            }
            HardwareMismatch::InsufficientRam {
                required_gb,
                available_gb,
            } => {
                write!(
                    f,
                    "{required_gb:.0} GB RAM required, only {available_gb:.1} GB available"
                )
            }
            HardwareMismatch::InsufficientDisk {
                required_gb,
                available_gb,
            } => {
                write!(
                    f,
                    "{required_gb:.0} GB free disk required, only {available_gb:.1} GB available"
                )
            }
        }
    }
}

// GPU detection

fn detect_gpus() -> Vec<GpuInfo> {
    let output = match Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,driver_version",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let cuda_ver = detect_cuda_version();
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter_map(|line| parse_gpu_csv(line, cuda_ver.clone()))
        .collect()
}

fn parse_gpu_csv(line: &str, cuda_version: Option<CudaVersion>) -> Option<GpuInfo> {
    let parts: Vec<&str> = line.splitn(3, ',').map(str::trim).collect();
    if parts.len() < 2 {
        return None;
    }
    let name = parts[0].to_string();
    let vram_mb = parts[1].parse::<u64>().ok()?;
    let driver_version = parts.get(2).map(|s| s.to_string());

    Some(GpuInfo {
        name,
        vram_mb,
        driver_version,
        cuda_version,
    })
}

/// Parse "CUDA Version: X.Y" from the `nvidia-smi` plain-text header.
fn detect_cuda_version() -> Option<CudaVersion> {
    let output = Command::new("nvidia-smi").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(rest) = line.find("CUDA Version:").map(|i| &line[i + 13..])
            && let Some(ver_str) = rest.split_whitespace().next()
        {
            return ver_str.parse().ok();
        }
    }
    None
}
