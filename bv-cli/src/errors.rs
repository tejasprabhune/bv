use owo_colors::{OwoColorize, Stream};

use bv_core::hardware::HardwareMismatch;

/// Print a structured hardware-mismatch diagnostic for one tool.
///
/// ```text
/// error: cannot add alphafold@2.3.2
///   requires an NVIDIA GPU with >= 8 GB VRAM (none detected)
///
/// help:  on macOS, GPU tools can be installed but not run locally
///   use `bv add --ignore-hardware` to bypass this check
/// ```
pub fn print_hardware_mismatch(tool_id: &str, version: &str, mismatches: &[HardwareMismatch]) {
    eprintln!();
    eprintln!(
        "{} cannot add {}@{}",
        "error:".if_supports_color(Stream::Stderr, |t| t.red().bold().to_string()),
        tool_id.if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
        version,
    );
    for m in mismatches {
        eprintln!("  {m}");
    }

    let has_gpu_mismatch = mismatches
        .iter()
        .any(|m| matches!(m, HardwareMismatch::NoGpu | HardwareMismatch::NoCuda { .. }));

    eprintln!();
    if has_gpu_mismatch && cfg!(target_os = "macos") {
        eprintln!(
            "{} on macOS, GPU tools can be installed but cannot run locally",
            "help: ".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string()),
        );
    } else {
        eprintln!(
            "{} hardware requirements not met",
            "help: ".if_supports_color(Stream::Stderr, |t| t.yellow().bold().to_string()),
        );
    }
    eprintln!(
        "  use `{}` to bypass this check",
        "bv add --ignore-hardware".if_supports_color(Stream::Stderr, |t| t.bold().to_string()),
    );
}
