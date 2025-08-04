use std::rc::Rc;
use std::sync::Arc;

use cargo::core::Workspace;
use cargo::core::compiler::fingerprint::{Fingerprint, calculate, compare_old_fingerprint};
use cargo::core::compiler::{
    self, BuildConfig, BuildRunner, MessageFormat, RustcTargetData, Unit, UnitInterner, UserIntent,
};
use cargo::core::profiles::Profiles;
use cargo::ops::{CompileFilter, CompileOptions, Packages, create_bcx, resolve_ws_with_opts};
use cargo::util::interning::InternedString;
use cargo::{CargoResult, GlobalContext};

use crate::beatrice::{Beatrice, UnitFreshness};
use crate::config::StaticScanConfig;

pub struct Scanner {
    config: StaticScanConfig,
    gctx: GlobalContext,
}

impl Scanner {
    pub fn try_new(config: StaticScanConfig) -> CargoResult<Self> {
        Ok(Self {
            config,
            gctx: GlobalContext::default()?,
        })
    }

    pub fn scan(&self, betty: &mut Beatrice, show_result: bool) -> CargoResult<()> {
        // todo: get the manifest path using cargo utils
        let manifest_path = self.config.get_manifest_path();

        let workspace = Workspace::new(&manifest_path, &self.gctx)?;
        let mut target_data = RustcTargetData::new(&workspace, &self.config.requested_kinds)?;
        let pkg_specs = Packages::All(vec![]).to_package_id_specs(&workspace)?;

        let _workspace_resolve = resolve_ws_with_opts(
            &workspace,
            &mut target_data,
            &self.config.requested_kinds,
            &self.config.features,
            &pkg_specs,
            self.config.has_dev_units,
            self.config.force_all_targets,
            false,
        )?;

        // check if profile exists
        let _ = Profiles::new(&workspace, InternedString::new(&self.config.profile_name))?;

        let build_config = BuildConfig {
            requested_kinds: self.config.requested_kinds.clone(),
            jobs: 1,
            keep_going: false,
            requested_profile: InternedString::new(&self.config.profile_name),
            intent: UserIntent::Build,
            message_format: MessageFormat::Human,
            force_rebuild: false,
            build_plan: false,
            unit_graph: false,
            dry_run: false,
            primary_unit_rustc: None,
            rustfix_diagnostic_server: Rc::new(std::cell::RefCell::new(None)),
            export_dir: None,
            future_incompat_report: false,
            timing_outputs: Vec::new(),
            sbom: false,
            compile_time_deps_only: false,
        };
        let compile_options = CompileOptions {
            build_config: build_config.clone(),
            cli_features: self.config.features.clone(),
            // spec: crate::ops::Packages::All(Vec::new()),
            spec: Packages::Default,
            filter: CompileFilter::Default {
                required_features_filterable: true,
            },
            target_rustdoc_args: None,
            target_rustc_args: None,
            target_rustc_crate_types: None,
            rustdoc_document_private_items: false,
            honor_rust_version: None,
        };
        let interner = UnitInterner::new();
        let build_ctx = create_bcx(&workspace, &compile_options, &interner)?;

        let num_total_units = build_ctx.unit_graph.len();
        println!("Found {num_total_units} units in the workspace");

        let mut build_runner = BuildRunner::new(&build_ctx)?;
        build_runner.lto = compiler::lto::generate(&build_ctx)?;
        build_runner.prepare_units()?;
        build_runner.prepare()?;
        compiler::custom_build::build_map(&mut build_runner)?;

        // skip clear memorized fingerprints

        let mut fresh_count = 0;
        let mut dirty_count = 0;
        for unit in build_ctx.unit_graph.keys() {
            if !build_runner.compiled.insert(unit.clone()) {
                // already processed
                continue;
            }

            let fingerprint = calculate(&mut build_runner, unit)?;
            let freshness = self.check_unit_freshness(&mut build_runner, unit, &fingerprint)?;

            // Extract package name and hash for updating Beatrice
            let package_name = unit.pkg.name().to_string();
            let fingerprint_hash = &freshness.current_fingerprint_hash;

            // Update Beatrice with freshness information
            let unit_freshness = if freshness.is_fresh {
                UnitFreshness::Fresh
            } else {
                UnitFreshness::Dirty(
                    freshness
                        .dirty_reason
                        .clone()
                        .unwrap_or_else(|| "Unknown reason".to_string()),
                )
            };
            betty.update_fingerprint_freshness(&package_name, fingerprint_hash, unit_freshness);

            if freshness.is_fresh {
                if show_result {
                    println!(
                        "✅ Unit {} is fresh, fingerprint hash: {}, path: {}",
                        unit.pkg.package_id(),
                        freshness.current_fingerprint_hash,
                        freshness.fingerprint_path
                    );
                }
                fresh_count += 1;
            } else {
                if show_result {
                    println!(
                        "❌ Unit {} is dirty: {:?}, fingerprint hash: {}, path: {}",
                        unit.pkg.package_id(),
                        freshness.dirty_reason,
                        freshness.current_fingerprint_hash,
                        freshness.fingerprint_path
                    );
                }
                dirty_count += 1;
            }
        }
        println!("Total fresh units: {fresh_count}, dirty units: {dirty_count}");

        Ok(())
    }

    fn check_unit_freshness(
        &self,
        build_runner: &mut BuildRunner<'_, '_>,
        unit: &Unit,
        fingerprint: &Arc<Fingerprint>,
    ) -> CargoResult<DependencyFreshness> {
        let current_hash = cargo::util::hex::to_hex(fingerprint.hash_u64());

        // Get the path to the old fingerprint file in the .fingerprint directory
        let fingerprint_file_path = build_runner.files().fingerprint_file_path(unit, "");
        let fingerprint_path_str = fingerprint_file_path.to_string_lossy().to_string();

        // Compare with the old fingerprint to determine freshness
        // This uses Cargo's internal comparison logic that checks:
        // - Fingerprint hash changes
        // - Source file mtimes
        // - Dependencies changes
        // - Configuration changes (rustflags, features, etc.)
        let mtime_on_use = build_runner.bcx.gctx.cli_unstable().mtime_on_use;
        let dirty_reason = compare_old_fingerprint(
            unit,
            &fingerprint_file_path,
            fingerprint,
            mtime_on_use,
            false, // force_rebuild
        );

        // Convert the dirty reason to our format
        let (is_fresh, dirty_reason_str) = match dirty_reason {
            None => (true, None),
            Some(reason) => (false, Some(format!("{:?}", reason))),
        };

        Ok(DependencyFreshness {
            unit: unit.clone(),
            is_fresh,
            dirty_reason: dirty_reason_str,
            current_fingerprint_hash: current_hash,
            fingerprint_path: fingerprint_path_str,
        })
    }
}

struct DependencyFreshness {
    #[allow(dead_code)]
    unit: Unit,
    is_fresh: bool,
    dirty_reason: Option<String>,
    current_fingerprint_hash: String,
    fingerprint_path: String,
}
