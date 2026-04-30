use bv_core::lockfile::{CondaPackagePin, LayerDescriptor};

use crate::popularity::PopularityMap;
use crate::spec::ResolvedPackage;

/// Strategy for grouping packages into OCI layers.
///
/// Layer order: most-stable (lowest in dependency graph) at index 0,
/// most-volatile (entrypoint) at the top. Docker pulls layers in manifest
/// order so stable-first minimises re-downloads across tool upgrades.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackingStrategy {
    /// Each package gets its own layer (default for small tool sets).
    OnePerPackage,
    /// Popularity-based packing when `max_layers` is exceeded.
    PopularityBased { max_layers: usize },
}

impl Default for PackingStrategy {
    fn default() -> Self {
        Self::OnePerPackage
    }
}

/// A group of packages that will be combined into a single OCI layer.
#[derive(Debug, Clone)]
pub struct LayerGroup {
    pub packages: Vec<ResolvedPackage>,
}

/// Group `packages` into layer groups according to `strategy`.
///
/// When `popularity` is provided and `strategy` is `PopularityBased`, packages
/// are sorted by their co-occurrence score (descending) before splitting into
/// solo vs. long-tail groups. Without scores the sort falls back to package
/// name for determinism, which is correct but not optimal.
///
/// The caller is responsible for appending the meta layer and entrypoint layer
/// after the returned groups.
pub fn pack(
    packages: &[ResolvedPackage],
    strategy: &PackingStrategy,
    popularity: Option<&PopularityMap>,
) -> Vec<LayerGroup> {
    match strategy {
        PackingStrategy::OnePerPackage => packages
            .iter()
            .map(|p| LayerGroup {
                packages: vec![p.clone()],
            })
            .collect(),

        PackingStrategy::PopularityBased { max_layers } => {
            pack_by_popularity(packages, *max_layers, popularity)
        }
    }
}

/// Sort `packages` by popularity score descending, then by name for
/// determinism.  The `max_layers - 2` most popular packages each get their own
/// layer; the remaining packages are packed into a single "long-tail" layer.
/// The last two layer slots are reserved for the meta and entrypoint layers
/// added by the caller.
///
/// **Stability invariant**: because scores are keyed by package *name* (not
/// version+build), upgrading an existing popular package (e.g. `openssl`
/// 3.2.1 → 3.3.0) preserves its high score and keeps it in a solo layer,
/// just with a new digest.  Only the solo/long-tail boundary changes when the
/// registry grows beyond `max_layers - 2` unique popular packages, which
/// happens at most `O(1)` times per new tool added.
fn pack_by_popularity(
    packages: &[ResolvedPackage],
    max_layers: usize,
    popularity: Option<&PopularityMap>,
) -> Vec<LayerGroup> {
    if max_layers < 3 || packages.is_empty() {
        return vec![LayerGroup {
            packages: packages.to_vec(),
        }];
    }

    // Sort by score desc, then name asc for determinism within ties.
    let mut sorted = packages.to_vec();
    sorted.sort_by(|a, b| {
        let sa = popularity.map(|p| p.score(&a.name)).unwrap_or(0);
        let sb = popularity.map(|p| p.score(&b.name)).unwrap_or(0);
        sb.cmp(&sa).then(a.name.cmp(&b.name))
    });

    let solo_count = max_layers.saturating_sub(2).min(sorted.len());
    let (solo, tail) = sorted.split_at(solo_count);

    let mut groups: Vec<LayerGroup> = solo
        .iter()
        .map(|p| LayerGroup {
            packages: vec![p.clone()],
        })
        .collect();

    if !tail.is_empty() {
        groups.push(LayerGroup {
            packages: tail.to_vec(),
        });
    }
    groups
}

