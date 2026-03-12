mod args;
mod beatrice;
mod config;
mod scan;
mod utils;

use anyhow::{Context, Result};
use args::{Args, Cli};
use cargo_metadata::MetadataCommand;
use clap::Parser;
use humansize::DECIMAL;

use crate::beatrice::{Beatrice, CleanupPlan};
use crate::config::StaticScanConfig;
use crate::scan::Scanner;
use crate::utils::{RemovalStats, profile_to_dir, remove_dirs, remove_files};

fn extract_fingerprint(file_stem: &str) -> Option<(String, String)> {
    file_stem
        .rsplit_once('-')
        .map(|(name, fingerprint)| (name.to_string(), fingerprint.to_string()))
}

fn report_cleanup_plan(plan: &CleanupPlan) {
    println!(
        "Cleanup Plan:\n\
        - Stale deps artifacts: {}\n\
        - Stale fingerprint dirs: {}\n\
        - Stale incremental dirs: {}\n\
        - Total filesystem entries: {}",
        plan.deps_files.len(),
        plan.fingerprint_dirs.len(),
        plan.incremental_dirs.len(),
        plan.total_paths(),
    );
}

fn main() -> Result<()> {
    let args = Args::from_cli(Cli::parse());

    let scan_config = StaticScanConfig::from_args(&args)
        .context("failed to parse forwarded cargo build arguments")?;
    let profile_dir_name = profile_to_dir(&scan_config.profile_name).to_string();
    let scanner = Scanner::try_new(scan_config).context("failed to create scanner")?;

    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .context("failed to retrieve cargo metadata")?;
    let target_path = metadata.target_directory;
    let profile_path = target_path.join(profile_dir_name);

    let mut betty = Beatrice::open(profile_path.clone());
    betty.load_library().context("failed to load library")?;
    println!("{}", betty.report());

    let scan_result = scanner
        .scan(args.verbose)
        .context("failed to statically scan the project")?;
    println!("{}", scan_result.report());

    let mut cleanup_plan = betty.plan_cleanup(&scan_result);
    cleanup_plan.incremental_dirs = betty
        .load_incremental()
        .context("failed to calculate incremental files")?;
    report_cleanup_plan(&cleanup_plan);

    if args.verbose {
        println!("deps files to remove {:#?}", cleanup_plan.deps_files);
        println!(
            "fingerprint dirs to remove {:#?}",
            cleanup_plan.fingerprint_dirs
        );
        println!(
            "incremental dirs to remove {:#?}",
            cleanup_plan.incremental_dirs
        );
    }

    if args.dry_run {
        println!("abort due to dry run");
        return Ok(());
    }

    let mut stats = RemovalStats::default();
    stats.merge(remove_files(&cleanup_plan.deps_files));
    stats.merge(remove_dirs(&cleanup_plan.fingerprint_dirs));
    stats.merge(remove_dirs(&cleanup_plan.incremental_dirs));

    let fail_report = if stats.failed_paths == 0 {
        String::new()
    } else {
        format!(", {} paths failed to remove", stats.failed_paths)
    };
    println!(
        "Removed {} filesystem entries from {:?}, {} total{}",
        stats.removed_paths,
        profile_path,
        humansize::format_size(stats.reclaimed_bytes, DECIMAL),
        fail_report,
    );
    Ok(())
}
