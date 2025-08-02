use std::path::PathBuf;

use cargo::core::{
    compiler::{CompileKind, CompileMode},
    resolver::{CliFeatures, ForceAllTargets, HasDevUnits},
};

use crate::args::Args;

#[derive(Debug)]
struct ParsedCargoArgs {
    features_args: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    target_args: Vec<String>,
    additional_profile: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StaticScanConfig {
    /// Features to enable/disable
    features: CliFeatures,
    /// Target platforms to compile for (e.g., host, x86_64-unknown-linux-gnu)
    requested_kinds: Vec<CompileKind>,
    /// Compilation mode (debug, release, test, etc.)
    mode: CompileMode,
    /// Whether to include dev dependencies
    has_dev_units: HasDevUnits,
    /// Force all targets to be considered
    force_all_targets: ForceAllTargets,
    /// Optional custom profile settings
    profile_name: String,

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

        let mut i = 0;
        while i < cargo_args.len() {
            match cargo_args[i].as_str() {
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
                _ => {
                    // Handle --features=value syntax
                    if cargo_args[i].starts_with("--features=") {
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
        }
    }

    pub fn from_args(args: &Args) -> Self {
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
            .unwrap_or_else(|_| CliFeatures::new_all(false))
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
            "dev" | "debug" => CompileMode::Build,
            "release" => CompileMode::Build,
            _ => CompileMode::Build,
        };

        let has_dev_units = match effective_profile.as_str() {
            "test" | "bench" => HasDevUnits::Yes,
            _ => HasDevUnits::No,
        };

        let force_all_targets = ForceAllTargets::No;

        // Use current working dir as root. Maybe need to handle a case when running
        // in a subdir or sub-crate.
        let work_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

        Self {
            features,
            requested_kinds,
            mode,
            has_dev_units,
            force_all_targets,
            profile_name: effective_profile,
            work_dir,
        }
    }
}

#[cfg(test)]
mod tests {
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
                name: "debug profile with no args",
                profile: "debug",
                cargo_args: vec![],
                expected_profile: "debug",
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
                profile: "debug",
                cargo_args: vec!["--profile", "release"],
                expected_profile: "release",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "profile override with --profile=value syntax",
                profile: "debug",
                cargo_args: vec!["--profile=custom"],
                expected_profile: "custom",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "features with --features flag",
                profile: "debug",
                cargo_args: vec!["--features", "feature1,feature2"],
                expected_profile: "debug",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: false,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "all features enabled",
                profile: "debug",
                cargo_args: vec!["--all-features"],
                expected_profile: "debug",
                expected_mode: CompileMode::Build,
                expected_has_dev_units: HasDevUnits::No,
                expected_all_features: true,
                expected_uses_default_features: true,
            },
            TestCase {
                name: "no default features",
                profile: "debug",
                cargo_args: vec!["--no-default-features"],
                expected_profile: "debug",
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
                profile: "debug",
                cargo_args: vec!["--target", "x86_64-unknown-linux-gnu"],
                expected_profile: "debug",
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

            let config = StaticScanConfig::from_args(&args);

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
}
