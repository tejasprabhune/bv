/// A benchmark fixture describing a set of tools to install together.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Fixture {
    pub name: String,
    pub tools: Vec<ToolPin>,
}

/// A pinned tool reference used in a fixture.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolPin {
    pub id: String,
    pub version: String,
}

impl Fixture {
    pub fn small() -> Self {
        Self {
            name: "small-1".into(),
            tools: vec![ToolPin {
                id: "samtools".into(),
                version: "1.21.0".into(),
            }],
        }
    }

    /// 5 tools all available on osx-arm64 bioconda.
    pub fn mac_medium() -> Self {
        Self {
            name: "mac-5".into(),
            tools: vec![
                ToolPin { id: "samtools".into(), version: "1.21.0".into() },
                ToolPin { id: "bcftools".into(), version: "1.21.0".into() },
                ToolPin { id: "bwa".into(), version: "0.7.18".into() },
                ToolPin { id: "fastqc".into(), version: "0.12.1".into() },
                ToolPin { id: "minimap2".into(), version: "2.28.0".into() },
            ],
        }
    }

    /// 10 tools all available on osx-arm64 bioconda.
    pub fn mac_large() -> Self {
        Self {
            name: "mac-10".into(),
            tools: vec![
                ToolPin { id: "samtools".into(), version: "1.21.0".into() },
                ToolPin { id: "bcftools".into(), version: "1.21.0".into() },
                ToolPin { id: "bwa".into(), version: "0.7.18".into() },
                ToolPin { id: "bowtie2".into(), version: "2.5.4".into() },
                ToolPin { id: "minimap2".into(), version: "2.28.0".into() },
                ToolPin { id: "fastqc".into(), version: "0.12.1".into() },
                ToolPin { id: "multiqc".into(), version: "1.25.0".into() },
                ToolPin { id: "kallisto".into(), version: "0.51.1".into() },
                ToolPin { id: "bedtools".into(), version: "2.31.1".into() },
                ToolPin { id: "fastp".into(), version: "0.23.4".into() },
            ],
        }
    }

    /// 5 tools including seqkit 2.8.1, which is Linux-only at this version.
    pub fn linux_medium() -> Self {
        Self {
            name: "linux-5".into(),
            tools: vec![
                ToolPin { id: "samtools".into(), version: "1.21.0".into() },
                ToolPin { id: "bcftools".into(), version: "1.21.0".into() },
                ToolPin { id: "bwa".into(), version: "0.7.18".into() },
                ToolPin { id: "fastqc".into(), version: "0.12.1".into() },
                ToolPin { id: "seqkit".into(), version: "2.8.1".into() },
            ],
        }
    }

    /// 20 tools, many Linux-only (blast, diamond, hmmer, mafft, seqkit 2.8.1).
    pub fn linux_large() -> Self {
        Self {
            name: "linux-20".into(),
            tools: vec![
                ToolPin { id: "samtools".into(), version: "1.21.0".into() },
                ToolPin { id: "bcftools".into(), version: "1.21.0".into() },
                ToolPin { id: "bwa".into(), version: "0.7.18".into() },
                ToolPin { id: "bowtie2".into(), version: "2.5.4".into() },
                ToolPin { id: "hmmer".into(), version: "3.3.2".into() },
                ToolPin { id: "blast".into(), version: "2.15.0".into() },
                ToolPin { id: "minimap2".into(), version: "2.28.0".into() },
                ToolPin { id: "fastqc".into(), version: "0.12.1".into() },
                ToolPin { id: "multiqc".into(), version: "1.25.0".into() },
                ToolPin { id: "diamond".into(), version: "2.1.9".into() },
                ToolPin { id: "salmon".into(), version: "1.10.3".into() },
                ToolPin { id: "kallisto".into(), version: "0.51.1".into() },
                ToolPin { id: "bedtools".into(), version: "2.31.1".into() },
                ToolPin { id: "seqkit".into(), version: "2.8.1".into() },
                ToolPin { id: "mafft".into(), version: "7.525.0".into() },
                ToolPin { id: "muscle".into(), version: "5.1.0".into() },
                ToolPin { id: "trimmomatic".into(), version: "0.39.0".into() },
                ToolPin { id: "hisat2".into(), version: "2.2.1".into() },
                ToolPin { id: "fastp".into(), version: "0.23.4".into() },
                ToolPin { id: "spades".into(), version: "4.0.0".into() },
            ],
        }
    }

    /// All tools are available on osx-arm64 bioconda.
    pub fn mac_suite() -> Vec<Self> {
        vec![Self::small(), Self::mac_medium(), Self::mac_large()]
    }

    /// Includes Linux-only tools; mamba/pixi/conda will fail some fixtures on macOS.
    pub fn linux_suite() -> Vec<Self> {
        vec![Self::small(), Self::linux_medium(), Self::linux_large()]
    }
}
