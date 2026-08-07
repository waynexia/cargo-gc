use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::utils::extract_fingerprint;

/// A filesystem entry together with the `name-hash` fingerprint parsed from
/// its file name. `hash == None` means the name does not follow the cargo
/// artifact naming rule (`name-<16 hex>`) and is kept conservatively.
#[derive(Debug, Clone)]
pub struct TrackedEntry {
    pub path: PathBuf,
    pub hash: Option<String>,
    /// True when this is a `bin-*` fingerprint (binary target). Binary
    /// targets do not produce a hashed filename that could appear in the
    /// catalog (the executable sits bare in the profile dir), so their
    /// fingerprint dirs must never be considered stale.
    pub is_bin: bool,
}

#[derive(Debug, Default)]
pub struct CleanupPlan {
    pub deps_files: HashSet<PathBuf>,
    pub fingerprint_dirs: HashSet<PathBuf>,
    pub incremental_dirs: HashSet<PathBuf>,
}

impl CleanupPlan {
    pub fn total_paths(&self) -> usize {
        self.deps_files.len() + self.fingerprint_dirs.len() + self.incremental_dirs.len()
    }
}

/// Scans the profile output directory (`target/debug` or `target/release`).
pub struct Beatrice {
    fingerprint_dirs: Vec<TrackedEntry>,
    dep_artifacts: Vec<TrackedEntry>,
    incremental_dirs: Vec<TrackedEntry>,
}

impl Beatrice {
    pub fn scan(profile_dir: &Path) -> Result<Self> {
        let mut fingerprint_dirs = Vec::new();
        let mut dep_artifacts = Vec::new();
        let mut incremental_dirs = Vec::new();

        let fingerprint_path = profile_dir.join(".fingerprint");
        if fingerprint_path.exists() {
            scan_fingerprint_directory(&fingerprint_path, &mut fingerprint_dirs)?;
        }

        let deps_path = profile_dir.join("deps");
        if deps_path.exists() {
            scan_directory(&deps_path, &mut dep_artifacts)?;
        }

        let incremental_path = profile_dir.join("incremental");
        if incremental_path.exists() {
            scan_directory(&incremental_path, &mut incremental_dirs)?;
        }

        Ok(Self {
            fingerprint_dirs,
            dep_artifacts,
            incremental_dirs,
        })
    }

    /// Plan cleanup: every entry whose hash is not among `live_hashes` is
    /// stale. Entries whose name cannot be parsed are always kept.
    pub fn plan_cleanup(&self, live_hashes: &HashSet<String>) -> CleanupPlan {
        CleanupPlan {
            deps_files: self
                .dep_artifacts
                .iter()
                .filter(|entry| Self::is_stale(entry, live_hashes))
                .map(|entry| entry.path.clone())
                .collect(),
            fingerprint_dirs: self
                .fingerprint_dirs
                .iter()
                .filter(|entry| Self::is_stale(entry, live_hashes))
                .map(|entry| entry.path.clone())
                .collect(),
            incremental_dirs: self
                .incremental_dirs
                .iter()
                .filter(|entry| Self::is_stale(entry, live_hashes))
                .map(|entry| entry.path.clone())
                .collect(),
        }
    }

    fn is_stale(entry: &TrackedEntry, live_hashes: &HashSet<String>) -> bool {
        // Binary units cannot be matched against the catalog at all, and
        // unparseable names are always kept.
        if entry.is_bin || entry.hash.is_none() {
            return false;
        }
        match &entry.hash {
            Some(hash) => !live_hashes.contains(hash),
            None => unreachable!(),
        }
    }

    pub fn report(&self) -> String {
        format!(
            "Beatrice Library Report:\n\
            - Fingerprint dirs on disk: {}\n\
            - Deps entries on disk: {}\n\
            - Incremental dirs on disk: {}",
            self.fingerprint_dirs.len(),
            self.dep_artifacts.len(),
            self.incremental_dirs.len(),
        )
    }
}

/// Enumerate a directory and parse the fingerprint suffix of each entry.
fn scan_directory(dir_path: &Path, target: &mut Vec<TrackedEntry>) -> Result<()> {
    let dir_iter = fs::read_dir(dir_path)
        .with_context(|| format!("failed to read directory: {dir_path:?}"))?;

    for entry in dir_iter {
        let entry = entry.with_context(|| format!("failed to read entry in {dir_path:?}"))?;
        let path = entry.path();
        let hash = extract_fingerprint(&path).map(|(_, hash)| hash);
        target.push(TrackedEntry {
            path,
            hash,
            is_bin: false,
        });
    }

    target.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(())
}

