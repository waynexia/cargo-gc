mod args;
mod beatrice;
mod catalog;
mod utils;

use anyhow::{Context, Result};
use args::{Args, Cli};
use cargo_metadata::MetadataCommand;
use clap::Parser;
use humansize::DECIMAL;

use crate::beatrice::{Beatrice, CleanupPlan};
use crate::catalog::Catalog;
use crate::utils::{RemovalStats, profile_to_dir, remove_dirs, remove_files};

/// Collect the profile flag forwarded via trailing cargo args, e.g.
/// `--profile release`, `--profile=release` or `--release`.
fn forwarded_profile(cargo_args: &[String]) -> (Option<String>, bool) {
    let mut i = 0;
    while i < cargo_args.len() {
        match cargo_args[i].as_str() {
            "--release" => return (None, true),
            "--profile" => {
                if let Some(profile) = cargo_args.get(i + 1) {
                    return (Some(profile.clone()), false);
                }
                i += 1;
            }
            arg if arg.starts_with("--profile=") => {
                return (Some(arg["--profile=".len()..].to_string()), false);
            }
            _ => i += 1,
        }
    }
    (None, false)
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

fn print_plan_paths(plan: &CleanupPlan) {
    println!("deps files to remove {:#?}", plan.deps_files);
    println!("fingerprint dirs to remove {:#?}", plan.fingerprint_dirs);
    println!("incremental dirs to remove {:#?}", plan.incremental_dirs);
}

fn main() -> Result<()> {
    let args = Args::from_cli(Cli::parse());

    // The effective profile decides which output directory to operate on and
    // how `cargo` is invoked. Forwarded cargo args take precedence.
    let (forwarded_profile, forwarded_release) = forwarded_profile(&args.cargo_args);
    let effective_profile = forwarded_profile
        .clone()
        .unwrap_or_else(|| args.profile.clone());

    let profile_arg: Option<String> = if forwarded_profile.is_some() || forwarded_release {
        // Already conveyed by the forwarded cargo args.
        None
    } else {
        match args.profile.as_str() {
            "dev" => None,
            "release" => Some("--release".to_string()),
            other => Some(format!("--profile={other}")),
        }
    };

    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .context("failed to retrieve cargo metadata")?;
    let profile_path = metadata
        .target_directory
        .join(profile_to_dir(&effective_profile));

    let catalog = Catalog::collect(
        profile_path.as_std_path(),
        profile_arg.as_deref(),
        &args.cargo_args,
    )
    .context("failed to collect live artifacts from the current toolchain")?;
    println!(
        "Collected {} live artifact hashes from the current cargo toolchain",
        catalog.hashes.len()
    );

    let betty = Beatrice::scan(profile_path.as_std_path()).context("failed to scan the project")?;
    println!("{}", betty.report());

    let cleanup_plan = betty.plan_cleanup(&catalog.hashes);
    report_cleanup_plan(&cleanup_plan);

    if args.verbose {
        print_plan_paths(&cleanup_plan);
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
