use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::ValueEnum;
use indicatif::ProgressBar;

use crate::utils::extract_fingerprint;

/// Which cargo invocation kinds should be collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CollectIntent {
    Build,
    Check,
    Test,
}

impl CollectIntent {
    fn as_command(&self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Build => ("build", &[]),
            Self::Check => ("check", &[]),
            Self::Test => ("test", &["--no-run"]),
        }
    }
}

/// Parse a comma/space separated list of intent names, ignoring unknown
/// values. Used for the `CARGO_GC_COLLECT` env var and Cargo.toml metadata.
pub fn parse_intent_list(values: &str) -> Vec<CollectIntent> {
    values
        .split([',', ' ', ';'])
        .filter(|item| !item.is_empty())
        .filter_map(|item| match item {
            "build" => Some(CollectIntent::Build),
            "check" => Some(CollectIntent::Check),
            "test" => Some(CollectIntent::Test),
            _ => None,
        })
        .collect()
}

/// Probe which intents have produced artifacts in the profile directory and
/// return the commands gc should collect to keep them alive.
///
/// Signals:
/// - a `.rlib` in `deps` means a real build was run
/// - a `.rmeta` without a matching `.rlib` means `cargo check` was used
/// - a `test-*` fingerprint file means tests were compiled
/// - an empty result means nothing was built in this profile yet
pub fn probe_intents(profile_dir: &Path) -> Vec<CollectIntent> {
    let mut intents = Vec::new();

    let deps_dir = profile_dir.join("deps");
    if deps_dir.is_dir() {
        let names: Vec<String> = fs::read_dir(&deps_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str().map(String::from))
            .collect();

        let mut saw_rlib = false;
        let mut saw_check_rmeta = false;
        for name in &names {
            if name.ends_with(".rlib") {
                saw_rlib = true;
            } else if let Some(stem) = name.strip_suffix(".rmeta")
                && !names.iter().any(|other| other == &format!("{stem}.rlib"))
            {
                saw_check_rmeta = true;
            }
        }
        if saw_rlib {
            intents.push(CollectIntent::Build);
        }
        if saw_check_rmeta {
            intents.push(CollectIntent::Check);
        }
    }

    let fingerprint_dir = profile_dir.join(".fingerprint");
    if fingerprint_dir.is_dir()
        && fs::read_dir(&fingerprint_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| dir_has_test_fingerprint(&entry.path()))
    {
        intents.push(CollectIntent::Test);
    }

    intents.sort_by_key(|intent| match intent {
        CollectIntent::Build => 0,
        CollectIntent::Check => 1,
        CollectIntent::Test => 2,
    });
    intents
}

fn dir_has_test_fingerprint(dir: &Path) -> bool {
    fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with("test-"))
}

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
        intents: &[CollectIntent],
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

        let mut used = Vec::with_capacity(intents.len());
        for intent in intents {
            let (verb, extra) = intent.as_command();
            // Deduplicate repeated intents from the CLI.
            if used.contains(&verb) {
                continue;
            }
            used.push(verb);

            spinner.set_message(format!("running cargo {verb} to gather artifacts..."));

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_intent_list() {
        assert_eq!(
            parse_intent_list("build,test"),
            vec![CollectIntent::Build, CollectIntent::Test]
        );
        assert_eq!(parse_intent_list("check"), vec![CollectIntent::Check]);
        assert_eq!(parse_intent_list(""), Vec::<CollectIntent>::new());
        assert_eq!(parse_intent_list("bogus"), Vec::<CollectIntent>::new());
        assert_eq!(
            parse_intent_list("build; check ;test"),
            vec![
                CollectIntent::Build,
                CollectIntent::Check,
                CollectIntent::Test
            ]
        );
    }

    #[test]
    fn test_probe_intents_empty_dir_returns_nothing() {
        let tmp = std::env::temp_dir().join(format!("cargo-gc-probe-empty-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let intents = probe_intents(&tmp);
        std::fs::remove_dir_all(&tmp).unwrap();
        assert!(intents.is_empty());
    }
}
