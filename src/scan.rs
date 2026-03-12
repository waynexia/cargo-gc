use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use cargo::core::Workspace;
use cargo::core::compiler::fingerprint::{Fingerprint, calculate, compare_old_fingerprint};
use cargo::core::compiler::unit_graph::UnitDep;
use cargo::core::compiler::{
    self, BuildConfig, BuildRunner, MessageFormat, RustcTargetData, Unit, UnitInterner, UserIntent,
};
use cargo::core::profiles::Profiles;
use cargo::ops::{CompileOptions, create_bcx, resolve_ws_with_opts};
use cargo::util::interning::InternedString;
use cargo::{CargoResult, GlobalContext};

use crate::config::StaticScanConfig;

pub struct Scanner {
    config: StaticScanConfig,
    gctx: GlobalContext,
}

pub struct ScanResult {
    unit_results: Vec<UnitScan>,
    pub live_dep_paths: HashSet<PathBuf>,
    pub live_fingerprint_dirs: HashSet<PathBuf>,
    pub current_package_names: HashSet<String>,
    pub current_target_names: HashSet<String>,
}

struct UnitScan {
    package_name: String,
    package_id: String,
    target_name: String,
    target_kind: String,
    mode: String,
    metadata_hash: Option<String>,
    is_fresh: bool,
    dirty_reason: Option<String>,
    current_fingerprint_hash: String,
    stored_fingerprint_hash: Option<String>,
    fingerprint_hash_path: PathBuf,
    fingerprint_dir: PathBuf,
    dep_paths: Vec<PathBuf>,
}

impl ScanResult {
    #[cfg(test)]
    pub fn from_live_sets(
        live_dep_paths: HashSet<PathBuf>,
        live_fingerprint_dirs: HashSet<PathBuf>,
    ) -> Self {
        Self {
            unit_results: Vec::new(),
            live_dep_paths,
            live_fingerprint_dirs,
            current_package_names: HashSet::new(),
            current_target_names: HashSet::new(),
        }
    }

    pub fn report(&self) -> String {
        let fresh_units = self
            .unit_results
            .iter()
            .filter(|unit| unit.is_fresh)
            .count();
        let dirty_units = self.unit_results.len() - fresh_units;
        let mut dirty_reasons = BTreeMap::new();

        for unit in self.unit_results.iter().filter(|unit| !unit.is_fresh) {
            let reason = unit
                .dirty_reason
                .as_deref()
                .unwrap_or("Unknown reason")
                .to_string();
            *dirty_reasons.entry(reason).or_insert(0usize) += 1;
        }

        let mut report = format!(
            "Static Scan Report:\n\
            - Units scanned: {}\n\
            - Fresh units: {}\n\
            - Dirty units: {}\n\
            - Live deps artifacts: {}\n\
            - Live fingerprint dirs: {}",
            self.unit_results.len(),
            fresh_units,
            dirty_units,
            self.live_dep_paths.len(),
            self.live_fingerprint_dirs.len(),
        );

        if !dirty_reasons.is_empty() {
            report.push_str("\nDirty reasons:");
            for (reason, count) in dirty_reasons {
                report.push_str(&format!("\n- {reason}: {count}"));
            }
        }

        report
    }
}

fn sort_units_by_dependencies(unit_graph: &HashMap<Unit, Vec<UnitDep>>) -> Vec<Unit> {
    let mut sorted_units = Vec::new();
    let mut visited = HashSet::new();
    let mut temp_visited = HashSet::new();

    fn visit(
        unit: &Unit,
        unit_graph: &HashMap<Unit, Vec<UnitDep>>,
        visited: &mut HashSet<Unit>,
        temp_visited: &mut HashSet<Unit>,
        sorted_units: &mut Vec<Unit>,
    ) {
        if temp_visited.contains(unit) || visited.contains(unit) {
            return;
        }

        temp_visited.insert(unit.clone());
        if let Some(deps) = unit_graph.get(unit) {
            for dep in deps {
                visit(&dep.unit, unit_graph, visited, temp_visited, sorted_units);
            }
        }
        temp_visited.remove(unit);
        visited.insert(unit.clone());
        sorted_units.push(unit.clone());
    }

    let mut all_units = unit_graph.keys().cloned().collect::<Vec<_>>();
    all_units.sort_by(|left, right| {
        left.pkg
            .package_id()
            .to_string()
            .cmp(&right.pkg.package_id().to_string())
            .then_with(|| left.target.name().cmp(right.target.name()))
            .then_with(|| format!("{:?}", left.mode).cmp(&format!("{:?}", right.mode)))
    });

    for unit in all_units {
        visit(
            &unit,
            unit_graph,
            &mut visited,
            &mut temp_visited,
            &mut sorted_units,
        );
    }

    sorted_units
}

impl Scanner {
    pub fn try_new(config: StaticScanConfig) -> CargoResult<Self> {
        Ok(Self {
            config,
            gctx: GlobalContext::default()?,
        })
    }

