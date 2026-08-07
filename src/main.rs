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
use crate::catalog::{Catalog, CollectIntent, parse_intent_list, probe_intents};
use crate::utils::{
    RemovalStats, display_path, path_size, profile_to_dir, remove_dirs, remove_files,
};

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

/// Remove `--target <triple>` / `--target=<triple>` from forwarded cargo
/// args. cargo-gc only manages host artifacts in the profile directory, so a
/// forwarded target would redirect produced files to `target/<triple>/...`
/// and make the collected hashes mismatch the scanned directory.
fn strip_target_args(cargo_args: &[String]) -> Vec<String> {
    let mut filtered = Vec::with_capacity(cargo_args.len());
    let mut i = 0;
    while i < cargo_args.len() {
        match cargo_args[i].as_str() {
            "--target" => {
                i += 2;
            }
            arg if arg.starts_with("--target=") => {
                i += 1;
            }
            _ => {
                filtered.push(cargo_args[i].clone());
                i += 1;
            }
        }
    }
    filtered
}

fn report_cleanup_plan(plan: &CleanupPlan) {
    let reclaim = plan_reclaim_bytes(plan);
    println!(
        "Cleanup Plan:\n\
        - Stale deps artifacts: {}\n\
        - Stale fingerprint dirs: {}\n\
        - Stale incremental dirs: {}\n\
        - Total filesystem entries: {}\n\
        - Estimated reclaim: {}",
        plan.deps_files.len(),
        plan.fingerprint_dirs.len(),
        plan.incremental_dirs.len(),
        plan.total_paths(),
        humansize::format_size(reclaim, DECIMAL),
    );
}

/// Sum up the sizes of every path currently planned for removal.
fn plan_reclaim_bytes(plan: &CleanupPlan) -> u64 {
    plan.deps_files
        .iter()
        .map(|path| path_size(path))
        .chain(plan.fingerprint_dirs.iter().map(|path| path_size(path)))
        .chain(plan.incremental_dirs.iter().map(|path| path_size(path)))
        .sum()
}

fn print_plan_paths(plan: &CleanupPlan) {
    for (label, paths) in [
        ("deps files to remove", &plan.deps_files),
        ("fingerprint dirs to remove", &plan.fingerprint_dirs),
        ("incremental dirs to remove", &plan.incremental_dirs),
    ] {
        println!("{label}:");
        let mut sorted: Vec<_> = paths.iter().collect();
        sorted.sort();
        for path in sorted {
            println!("  {}", display_path(path));
        }
    }
}

/// Resolve which intents to collect. Precedence: CLI first, then the
/// `CARGO_GC_COLLECT` env var, then the `[package.metadata.cargo-gc]` table
/// of the root manifest, then probing the target directory.
fn resolve_intents(
    cli: &[CollectIntent],
    profile: &str,
    profile_dir: &std::path::Path,
    metadata: &cargo_metadata::Metadata,
) -> Result<Vec<CollectIntent>> {
    if !cli.is_empty() {
        return Ok(cli.to_vec());
    }

    if let Ok(values) = std::env::var("CARGO_GC_COLLECT")
        && !values.trim().is_empty()
    {
        return parse_intent_list_or_err("CARGO_GC_COLLECT", &values);
    }

    if let Some(intents) =
        collect_from_metadata(metadata, profile, "collect")?
    {
        return Ok(intents);
    }

    Ok(probe_intents(profile_dir))
}

/// Resolve the `collect` list from `[package.metadata.cargo-gc]`, preferring
/// the per-profile table:
///
/// ```toml
/// [package.metadata.cargo-gc]
/// collect = ["check"]
/// [package.metadata.cargo-gc.profile.release]
/// collect = ["build", "test"]
/// ```
///
/// An empty list (or no list at all) means "not configured here".
fn collect_from_metadata(
    metadata: &cargo_metadata::Metadata,
    profile: &str,
    key: &str,
) -> Result<Option<Vec<CollectIntent>>> {
    let Some(root) = metadata.root_package() else {
        return Ok(None);
    };
    let Some(config) = root.metadata.get("cargo-gc") else {
        return Ok(None);
    };

    // Per-profile table wins over the global one.
    let per_profile = config
        .get("profile")
        .and_then(|profiles| profiles.get(profile))
        .and_then(|spec| spec.get(key))
        .and_then(|value| value.as_array());
    if let Some(items) = per_profile
        && !items.is_empty()
    {
        return parse_collect_items(
            &format!("[package.metadata.cargo-gc.profile.{profile}] {key}"),
            items,
        )
        .map(Some);
    }

    if let Some(items) = config.get(key).and_then(|value| value.as_array())
        && !items.is_empty()
    {
        return parse_collect_items(
            &format!("[package.metadata.cargo-gc] {key}"),
            items,
        )
        .map(Some);
    }

    Ok(None)
}

