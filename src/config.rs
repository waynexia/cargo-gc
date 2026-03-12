use std::path::PathBuf;

use anyhow::{Context, Result};
use cargo::core::compiler::{CompileKind, UserIntent};
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
}

#[derive(Debug, Clone)]
pub struct ScanSpec {
    pub requested_profile: String,
    pub intent: UserIntent,
    pub has_dev_units: HasDevUnits,
    pub force_all_targets: ForceAllTargets,
    pub packages: Packages,
    pub filter: CompileFilter,
}

impl ScanSpec {
    fn new(requested_profile: &str, intent: UserIntent) -> Self {
        let filter = CompileFilter::new_all_targets();
        let has_dev_units = if filter.need_dev_deps(intent) {
            HasDevUnits::Yes
        } else {
            HasDevUnits::No
        };

        Self {
            requested_profile: requested_profile.to_string(),
            intent,
            has_dev_units,
            force_all_targets: ForceAllTargets::Yes,
            packages: Packages::Default,
            filter,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StaticScanConfig {
    /// Features to enable/disable
    pub features: CliFeatures,
    /// Target platforms to compile for (e.g., host, x86_64-unknown-linux-gnu)
    pub requested_kinds: Vec<CompileKind>,
    /// Exact scan requests to union for the selected profile/feature set
    pub scan_specs: Vec<ScanSpec>,
    /// The profile selected for GC
    pub profile_name: String,

    /// Working directory for current command run
    work_dir: PathBuf,
}

impl StaticScanConfig {
    /// Parse cargo_args to extract relevant flags and configuration.
    ///
    /// GC only keys off feature selection and profile. Other forwarded build
    /// shape flags are intentionally ignored because cleanup should preserve
    /// all target kinds that may share the same profile directory.
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
                    if cargo_args[i].starts_with("--features=") {
                        if let Some(feature_list) = cargo_args[i].strip_prefix("--features=") {
                            features_args.push(feature_list.to_string());
                        }
                    } else if cargo_args[i].starts_with("--target=") {
                        if let Some(target) = cargo_args[i].strip_prefix("--target=") {
                            target_args.push(target.to_string());
                        }
                    } else if cargo_args[i].starts_with("--profile=")
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

    pub fn from_args(args: &Args) -> Result<Self> {
        let parsed = Self::parse_cargo_args(&args.cargo_args);

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

        let scan_specs = Self::build_scan_specs(&effective_profile);

        let work_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

        Ok(Self {
            features,
            requested_kinds,
            scan_specs,
            profile_name: effective_profile,
            work_dir,
        })
    }

    fn build_scan_specs(profile_name: &str) -> Vec<ScanSpec> {
        let mut scan_specs = vec![
            ScanSpec::new(profile_name, UserIntent::Build),
            ScanSpec::new(profile_name, UserIntent::Check { test: false }),
        ];

        if profile_name == "dev" {
            scan_specs.push(ScanSpec::new("test", UserIntent::Test));
        }

        scan_specs
    }

    pub fn get_manifest_path(&self) -> PathBuf {
        self.work_dir.join("Cargo.toml")
    }
}

#[cfg(test)]
mod tests {
    use cargo::core::compiler::UserIntent;

    use super::*;

    #[test]
    fn test_static_scan_config_from_args() {
        let args = Args {
            profile: "dev".to_string(),
            verbose: false,
            dry_run: false,
            cargo_args: vec![
                "--features".to_string(),
                "feature1,feature2".to_string(),
                "--bin".to_string(),
                "ignored-bin".to_string(),
            ],
        };

        let config = StaticScanConfig::from_args(&args).expect("config should parse");

        assert_eq!(config.profile_name, "dev");
        assert_eq!(config.requested_kinds, vec![CompileKind::Host]);
        assert_eq!(config.scan_specs.len(), 3);

        assert!(matches!(config.scan_specs[0].intent, UserIntent::Build));
        assert_eq!(config.scan_specs[0].requested_profile, "dev");
        assert!(matches!(
            config.scan_specs[1].intent,
            UserIntent::Check { test: false }
        ));
        assert_eq!(config.scan_specs[1].requested_profile, "dev");
        assert!(matches!(config.scan_specs[2].intent, UserIntent::Test));
        assert_eq!(config.scan_specs[2].requested_profile, "test");

        for spec in &config.scan_specs {
            assert!(matches!(spec.force_all_targets, ForceAllTargets::Yes));
            assert!(matches!(
                spec.filter,
                CompileFilter::Only {
                    all_targets: true,
                    ..
                }
            ));
        }
    }

    #[test]
    fn test_release_profile_only_scans_build_and_check() {
        let args = Args {
            profile: "release".to_string(),
            verbose: false,
            dry_run: false,
            cargo_args: vec![],
        };

        let config = StaticScanConfig::from_args(&args).expect("config should parse");

        assert_eq!(config.profile_name, "release");
        assert_eq!(config.scan_specs.len(), 2);
        assert!(matches!(config.scan_specs[0].intent, UserIntent::Build));
        assert!(matches!(
            config.scan_specs[1].intent,
            UserIntent::Check { test: false }
        ));
    }

    #[test]
    fn test_profile_override_with_forwarded_profile_flag() {
        let args = Args {
            profile: "dev".to_string(),
            verbose: false,
            dry_run: false,
            cargo_args: vec!["--profile".to_string(), "release".to_string()],
        };

        let config = StaticScanConfig::from_args(&args).expect("config should parse");

        assert_eq!(config.profile_name, "release");
        assert_eq!(config.scan_specs.len(), 2);
        assert_eq!(config.scan_specs[0].requested_profile, "release");
    }

    #[test]
    fn test_all_features_enabled() {
        let args = Args {
            profile: "dev".to_string(),
            verbose: false,
            dry_run: false,
            cargo_args: vec!["--all-features".to_string()],
        };

        let config = StaticScanConfig::from_args(&args).expect("config should parse");

        assert!(config.features.all_features);
    }

    #[test]
    fn test_no_default_features() {
        let args = Args {
            profile: "dev".to_string(),
            verbose: false,
            dry_run: false,
            cargo_args: vec!["--no-default-features".to_string()],
        };

        let config = StaticScanConfig::from_args(&args).expect("config should parse");

        assert!(!config.features.uses_default_features);
    }
}
