use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use cargo::core::Workspace;
use cargo::core::compiler::fingerprint::{Fingerprint, calculate, compare_old_fingerprint};
use cargo::core::compiler::unit_graph::UnitDep;
use cargo::core::compiler::{
    self, BuildConfig, BuildRunner, MessageFormat, RustcTargetData, Unit, UnitInterner,
};
use cargo::core::profiles::Profiles;
use cargo::ops::{CompileOptions, create_bcx, resolve_ws_with_opts};
use cargo::util::interning::InternedString;
use cargo::{CargoResult, GlobalContext};

use crate::config::{ScanSpec, StaticScanConfig};

pub struct Scanner {
    config: StaticScanConfig,
    gctx: GlobalContext,
}

pub struct ScanResult {
    unit_results: Vec<UnitScan>,
    pub keep_paths: HashSet<PathBuf>,
    pub keep_dep_paths: HashSet<PathBuf>,
    pub keep_fingerprint_dirs: HashSet<PathBuf>,
}

impl Default for ScanResult {
    fn default() -> Self {
        Self {
            unit_results: Vec::new(),
            keep_paths: HashSet::new(),
            keep_dep_paths: HashSet::new(),
            keep_fingerprint_dirs: HashSet::new(),
        }
    }
}

struct UnitScan {
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
    dep_info_paths: Vec<PathBuf>,
}

impl ScanResult {
    #[cfg(test)]
    pub fn from_live_sets(
        keep_dep_paths: HashSet<PathBuf>,
        keep_fingerprint_dirs: HashSet<PathBuf>,
    ) -> Self {
        let keep_paths = keep_dep_paths
            .iter()
            .chain(keep_fingerprint_dirs.iter())
            .cloned()
            .collect();
        Self {
            unit_results: Vec::new(),
            keep_paths,
            keep_dep_paths,
            keep_fingerprint_dirs,
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
            - Keep deps artifacts: {}\n\
            - Keep fingerprint dirs: {}\n\
            - Keep paths total: {}",
            self.unit_results.len(),
            fresh_units,
            dirty_units,
            self.keep_dep_paths.len(),
            self.keep_fingerprint_dirs.len(),
            self.keep_paths.len(),
        );

        if !dirty_reasons.is_empty() {
            report.push_str("\nDirty reasons:");
            for (reason, count) in dirty_reasons {
                report.push_str(&format!("\n- {reason}: {count}"));
            }
        }

        report
    }

    fn merge(&mut self, mut other: Self) {
        self.unit_results.append(&mut other.unit_results);
        self.keep_paths.extend(other.keep_paths);
        self.keep_dep_paths.extend(other.keep_dep_paths);
        self.keep_fingerprint_dirs
            .extend(other.keep_fingerprint_dirs);
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
        let mut aggregate = ScanResult::default();

        for request in &self.config.scan_specs {
            if show_result {
                println!(
                    "Scanning profile={} intent={:?}",
                    request.requested_profile, request.intent
                );
            }
            aggregate.merge(self.scan_request(&workspace, request, show_result)?);
        }

        Ok(aggregate)
    }

    fn scan_request(
        &self,
        workspace: &Workspace<'_>,
        request: &ScanSpec,
        show_result: bool,
    ) -> CargoResult<ScanResult> {
        let mut target_data = RustcTargetData::new(workspace, &self.config.requested_kinds)?;
        let pkg_specs = request.packages.to_package_id_specs(workspace)?;

        let _workspace_resolve = resolve_ws_with_opts(
            workspace,
            &mut target_data,
            &self.config.requested_kinds,
            &self.config.features,
            &pkg_specs,
            request.has_dev_units,
            request.force_all_targets,
            false,
        )?;

        let _ = Profiles::new(workspace, InternedString::new(&request.requested_profile))?;

        let build_config = self.build_config(request);
        let compile_options = CompileOptions {
            build_config: build_config.clone(),
            cli_features: self.config.features.clone(),
            spec: request.packages.clone(),
            filter: request.filter.clone(),
            target_rustdoc_args: None,
            target_rustc_args: None,
            target_rustc_crate_types: None,
            rustdoc_document_private_items: false,
            honor_rust_version: None,
        };
        let interner = UnitInterner::new();
        let build_ctx = create_bcx(workspace, &compile_options, &interner)?;

        let units = sort_units_by_dependencies(&build_ctx.unit_graph);

        println!(
            "Resolved {} units in the workspace for profile={} intent={:?}",
            units.len(),
            request.requested_profile,
            request.intent
        );

        let mut build_runner = BuildRunner::new(&build_ctx)?;
        build_runner.lto = compiler::lto::generate(&build_ctx)?;
        build_runner.prepare_units()?;
        build_runner.prepare()?;
        compiler::custom_build::build_map(&mut build_runner)?;

        let mut unit_results = Vec::with_capacity(units.len());
        let mut keep_paths = HashSet::new();
        let mut keep_dep_paths = HashSet::new();
        let mut keep_fingerprint_dirs = HashSet::new();

        for unit in units {
            if unit.mode.is_doc_test() {
                if show_result {
                    println!(
                        "Skipping doctest unit {} target={}",
                        unit.pkg.package_id(),
                        unit.target.name(),
                    );
                }
                continue;
            }
            let fingerprint = calculate(&mut build_runner, &unit)?;
            let unit_scan = self.inspect_unit(&mut build_runner, &unit, &fingerprint)?;
            keep_fingerprint_dirs.insert(unit_scan.fingerprint_dir.clone());
            keep_paths.insert(unit_scan.fingerprint_dir.clone());
            keep_dep_paths.extend(unit_scan.dep_paths.iter().cloned());
            keep_dep_paths.extend(unit_scan.dep_info_paths.iter().cloned());
            keep_paths.extend(unit_scan.dep_paths.iter().cloned());
            keep_paths.extend(unit_scan.dep_info_paths.iter().cloned());

            if show_result {
                Self::print_unit_scan(&unit_scan);
            }

            unit_results.push(unit_scan);
        }

        Ok(ScanResult {
            unit_results,
            keep_paths,
            keep_dep_paths,
            keep_fingerprint_dirs,
        })
    }

    fn build_config(&self, request: &ScanSpec) -> BuildConfig {
        BuildConfig {
            requested_kinds: self.config.requested_kinds.clone(),
            jobs: 1,
            keep_going: false,
            requested_profile: InternedString::new(&request.requested_profile),
            intent: request.intent,
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
        let mut dep_paths = Vec::new();
        let mut dep_info_paths = Vec::new();
        for output in build_runner.outputs(unit)?.iter() {
            Self::collect_dep_keep_path(
                &mut dep_paths,
                &mut dep_info_paths,
                &deps_dir,
                &output.path,
            );
            if let Some(hardlink) = &output.hardlink {
                Self::collect_dep_keep_path(
                    &mut dep_paths,
                    &mut dep_info_paths,
                    &deps_dir,
                    hardlink,
                );
            }
        }
        dep_paths.sort();
        dep_paths.dedup();
        dep_info_paths.sort();
        dep_info_paths.dedup();

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
            dep_info_paths,
        })
    }

    fn collect_dep_keep_path(
        dep_paths: &mut Vec<PathBuf>,
        dep_info_paths: &mut Vec<PathBuf>,
        deps_dir: &Path,
        path: &Path,
    ) {
        if !path.starts_with(deps_dir) {
            return;
        }

        dep_paths.push(path.to_path_buf());
        dep_info_paths.push(path.with_extension("d"));
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
        for path in &unit_scan.dep_info_paths {
            println!("    keep dep-info: {}", path.display());
        }
    }
}
