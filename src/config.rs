use std::path::PathBuf;

use anyhow::{Context, Result};
use cargo::core::compiler::{CompileKind, CompileMode};
use cargo::core::resolver::{CliFeatures, ForceAllTargets, HasDevUnits};
use cargo::ops::{CompileFilter, Packages};

use crate::args::Args;

#[derive(Debug)]
struct ParsedCargoArgs {
    features_args: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    target_args: Vec<String>,
    additional_profile: Option<String>,

    packages: Vec<String>,
    workspace: bool,
    excludes: Vec<String>,
    lib_only: bool,
    bins: Vec<String>,
    all_bins: bool,
    examples: Vec<String>,
    all_examples: bool,
    tests: Vec<String>,
    all_tests: bool,
    benches: Vec<String>,
    all_benches: bool,
    all_targets: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StaticScanConfig {
    /// Features to enable/disable
    pub features: CliFeatures,
    /// Target platforms to compile for (e.g., host, x86_64-unknown-linux-gnu)
    pub requested_kinds: Vec<CompileKind>,
    /// Compilation mode (debug, release, test, etc.)
    pub mode: CompileMode,
    /// Whether to include dev dependencies
    pub has_dev_units: HasDevUnits,
    /// Force all targets to be considered
    pub force_all_targets: ForceAllTargets,
    /// Package selection to mirror cargo build
    pub packages: Packages,
    /// Target filter to mirror cargo build
    pub filter: CompileFilter,
    /// Optional custom profile settings
    pub profile_name: String,

    /// Working directory for current command run
    work_dir: PathBuf,
}

impl StaticScanConfig {
    /// Parse cargo_args to extract relevant flags and configuration
    ///
    /// todo: Consider reuse cargo build's parse logic
    fn parse_cargo_args(cargo_args: &[String]) -> ParsedCargoArgs {
        let mut features_args = Vec::new();
        let mut all_features = false;
        let mut no_default_features = false;
        let mut target_args = Vec::new();
        let mut additional_profile = None;
        let mut packages = Vec::new();
        let mut workspace = false;
        let mut excludes = Vec::new();
        let mut lib_only = false;
        let mut bins = Vec::new();
        let mut all_bins = false;
        let mut examples = Vec::new();
        let mut all_examples = false;
        let mut tests = Vec::new();
        let mut all_tests = false;
        let mut benches = Vec::new();
        let mut all_benches = false;
        let mut all_targets = false;

        let mut i = 0;
        while i < cargo_args.len() {
            match cargo_args[i].as_str() {
                "-p" | "--package" => {
                    if i + 1 < cargo_args.len() {
                        packages.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--workspace" => {
                    workspace = true;
                    i += 1;
                }
                "--exclude" => {
                    if i + 1 < cargo_args.len() {
                        excludes.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--features" => {
                    if i + 1 < cargo_args.len() {
                        features_args.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--all-features" => {
                    all_features = true;
                    i += 1;
                }
                "--no-default-features" => {
                    no_default_features = true;
                    i += 1;
                }
                "--target" => {
                    if i + 1 < cargo_args.len() {
                        target_args.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--profile" => {
                    if i + 1 < cargo_args.len() {
                        additional_profile = Some(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--lib" => {
                    lib_only = true;
                    i += 1;
                }
                "--bin" => {
                    if i + 1 < cargo_args.len() {
                        bins.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--bins" => {
                    all_bins = true;
                    i += 1;
                }
                "--example" => {
                    if i + 1 < cargo_args.len() {
                        examples.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--examples" => {
                    all_examples = true;
                    i += 1;
                }
                "--test" => {
                    if i + 1 < cargo_args.len() {
                        tests.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--tests" => {
                    all_tests = true;
                    i += 1;
                }
                "--bench" => {
                    if i + 1 < cargo_args.len() {
                        benches.push(cargo_args[i + 1].clone());
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                "--benches" => {
                    all_benches = true;
                    i += 1;
                }
                "--all-targets" => {
                    all_targets = true;
                    i += 1;
                }
                _ => {
                    if let Some(package) = cargo_args[i].strip_prefix("--package=") {
                        packages.push(package.to_string());
                    } else if let Some(package) = cargo_args[i].strip_prefix("-p=") {
                        packages.push(package.to_string());
                    }
                    // Handle --features=value syntax
                    else if cargo_args[i].starts_with("--features=") {
                        if let Some(feature_list) = cargo_args[i].strip_prefix("--features=") {
                            features_args.push(feature_list.to_string());
                        }
                    }
                    // Handle --target=value syntax
                    else if cargo_args[i].starts_with("--target=") {
                        if let Some(target) = cargo_args[i].strip_prefix("--target=") {
                            target_args.push(target.to_string());
                        }
                    }
                    // Handle --profile=value syntax
                    else if cargo_args[i].starts_with("--profile=")
                        && let Some(profile) = cargo_args[i].strip_prefix("--profile=")
                    {
                        additional_profile = Some(profile.to_string());
                    } else if let Some(exclude) = cargo_args[i].strip_prefix("--exclude=") {
                        excludes.push(exclude.to_string());
                    } else if let Some(bin) = cargo_args[i].strip_prefix("--bin=") {
                        bins.push(bin.to_string());
                    } else if let Some(example) = cargo_args[i].strip_prefix("--example=") {
                        examples.push(example.to_string());
                    } else if let Some(test) = cargo_args[i].strip_prefix("--test=") {
                        tests.push(test.to_string());
                    } else if let Some(bench) = cargo_args[i].strip_prefix("--bench=") {
                        benches.push(bench.to_string());
                    }
                    i += 1;
                }
            }
        }

        ParsedCargoArgs {
            features_args,
            all_features,
            no_default_features,
            target_args,
            additional_profile,
            packages,
            workspace,
            excludes,
            lib_only,
            bins,
            all_bins,
            examples,
            all_examples,
            tests,
            all_tests,
            benches,
            all_benches,
            all_targets,
        }
    }

    pub fn from_args(args: &Args) -> Result<Self> {
        let parsed = Self::parse_cargo_args(&args.cargo_args);

        // Determine the final profile to use
        let effective_profile = parsed
            .additional_profile
            .unwrap_or_else(|| args.profile.clone());

        let features = if parsed.all_features {
            CliFeatures::new_all(true)
        } else {
            CliFeatures::from_command_line(
                &parsed.features_args,
                false,
                !parsed.no_default_features,
            )
            .context("invalid feature selection in forwarded cargo args")?
        };

        let requested_kinds = if parsed.target_args.is_empty() {
            vec![CompileKind::Host]
        } else {
            // todo: Handle target parsing properly
            vec![CompileKind::Host]
        };

        let mode = match effective_profile.as_str() {
            "test" => CompileMode::Test,
            "bench" => CompileMode::Test,
            "dev" => CompileMode::Build,
            "release" => CompileMode::Build,
            _ => CompileMode::Build,
        };

        let has_dev_units = match effective_profile.as_str() {
            "test" | "bench" => HasDevUnits::Yes,
            _ => HasDevUnits::No,
        };

        let force_all_targets = ForceAllTargets::No;
        let packages = Packages::from_flags(parsed.workspace, parsed.excludes, parsed.packages)
            .context("invalid package selection in forwarded cargo args")?;
        let filter = CompileFilter::from_raw_arguments(
            parsed.lib_only,
            parsed.bins,
            parsed.all_bins,
            parsed.tests,
            parsed.all_tests,
            parsed.examples,
            parsed.all_examples,
            parsed.benches,
            parsed.all_benches,
            parsed.all_targets,
        );
        let has_dev_units = if matches!(has_dev_units, HasDevUnits::Yes)
            || filter.need_dev_deps(cargo::core::compiler::UserIntent::Build)
        {
            HasDevUnits::Yes
        } else {
            HasDevUnits::No
        };

        // Use current working dir as root. Maybe need to handle a case when running
        // in a subdir or sub-crate.
        let work_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

        Ok(Self {
            features,
            requested_kinds,
            mode,
            has_dev_units,
            force_all_targets,
            packages,
            filter,
            profile_name: effective_profile,
            work_dir,
        })
    }

    pub fn get_manifest_path(&self) -> PathBuf {
        self.work_dir.join("Cargo.toml")
    }
}

#[cfg(test)]
mod tests {
    use cargo::ops::{CompileFilter, FilterRule};

    use super::*;

    #[derive(Debug)]
    struct TestCase {
        name: &'static str,
        profile: &'static str,
        cargo_args: Vec<&'static str>,
        expected_profile: &'static str,
        expected_mode: CompileMode,
        expected_has_dev_units: HasDevUnits,
        expected_all_features: bool,
        expected_uses_default_features: bool,
    }

    #[test]
    fn test_static_scan_config_from_args() {
        let test_cases = vec![
            TestCase {
                name: "dev profile with no args",
                profile: "dev",
                cargo_args: vec![],
                expected_profile: "dev",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "test profile",
                profile: "test",
                cargo_args: vec![],
                expected_profile: "test",
                expected_mode: CompileMode::Test,
                expected_has_dev_units: HasDevUnits::Yes,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "release profile",
                profile: "release",
                cargo_args: vec!["--verbose"],
                expected_profile: "release",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "profile override with --profile flag",
                profile: "dev",
                cargo_args: vec!["--profile", "release"],
                expected_profile: "release",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "profile override with --profile=value syntax",
                profile: "dev",
                cargo_args: vec!["--profile=custom"],
                expected_profile: "custom",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "features with --features flag",
                profile: "dev",
                cargo_args: vec!["--features", "feature1,feature2"],
                expected_profile: "dev",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "all features enabled",
                profile: "dev",
                cargo_args: vec!["--all-features"],
                expected_profile: "dev",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: true,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "no default features",
                profile: "dev",
                cargo_args: vec!["--no-default-features"],
                expected_profile: "dev",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: false,
            },
            TestCase {
                name: "bench profile",
                profile: "bench",
                cargo_args: vec![],
                expected_profile: "bench",
                expected_mode: CompileMode::Test,
                expected_has_dev_units: HasDevUnits::Yes,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "with target specification",
                profile: "dev",
                cargo_args: vec!["--target", "x86_64-unknown-linux-gnu"],
                expected_profile: "dev",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
        ];

        for test_case in test_cases {
            let args = Args {
                profile: test_case.profile.to_string(),
                verbose: false,
                dry_run: false,
                cargo_args: test_case.cargo_args.iter().map(|s| s.to_string()).collect(),
            };

            let config = StaticScanConfig::from_args(&args).expect("config should parse");

            // Assert all expected values
            assert_eq!(
                config.profile_name, test_case.expected_profile,
                "Test case '{}': profile_name mismatch",
                test_case.name
            );
            assert_eq!(
                config.mode, test_case.expected_mode,
                "Test case '{}': mode mismatch",
                test_case.name
            );
            assert_eq!(
                config.has_dev_units, test_case.expected_has_dev_units,
                "Test case '{}': has_dev_units mismatch",
                test_case.name
            );
            assert_eq!(
                config.features.all_features, test_case.expected_all_features,
                "Test case '{}': all_features mismatch",
                test_case.name
            );
            assert_eq!(
                config.features.uses_default_features, test_case.expected_uses_default_features,
                "Test case '{}': uses_default_features mismatch",
                test_case.name
            );

            // Common assertions that should be true for all test cases
            assert_eq!(
                config.force_all_targets,
                ForceAllTargets::No,
                "Test case '{}': force_all_targets should always be No",
                test_case.name
            );
            assert_eq!(
                config.requested_kinds.len(),
                1,
                "Test case '{}': should have exactly one requested_kind",
                test_case.name
            );
        }
    }

    #[test]
    fn test_static_scan_config_parses_bin_filter() {
        let args = Args {
            profile: "dev".to_string(),
            verbose: false,
            dry_run: false,
            cargo_args: vec!["--bin".to_string(), "greptime".to_string()],
        };

        let config = StaticScanConfig::from_args(&args).expect("config should parse");

        match config.filter {
            CompileFilter::Only {
                all_targets,
                lib,
                bins,
                examples,
                tests,
                benches,
            } => {
                assert!(!all_targets);
                assert_eq!(format!("{lib:?}"), "False");
                assert!(matches!(bins, FilterRule::Just(names) if names == vec!["greptime"]));
                assert!(matches!(examples, FilterRule::Just(names) if names.is_empty()));
                assert!(matches!(tests, FilterRule::Just(names) if names.is_empty()));
                assert!(matches!(benches, FilterRule::Just(names) if names.is_empty()));
            }
            other => panic!("expected specific compile filter, got {other:?}"),
        }
    }

    #[test]
    fn test_static_scan_config_rejects_invalid_package_selection() {
        let args = Args {
            profile: "dev".to_string(),
            verbose: false,
            dry_run: false,
            cargo_args: vec!["--exclude".to_string(), "foo".to_string()],
        };

        let err = StaticScanConfig::from_args(&args)
            .expect_err("config should reject --exclude without --workspace");
        assert!(
            err.to_string()
                .contains("invalid package selection in forwarded cargo args")
        );
    }
}