/// Enumerate `.fingerprint` and mark directories that hold a `bin-*` file.
fn scan_fingerprint_directory(dir_path: &Path, target: &mut Vec<TrackedEntry>) -> Result<()> {
    let dir_iter = fs::read_dir(dir_path)
        .with_context(|| format!("failed to read directory: {dir_path:?}"))?;

    for entry in dir_iter {
        let entry = entry.with_context(|| format!("failed to read entry in {dir_path:?}"))?;
        let path = entry.path();
        let hash = extract_fingerprint(&path).map(|(_, hash)| hash);
        let is_bin = fingerprint_dir_has_bin(&path);
        target.push(TrackedEntry { path, hash, is_bin });
    }

    target.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(())
}

/// A binary unit fingerprint dir contains a `bin-<name>` hash file
/// (libraries use `lib-<name>`, build scripts use `build-script-*`).
fn fingerprint_dir_has_bin(dir_path: &Path) -> bool {
    let Ok(dir_iter) = fs::read_dir(dir_path) else {
        return false;
    };
    dir_iter.flatten().any(|entry| {
        let name = entry.file_name();
        name.to_string_lossy().starts_with("bin-")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_cleanup_keeps_live_hashes_only() {
        let beatrice = Beatrice {
            fingerprint_dirs: vec![
                TrackedEntry {
                    path: PathBuf::from("/tmp/.fingerprint/foo-aaa"),
                    hash: Some("aaa".to_string()),
                    is_bin: false,
                },
                TrackedEntry {
                    path: PathBuf::from("/tmp/.fingerprint/foo-bbb"),
                    hash: Some("bbb".to_string()),
                    is_bin: false,
                },
            ],
            dep_artifacts: vec![
                TrackedEntry {
                    path: PathBuf::from("/tmp/deps/libfoo-aaa.rlib"),
                    hash: Some("aaa".to_string()),
                    is_bin: false,
                },
                TrackedEntry {
                    path: PathBuf::from("/tmp/deps/foo-bbb.d"),
                    hash: Some("bbb".to_string()),
                    is_bin: false,
                },
            ],
            incremental_dirs: vec![TrackedEntry {
                path: PathBuf::from("/tmp/incremental/foo-bbb"),
                hash: Some("bbb".to_string()),
                is_bin: false,
            }],
        };

        let live = HashSet::from(["aaa".to_string()]);
        let plan = beatrice.plan_cleanup(&live);

        assert!(
            !plan
                .deps_files
                .contains(&PathBuf::from("/tmp/deps/libfoo-aaa.rlib"))
        );
        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/deps/foo-bbb.d"))
        );
        assert!(
            !plan
                .fingerprint_dirs
                .contains(&PathBuf::from("/tmp/.fingerprint/foo-aaa"))
        );
        assert!(
            plan.fingerprint_dirs
                .contains(&PathBuf::from("/tmp/.fingerprint/foo-bbb"))
        );
        assert!(
            plan.incremental_dirs
                .contains(&PathBuf::from("/tmp/incremental/foo-bbb"))
        );
    }

    #[test]
    fn test_plan_cleanup_keeps_unparseable_entries() {
        let beatrice = Beatrice {
            fingerprint_dirs: vec![TrackedEntry {
                path: PathBuf::from("/tmp/.fingerprint/weird-name"),
                hash: None,
                is_bin: false,
            }],
            dep_artifacts: vec![
                TrackedEntry {
                    path: PathBuf::from("/tmp/deps/libfoo-aaaa.rlib"),
                    hash: Some("aaaa".to_string()),
                    is_bin: false,
                },
                TrackedEntry {
                    path: PathBuf::from("/tmp/deps/README"),
                    hash: None,
                    is_bin: false,
                },
            ],
            incremental_dirs: Vec::new(),
        };

        let live = HashSet::new();
        let plan = beatrice.plan_cleanup(&live);

        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/deps/libfoo-aaaa.rlib"))
        );
        assert!(!plan.deps_files.contains(&PathBuf::from("/tmp/deps/README")));
        assert!(
            !plan
                .fingerprint_dirs
                .contains(&PathBuf::from("/tmp/.fingerprint/weird-name"))
        );
    }

    #[test]
    fn test_plan_cleanup_always_keeps_bin_fingerprints() {
        let beatrice = Beatrice {
            fingerprint_dirs: vec![TrackedEntry {
                path: PathBuf::from("/tmp/.fingerprint/cargo-gc-bin-deadbeef00000000"),
                hash: Some("deadbeef00000000".to_string()),
                is_bin: true,
            }],
            dep_artifacts: Vec::new(),
            incremental_dirs: Vec::new(),
        };

        let live = HashSet::new();
        let plan = beatrice.plan_cleanup(&live);
        assert!(!plan.fingerprint_dirs.contains(&PathBuf::from(
            "/tmp/.fingerprint/cargo-gc-bin-deadbeef00000000"
        )));
    }
}