fn parse_collect_items(
    source: &str,
    items: &[serde_json::Value],
) -> Result<Vec<CollectIntent>> {
    let text = items
        .iter()
        .filter_map(|item| item.as_str())
        .collect::<Vec<_>>()
        .join(",");
    parse_intent_list_or_err(source, &text)
}

/// Parse an intent list, failing loudly when the source provided a value
/// but none of it was understood.
fn parse_intent_list_or_err(source: &str, values: &str) -> Result<Vec<CollectIntent>> {
    let parsed = parse_intent_list(values);
    if parsed.is_empty() {
        return Err(anyhow::anyhow!(
            "{source} has no valid intent values ({values:?}), expected build, check and/or test"
        ));
    }
    Ok(parsed)
}

fn main() -> Result<()> {
    let args = Args::from(Cli::parse());

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

    let intents = resolve_intents(
        &args.collect,
        &effective_profile,
        profile_path.as_std_path(),
        &metadata,
    )?;
    if intents.is_empty() {
        println!(
            "Warning: no build artifacts found in {profile_path} yet, nothing to do.\n\
             Run `cargo build` first, or force collection with `cargo gc --collect build`."
        );
        return Ok(());
    }
    let intent_names = intents
        .iter()
        .map(|intent| match intent {
            CollectIntent::Build => "build",
            CollectIntent::Check => "check",
            CollectIntent::Test => "test",
        })
        .collect::<Vec<_>>()
        .join(", ");
    println!("Collecting live artifacts: {intent_names}");

    let forwarded_args = strip_target_args(&args.cargo_args);
    if forwarded_args.len() != args.cargo_args.len() {
        println!("Note: ignoring --target, gc only manages the host profile directory");
    }
    let catalog = Catalog::collect(
        profile_path.as_std_path(),
        &intents,
        profile_arg.as_deref(),
        &forwarded_args,
    )
    .context("failed to collect live artifacts from the current toolchain")?;
    println!(
        "Collected {} artifact hashes from the current toolchain",
        catalog.hashes.len()
    );

    println!("Scanning {}", display_path(profile_path.as_std_path()));
    let betty = Beatrice::scan(profile_path.as_std_path()).context("failed to scan the project")?;
    println!("{}", betty.report());

    let cleanup_plan = betty.plan_cleanup(&catalog.hashes);
    report_cleanup_plan(&cleanup_plan);

    if args.verbose {
        print_plan_paths(&cleanup_plan);
    }

    if args.dry_run {
        println!("Dry run: no changes were made");
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
        "Removed {} filesystem entries from {}, {} total{}",
        stats.removed_paths,
        display_path(profile_path.as_std_path()),
        humansize::format_size(stats.reclaimed_bytes, DECIMAL),
        fail_report,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_target_args() {
        let args = vec![
            "--target".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "--features".to_string(),
            "foo".to_string(),
            "--target=wasm32-unknown-unknown".to_string(),
        ];
        assert_eq!(
            strip_target_args(&args),
            vec!["--features".to_string(), "foo".to_string()]
        );
    }

    #[test]
    fn test_parse_collect_items_with_garbage() {
        let items = vec![
            serde_json::Value::String("build".to_string()),
            serde_json::Value::String("bogus".to_string()),
        ];
        // Unknown values are skipped; known ones are kept.
        assert_eq!(
            parse_collect_items("test", &items).unwrap(),
            vec![CollectIntent::Build]
        );

        let all_garbage = vec![serde_json::Value::String("nope".to_string())];
        assert!(parse_collect_items("test", &all_garbage).is_err());
    }
}
