use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub fixture_name: String,
    pub tool_count: usize,
    pub path_name: String,
    #[serde(with = "duration_secs")]
    pub install_duration: Duration,
    pub footprint_bytes: u64,
    #[serde(with = "duration_secs")]
    pub cold_run_duration: Duration,
    pub timestamp: DateTime<Utc>,
}

impl BenchResult {
    pub fn footprint_mb(&self) -> f64 {
        self.footprint_bytes as f64 / 1_048_576.0
    }

    pub fn install_secs(&self) -> f64 {
        self.install_duration.as_secs_f64()
    }

    pub fn cold_run_secs(&self) -> f64 {
        self.cold_run_duration.as_secs_f64()
    }
}

/// A collection of results from one benchmark run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    pub bv_version: String,
    pub results: Vec<BenchResult>,
}

impl BenchReport {
    pub fn new(results: Vec<BenchResult>) -> Self {
        Self {
            bv_version: env!("CARGO_PKG_VERSION").to_string(),
            results,
        }
    }

    pub fn print_table(&self) {
        println!(
            "{:<12} {:<15} {:>10} {:>14} {:>14} {:>12}",
            "path", "fixture", "tools", "install (s)", "footprint (MB)", "cold-run (s)"
        );
        println!("{}", "-".repeat(81));
        for r in &self.results {
            println!(
                "{:<12} {:<15} {:>10} {:>14.2} {:>14.1} {:>12.3}",
                r.path_name,
                r.fixture_name,
                r.tool_count,
                r.install_secs(),
                r.footprint_mb(),
                r.cold_run_secs(),
            );
        }
    }
}

mod duration_secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(d.as_secs_f64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(d)?;
        Ok(Duration::from_secs_f64(secs))
    }
}