    pub fn scan(&self, show_result: bool) -> CargoResult<ScanResult> {
        let manifest_path = self.config.get_manifest_path();

        let workspace = Workspace::new(&manifest_path, &self.gctx)?;
        let mut target_data = RustcTargetData::new(&workspace, &self.config.requested_kinds)?;
        let pkg_specs = self.config.packages.to_package_id_specs(&workspace)?;

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

        let _ = Profiles::new(&workspace, InternedString::new(&self.config.profile_name))?;

        let build_config = self.build_config();
        let compile_options = CompileOptions {
            build_config: build_config.clone(),
            cli_features: self.config.features.clone(),
            spec: self.config.packages.clone(),
            filter: self.config.filter.clone(),
            target_rustdoc_args: None,
            target_rustc_args: None,
            target_rustc_crate_types: None,
            rustdoc_document_private_items: false,
            honor_rust_version: None,
        };
        let interner = UnitInterner::new();
        let build_ctx = create_bcx(&workspace, &compile_options, &interner)?;

        let units = sort_units_by_dependencies(&build_ctx.unit_graph);

        println!("Resolved {} units in the workspace", units.len());

        let mut build_runner = BuildRunner::new(&build_ctx)?;
        build_runner.lto = compiler::lto::generate(&build_ctx)?;
        build_runner.prepare_units()?;
        build_runner.prepare()?;
        compiler::custom_build::build_map(&mut build_runner)?;

        let mut unit_results = Vec::with_capacity(units.len());
        let mut live_dep_paths = HashSet::new();
        let mut live_fingerprint_dirs = HashSet::new();
        let mut current_package_names = HashSet::new();
        let mut current_target_names = HashSet::new();

        for unit in units {
            let fingerprint = calculate(&mut build_runner, &unit)?;
            let unit_scan = self.inspect_unit(&mut build_runner, &unit, &fingerprint)?;
            current_package_names.insert(unit_scan.package_name.clone());
            let target_name = crate::utils::normalize_package_name(&unit_scan.target_name);
            current_target_names.insert(target_name.clone());

            // Retain artifacts for every current unit in the resolved graph.
            // Freshness diagnostics are useful for reporting, but are not yet
            // strong enough to be the deletion boundary on large workspaces.
            live_fingerprint_dirs.insert(unit_scan.fingerprint_dir.clone());
            live_dep_paths.extend(unit_scan.dep_paths.iter().cloned());

            if show_result {
                Self::print_unit_scan(&unit_scan);
            }

            unit_results.push(unit_scan);
        }

        Ok(ScanResult {
            unit_results,
            live_dep_paths,
            live_fingerprint_dirs,
            current_package_names,
            current_target_names,
        })
    }

    fn build_config(&self) -> BuildConfig {
        BuildConfig {
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
        }
    }

    fn inspect_unit(
        &self,
        build_runner: &mut BuildRunner<'_, '_>,
        unit: &Unit,
        fingerprint: &Arc<Fingerprint>,
    ) -> CargoResult<UnitScan> {
        let current_fingerprint_hash = cargo::util::hex::to_hex(fingerprint.hash_u64());
        let fingerprint_hash_path = build_runner.files().fingerprint_file_path(unit, "");
        let fingerprint_dir = build_runner.files().fingerprint_dir(unit);
        let stored_fingerprint_hash = std::fs::read_to_string(&fingerprint_hash_path)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let metadata_hash = build_runner
            .files()
            .metadata(unit)
            .c_extra_filename()
            .map(|hash| hash.to_string());
        let deps_dir = build_runner.files().deps_dir(unit).to_path_buf();
        let mut dep_paths = build_runner
            .outputs(unit)?
            .iter()
            .map(|output| output.path.clone())
            .filter(|path| path.starts_with(&deps_dir))
            .collect::<Vec<_>>();
        dep_paths.sort();
        dep_paths.dedup();

        let mtime_on_use = build_runner.bcx.gctx.cli_unstable().mtime_on_use;
        let dirty_reason = compare_old_fingerprint(
            unit,
            &fingerprint_hash_path,
            fingerprint,
            mtime_on_use,
            false,
        );
        let (is_fresh, dirty_reason) = match dirty_reason {
            None => (true, None),
            Some(reason) => (false, Some(format!("{reason:?}"))),
        };

        Ok(UnitScan {
            package_name: crate::utils::normalize_package_name(unit.pkg.name().as_str()),
            package_id: unit.pkg.package_id().to_string(),
            target_name: unit.target.name().to_string(),
            target_kind: unit.target.kind().description().to_string(),
            mode: format!("{:?}", unit.mode),
            metadata_hash,
            is_fresh,
            dirty_reason,
            current_fingerprint_hash,
            stored_fingerprint_hash,
            fingerprint_hash_path,
            fingerprint_dir,
            dep_paths,
        })
    }

    fn print_unit_scan(unit_scan: &UnitScan) {
        let status = if unit_scan.is_fresh { "fresh" } else { "dirty" };
        let metadata_hash = unit_scan.metadata_hash.as_deref().unwrap_or("none");
        let stored_hash = unit_scan
            .stored_fingerprint_hash
            .as_deref()
            .unwrap_or("missing");
        let hash_match = if unit_scan.stored_fingerprint_hash.as_deref()
            == Some(unit_scan.current_fingerprint_hash.as_str())
        {
            "match"
        } else {
            "mismatch"
        };

        println!(
            "[{status}] {} target={} kind={} mode={} meta={} deps={}",
            unit_scan.package_id,
            unit_scan.target_name,
            unit_scan.target_kind,
            unit_scan.mode,
            metadata_hash,
            unit_scan.dep_paths.len(),
        );
        println!(
            "    fingerprint dir: {}",
            unit_scan.fingerprint_dir.display()
        );
        println!(
            "    fingerprint hash: current={} stored={} ({})",
            unit_scan.current_fingerprint_hash, stored_hash, hash_match
        );
        println!(
            "    fingerprint file: {}",
            unit_scan.fingerprint_hash_path.display()
        );

        if let Some(reason) = &unit_scan.dirty_reason {
            println!("    dirty reason: {reason}");
        }

        for path in &unit_scan.dep_paths {
            println!("    keep deps artifact: {}", path.display());
        }
    }
}
