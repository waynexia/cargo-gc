mod args;
mod beatrice;
mod config;
mod scan;
mod utils;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use args::{Args, Cli};
use cargo_metadata::MetadataCommand;
use clap::Parser;
use humansize::DECIMAL;
use indicatif::ProgressBar;
use serde::Deserialize;

use crate::beatrice::Beatrice;
use crate::config::StaticScanConfig;
use crate::scan::Scanner;

type Fingerprints = HashSet<(String, String)>;

struct OutputCollection {
    /// (Names, Fingerprints)
    deps_fingerprints: Fingerprints,
}

impl OutputCollection {
    fn from_json(json: &str) -> Result<Self> {
        let result = json
            .lines()
            .map(|raw| serde_json::from_str(raw).context("failed to deserialize build graph json"))
            .collect::<Result<Vec<OutputItem>>>()?;

        let mut set = HashSet::new();
        for item in result {
            for name in item.filenames.unwrap_or_default() {
                let path = PathBuf::from(name);
                let file_stem = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if file_stem.is_empty() {
                    continue;
                }
                if let Some((name, fingerprint)) = extract_fingerprint(&file_stem) {
                    set.insert((name.to_string(), fingerprint.to_string()));
                }
            }
        }
        if set.is_empty() {
            return Err(anyhow::anyhow!(
                "no valid file is found, you can just run `cargo clean`"
            ));
        }
        Ok(Self {
            deps_fingerprints: set,
        })
    }
}

fn extract_fingerprint(file_stem: &str) -> Option<(String, String)> {
    file_stem
        .rsplit_once('-')
        .map(|(name, fingerprint)| (name.to_string(), fingerprint.to_string()))
}

#[derive(Deserialize, Default)]
struct OutputItem {
    filenames: Option<Vec<String>>,
}

fn get_fingerprints(args: &Args) -> Result<Fingerprints> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_message("running cargo build to gather message...");
    spinner.enable_steady_tick(Duration::from_millis(100));

    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo_bin)
        .args(["build", "--message-format=json"])
        .args(args.cargo_profile_args())
        .args(&args.cargo_args)
        .output()
        .context("failed to execute cargo build")?;
    spinner.finish_and_clear();

    // check cargo build result
    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr).context("failed to parse stderr")?;
        return Err(anyhow::anyhow!("cargo build failed: {}", stderr));
    }

    let stdout = String::from_utf8(output.stdout).context("failed to parse stdout")?;
    let collection = OutputCollection::from_json(&stdout)?;
    Ok(collection.deps_fingerprints)
}

fn main() -> Result<()> {
    let args = Args::from_cli(Cli::parse());

    let scan_config = StaticScanConfig::from_args(&args);
    let scanner = Scanner::try_new(scan_config).context("failed to create scanner")?;

    let fingerprints = get_fingerprints(&args)?;
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .context("failed to retrieve cargo metadata")?;
    let target_path = metadata.target_directory;
    let profile_path = target_path.join(args.profile);

    // Create Beatrice instance early so we can use it with Scanner
    let mut betty = Beatrice::open(profile_path.clone());
    betty.load_library().context("failed to load library")?;

    // Run scanner with Beatrice integration
    scanner
        .scan(&mut betty, false)
        .context("failed to scan the project")?;
    println!("{}", betty.report());

    let deps_path = profile_path.join("deps");
    let files_iter = fs::read_dir(deps_path.clone())
        .with_context(|| format!("failed to read deps directory: {deps_path:?}"))?;

    let mut files_to_remove = HashSet::new();
    // Find the newest file for each crate
    for file in files_iter {
        let file = file.with_context(|| format!("failed to read file in {deps_path:?}"))?;
        if file
            .file_type()
            .context("failed to get fs entry type")?
            .is_dir()
        {
            continue;
        }

        let path = file.path();
        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let full_file_path = path
            .canonicalize()
            .with_context(|| format!("cannot canonicalize path {path:?}"))?
            .to_string_lossy()
            .to_string();
        let stem = path
            .file_stem()
            .with_context(|| format!("cannot get file stem of {path:?}"))?
            .to_string_lossy()
            .to_string();
        let Some((name, fingerprint)) = extract_fingerprint(&stem) else {
            // Skip files that are not in the format of `name-fingerprint`.
            // They are `.d` files for output targets.
            continue;
        };

        if !fingerprints.contains(&(name, fingerprint)) && ext != "d" {
            files_to_remove.insert(full_file_path.clone());
        }
    }

    println!("found {} outdated dep files", files_to_remove.len());

    // let incremental_files_to_remove = incremental_files(&profile_path)?;
    let incremental_files_to_remove = betty
        .load_incremental()
        .context("failed to calculate incremental files")?;
    println!(
        "found {} incremental files",
        incremental_files_to_remove.len()
    );

    if args.verbose {
        println!("files to remove {files_to_remove:#?}");
        println!("incremental files to remove {incremental_files_to_remove:#?}");
    }
    if args.dry_run {
        println!("abort due to dry run");
        return Ok(());
    }

    // Remove old files
    let mut failed = 0;
    let total_count = files_to_remove.len();
    let mut success_size = 0;
    for file in files_to_remove {
        let size = fs::metadata(&file).map(|m| m.len()).unwrap_or_default();
        success_size += size;
        if let Err(e) = fs::remove_file(file) {
            failed += 1;
            success_size -= size;
            println!("failed to remove file: {e}");
        };
    }
    for dir in incremental_files_to_remove {
        let dir_iter = fs::read_dir(dir.clone())
            .with_context(|| format!("failed to read incremental directory: {dir:?}"))?;
        let size: u64 = dir_iter
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let metadata = entry.metadata().ok()?;
                let size = metadata.len();
                Some(size)
            })
            .sum();
        success_size += size;
        if let Err(e) = fs::remove_dir_all(dir) {
            failed += 1;
            success_size -= size;
            println!("failed to remove dir: {e}");
        };
    }

    let fail_report = if failed == 0 {
        "".to_string()
    } else {
        format!(", {failed} files failed to remove")
    };
    println!(
        "Removed {} files from {:?}, {} total{}",
        total_count - failed,
        profile_path,
        humansize::format_size(success_size, DECIMAL),
        fail_report,
    );
    Ok(())
}
