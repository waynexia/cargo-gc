use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use indicatif::ProgressBar;

use crate::utils::extract_fingerprint;

/// Set of "live" artifact hashes collected from the current toolchain.
///
/// Instead of linking against a pinned cargo library (whose hash algorithm
/// drifts from the installed `cargo`), we invoke the real `cargo` — the same
/// binary the user builds with (`CARGO` env var, set by the cargo proxy) —
/// with `--message-format=json`. Every `compiler-artifact` message reports the
/// produced filenames; the `name-hash` suffix of each file is a build-unit
/// hash. Artifacts whose hash is not in this set are stale.
pub struct Catalog {
    /// Hashes of artifacts produced by the latest build/check/test run.
    pub hashes: HashSet<String>,
}

impl Catalog {
    /// Run `cargo build`, `cargo check` and `cargo test --no-run` with the
    /// current toolchain and collect the union of produced artifact hashes.
    pub fn collect(
        profile_dir: &Path,
        profile_arg: Option<&str>,
        cargo_args: &[String],
    ) -> Result<Self> {
        let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());

        let mut shared: Vec<String> = Vec::new();
        if let Some(profile) = profile_arg {
            shared.push(profile.to_string());
        }
        shared.extend(cargo_args.iter().cloned());
        shared.push("--message-format=json".to_string());

        let mut hashes = HashSet::new();
        let spinner = ProgressBar::new_spinner();
        spinner.enable_steady_tick(Duration::from_millis(100));

        for (verb, extra, description) in [
            (
                "build",
                &[][..],
                "running cargo build to gather artifacts...",
            ),
            (
                "check",
                &[][..],
                "running cargo check to gather artifacts...",
            ),
            (
                "test",
                &["--no-run"][..],
                "running cargo test to gather artifacts...",
            ),
        ] {
            spinner.set_message(description);

            let mut cmd = Command::new(&cargo_bin);
            cmd.arg(verb);
            cmd.args(extra);
            cmd.args(&shared);
            let output = cmd
                .output()
                .with_context(|| format!("failed to execute `cargo {verb}`"))?;
            if !output.status.success() {
                spinner.finish_and_clear();
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow::anyhow!(
                    "cargo {verb} failed, aborting before removing anything: {stderr}"
                ));
            }
            collect_output(&output.stdout, &mut hashes);
        }
        spinner.finish_and_clear();

        // Build-script units never produce a hashed file under `deps`, their
        // unit hash only shows up as `<crate>-<hash>` in the `build`
        // directory. Without those hashes the fingerprint dirs of every
        // build script would be considered stale and re-run on the next
        // build. Play it safe and treat them as live.
        collect_build_hashes(&profile_dir.join("build"), &mut hashes);

        if hashes.is_empty() {
            return Err(anyhow::anyhow!(
                "no artifacts were collected from `cargo`, you can just run `cargo clean`"
            ));
        }

        Ok(Self { hashes })
    }
}

/// Add the `<crate>-<hash>` names from the `build` directory.
fn collect_build_hashes(build_dir: &Path, hashes: &mut HashSet<String>) {
    let Ok(dir_iter) = fs::read_dir(build_dir) else {
        return;
    };
    for entry in dir_iter.flatten() {
        if let Some((_, hash)) = extract_fingerprint(&entry.path()) {
            hashes.insert(hash);
        }
    }
}

/// Parse the JSON-message stream and record every `name-hash` filename.
fn collect_output(stdout: &[u8], hashes: &mut HashSet<String>) -> usize {
    let mut count = 0;
    for line in stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let Some(filenames) = value.get("filenames").and_then(|v| v.as_array()) else {
            continue;
        };
        for filename in filenames {
            let Some(name) = filename.as_str() else {
                continue;
            };
            if let Some((_, hash)) = extract_fingerprint(Path::new(name))
                && hashes.insert(hash)
            {
                count += 1;
            }
        }
    }
    count
}