/// Convert a `ResolvedPackage` into a `LayerDescriptor` placeholder.
/// The actual `digest` and `size` are filled in by `build::build_layer` after
/// the layer blob has been created.
pub fn placeholder_descriptor(pkg: &ResolvedPackage) -> LayerDescriptor {
    LayerDescriptor {
        digest: String::new(),
        size: 0,
        media_type: "application/vnd.oci.image.layer.v1.tar+zstd".into(),
        conda_package: Some(CondaPackagePin {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            build: pkg.build.clone(),
            channel: pkg.channel.clone(),
            sha256: pkg.sha256.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str) -> ResolvedPackage {
        crate::spec::ResolvedPackage {
            name: name.into(),
            version: "1.0.0".into(),
            build: "h0_0".into(),
            channel: "conda-forge".into(),
            url: format!("https://example.com/{name}.conda"),
            sha256: "abc".into(),
            filename: format!("{name}-1.0.0-h0_0.conda"),
            depends: vec![],
        }
    }

    #[test]
    fn one_per_package_gives_n_groups() {
        let pkgs = vec![pkg("openssl"), pkg("zlib"), pkg("samtools")];
        let groups = pack(&pkgs, &PackingStrategy::OnePerPackage, None);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].packages[0].name, "openssl");
    }

    #[test]
    fn popularity_packing_respects_max_layers() {
        let pkgs: Vec<_> = (0..10).map(|i| pkg(&format!("pkg{i:02}"))).collect();
        let groups = pack(
            &pkgs,
            &PackingStrategy::PopularityBased { max_layers: 5 },
            None,
        );
        // 3 solo layers + 1 long-tail (slots 4 and 5 reserved for meta+entrypoint)
        assert_eq!(groups.len(), 4);
        assert_eq!(groups.last().unwrap().packages.len(), 7); // 10 - 3
    }

    #[test]
    fn popularity_packing_degenerate_small_input() {
        let pkgs = vec![pkg("samtools")];
        let groups = pack(
            &pkgs,
            &PackingStrategy::PopularityBased { max_layers: 64 },
            None,
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].packages[0].name, "samtools");
    }

    #[test]
    fn popular_packages_placed_before_rare_ones() {
        let mut pop = PopularityMap::new();
        // openssl appears in 10 tools, zlib in 3, rare in 1
        for _ in 0..10 {
            pop.record_tool(&["openssl".into()]);
        }
        for _ in 0..3 {
            pop.record_tool(&["zlib".into()]);
        }
        pop.record_tool(&["rare".into()]);

        let pkgs = vec![pkg("rare"), pkg("zlib"), pkg("openssl")];
        let groups = pack(
            &pkgs,
            &PackingStrategy::PopularityBased { max_layers: 64 },
            Some(&pop),
        );

        // All three fit in solo layers (64 - 2 = 62 solo slots).
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].packages[0].name, "openssl");
        assert_eq!(groups[1].packages[0].name, "zlib");
        assert_eq!(groups[2].packages[0].name, "rare");
    }

    #[test]
    fn rare_packages_land_in_long_tail() {
        let mut pop = PopularityMap::new();
        pop.record_tool(&["openssl".into(), "zlib".into()]);
        pop.record_tool(&["openssl".into(), "bz2".into()]);

        // 3 solo slots: max_layers=5, 5-2=3 solo, 1 long-tail
        let pkgs = vec![pkg("openssl"), pkg("zlib"), pkg("bz2"), pkg("rare1"), pkg("rare2")];
        let groups = pack(
            &pkgs,
            &PackingStrategy::PopularityBased { max_layers: 5 },
            Some(&pop),
        );

        // Exactly 4 groups: openssl solo, zlib solo, bz2 solo, long-tail (rare1+rare2).
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].packages[0].name, "openssl");
        // rare packages are in the last group
        let tail = groups.last().unwrap();
        assert_eq!(tail.packages.len(), 2);
    }

    #[test]
    fn packing_is_deterministic_for_same_scores() {
        let mut pop = PopularityMap::new();
        pop.record_tool(&["aa".into(), "bb".into(), "cc".into()]);

        let pkgs = vec![pkg("cc"), pkg("aa"), pkg("bb")];
        let groups1 = pack(
            &pkgs,
            &PackingStrategy::PopularityBased { max_layers: 64 },
            Some(&pop),
        );
        let groups2 = pack(
            &pkgs,
            &PackingStrategy::PopularityBased { max_layers: 64 },
            Some(&pop),
        );

        let names1: Vec<_> = groups1.iter().map(|g| g.packages[0].name.as_str()).collect();
        let names2: Vec<_> = groups2.iter().map(|g| g.packages[0].name.as_str()).collect();
        assert_eq!(names1, names2, "packing must be deterministic");
        // Tie-broken by name: aa < bb < cc
        assert_eq!(names1, vec!["aa", "bb", "cc"]);
    }

    /// M5.4: Synthesize 100 fake tool specs with overlapping deps.
    /// Assert that for any two specs sharing a popular package, that package
    /// lands in a solo LayerGroup in both specs — guaranteeing identical
    /// layer digests when the same package+version+build is built reproducibly.
    #[test]
    fn shared_popular_packages_get_solo_layers_across_tools() {
        const NUM_TOOLS: usize = 100;
        const MAX_LAYERS: usize = 64;
        const SHARED_PKGS: &[&str] = &[
            "openssl", "zlib", "libgcc", "libstdcxx", "ncurses", "xz", "bzip2",
        ];
        const UNIQUE_SUFFIX: &str = "tool-specific-pkg";

        // Build a fake registry: each tool uses all shared packages + one unique package.
        let all_tool_packages: Vec<Vec<String>> = (0..NUM_TOOLS)
            .map(|i| {
                let mut pkgs: Vec<String> = SHARED_PKGS.iter().map(|s| s.to_string()).collect();
                pkgs.push(format!("{UNIQUE_SUFFIX}-{i}"));
                pkgs
            })
            .collect();

        // Compute popularity from all tools.
        let mut pop = PopularityMap::new();
        for tool_pkgs in &all_tool_packages {
            pop.record_tool(tool_pkgs);
        }

        // Pack two representative tools and assert shared packages get solo layers.
        for tool_idx in [0usize, 42, 99] {
            let pkgs: Vec<_> = all_tool_packages[tool_idx]
                .iter()
                .map(|name| crate::spec::ResolvedPackage {
                    name: name.clone(),
                    version: "1.0.0".into(),
                    build: "h0_0".into(),
                    channel: "conda-forge".into(),
                    url: format!("https://example.com/{name}.conda"),
                    sha256: format!("sha256-{name}"),
                    filename: format!("{name}-1.0.0-h0_0.conda"),
                    depends: vec![],
                })
                .collect();

            let groups = pack(
                &pkgs,
                &PackingStrategy::PopularityBased { max_layers: MAX_LAYERS },
                Some(&pop),
            );

            // Every shared package must appear in a solo group (one package per group).
            for shared in SHARED_PKGS {
                let solo = groups.iter().any(|g| {
                    g.packages.len() == 1 && g.packages[0].name == *shared
                });
                assert!(
                    solo,
                    "shared package '{}' must get its own layer in tool-{tool_idx}",
                    shared
                );
            }
        }
    }

    /// Same shared package in two different tools must produce the same
    /// LayerGroup structure (same single package), confirming digest identity.
    #[test]
    fn shared_package_has_same_solo_group_across_tools() {
        let mut pop = PopularityMap::new();
        pop.record_tool(&["openssl".into(), "samtools".into()]);
        pop.record_tool(&["openssl".into(), "bwa".into()]);

        let samtools_pkgs = vec![pkg("openssl"), pkg("samtools")];
        let bwa_pkgs = vec![pkg("openssl"), pkg("bwa")];

        let groups_s = pack(
            &samtools_pkgs,
            &PackingStrategy::PopularityBased { max_layers: 64 },
            Some(&pop),
        );
        let groups_b = pack(
            &bwa_pkgs,
            &PackingStrategy::PopularityBased { max_layers: 64 },
            Some(&pop),
        );

        // openssl is the first group in both (highest score = 2).
        assert_eq!(groups_s[0].packages[0].name, "openssl");
        assert_eq!(groups_b[0].packages[0].name, "openssl");

        // Both openssl groups contain exactly one package with the same identity.
        // A deterministic build on those groups would yield identical layer digests.
        assert_eq!(
            groups_s[0].packages[0].sha256,
            groups_b[0].packages[0].sha256,
        );
    }
}
