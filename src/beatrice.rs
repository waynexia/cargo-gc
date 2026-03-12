use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use cargo_metadata::camino::Utf8PathBuf;

use crate::extract_fingerprint;
use crate::scan::ScanResult;

#[derive(Debug, Clone)]
pub struct ItemInfo {
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct FingerprintDirectory {
    pub path: PathBuf,
    pub fingerprint_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DepArtifact {
    pub path: PathBuf,
    pub info: ItemInfo,
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

pub struct Beatrice {
    profile_dir: Utf8PathBuf,
    pub fingerprint_dirs: Vec<FingerprintDirectory>,
    pub dep_artifacts: Vec<DepArtifact>,
}

impl Beatrice {
    pub fn open(profile_dir: Utf8PathBuf) -> Self {
        Self {
            profile_dir,
            fingerprint_dirs: Vec::new(),
            dep_artifacts: Vec::new(),
        }
    }

    pub fn load_library(&mut self) -> Result<()> {
        self.fingerprint_dirs.clear();
        self.dep_artifacts.clear();

        let fingerprint_path = self.profile_dir.join(".fingerprint");
        if fingerprint_path.exists() {
            Self::scan_fingerprint_directory(&fingerprint_path, &mut self.fingerprint_dirs)?;
        }

        let deps_path = self.profile_dir.join("deps");
        if deps_path.exists() {
            Self::scan_deps_directory(&deps_path, &mut self.dep_artifacts)?;
        }

        Ok(())
    }

    fn scan_fingerprint_directory(
        dir_path: &Utf8PathBuf,
        target_library: &mut Vec<FingerprintDirectory>,
    ) -> Result<()> {
        let dir_iter = fs::read_dir(dir_path)
            .with_context(|| format!("failed to read directory: {dir_path:?}"))?;

        for entry in dir_iter {
            let entry = entry.with_context(|| format!("failed to read entry in {dir_path:?}"))?;
            let entry_path = entry.path();

            if !entry
                .file_type()
                .with_context(|| format!("failed to get file type for {:?}", entry_path))?
                .is_dir()
            {
                continue;
            }

            let fingerprint_hash = Self::read_fingerprint_hash_file(&entry_path)
                .with_context(|| format!("failed to read fingerprint file in {entry_path:?}"))?;

            target_library.push(FingerprintDirectory {
                path: entry_path,
                fingerprint_hash,
            });
        }

        target_library.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(())
    }

    fn read_fingerprint_hash_file(fingerprint_dir: &Path) -> Result<Option<String>> {
        let dir_iter = fs::read_dir(fingerprint_dir).with_context(|| {
            format!("failed to read fingerprint directory: {fingerprint_dir:?}")
        })?;

        for entry in dir_iter {
            let entry =
                entry.with_context(|| format!("failed to read entry in {fingerprint_dir:?}"))?;
            let entry_path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();

            if entry
                .file_type()
                .with_context(|| format!("failed to get file type for {:?}", entry_path))?
                .is_dir()
            {
                continue;
            }

            if !file_name.contains('.') && !file_name.starts_with("dep") {
                let content = fs::read_to_string(&entry_path)
                    .with_context(|| format!("failed to read fingerprint file: {entry_path:?}"))?;
                return Ok(Some(content.trim().to_string()));
            }
        }

        Ok(None)
    }

    fn scan_deps_directory(
        dir_path: &Utf8PathBuf,
        target_library: &mut Vec<DepArtifact>,
    ) -> Result<()> {
        let dir_iter = fs::read_dir(dir_path)
            .with_context(|| format!("failed to read directory: {dir_path:?}"))?;

        for entry in dir_iter {
            let entry = entry.with_context(|| format!("failed to read entry in {dir_path:?}"))?;
            let entry_path = entry.path();
            let metadata = entry
                .metadata()
                .with_context(|| format!("failed to get metadata of {:?}", entry_path))?;
            target_library.push(DepArtifact {
                path: entry_path,
                info: ItemInfo {
                    size: metadata.len(),
                },
            });
        }

        target_library.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(())
    }

    pub fn plan_cleanup(&self, scan: &ScanResult) -> CleanupPlan {
        let deps_files = self
            .dep_artifacts
            .iter()
            .filter(|artifact| !scan.keep_paths.contains(&artifact.path))
            .map(|artifact| artifact.path.clone())
            .collect();
        let fingerprint_dirs = self
            .fingerprint_dirs
            .iter()
            .filter(|dir| !scan.keep_paths.contains(&dir.path))
            .map(|dir| dir.path.clone())
            .collect();

        CleanupPlan {
            deps_files,
            fingerprint_dirs,
            incremental_dirs: HashSet::new(),
        }
    }

    pub fn load_incremental(&self) -> Result<HashSet<PathBuf>> {
        let incremental_path = self.profile_dir.join("incremental");
        if !incremental_path.exists() {
            return Ok(HashSet::new());
        }

        let mut pathbuf_to_remove = HashSet::new();
        let mut latest_one: HashMap<String, (String, SystemTime)> = HashMap::new();

        let dir_iter = fs::read_dir(incremental_path.clone()).with_context(|| {
            format!("failed to read incremental directory: {incremental_path:?}")
        })?;
        for dir in dir_iter {
            let dir = dir.with_context(|| format!("failed to read dir in {incremental_path:?}"))?;
            if !dir
                .file_type()
                .map(|open_dir| open_dir.is_dir())
                .unwrap_or_default()
            {
                continue;
            }
            let Some((dep_name, hash)) =
                extract_fingerprint(dir.file_name().to_string_lossy().as_ref())
            else {
                continue;
            };
            let last_modified = dir
                .metadata()
                .with_context(|| format!("failed to get metadata of {:?}", dir.path()))?
                .modified()
                .with_context(|| format!("failed to get modified time of {:?}", dir.path()))?;
            match latest_one.entry(dep_name.clone()) {
                Entry::Occupied(mut entry) => {
                    let (prev_hash, prev_last_modified) = entry.get_mut();
                    if last_modified > *prev_last_modified {
                        let previous_hash = prev_hash.clone();
                        *prev_hash = hash;
                        *prev_last_modified = last_modified;
                        pathbuf_to_remove.insert(
                            incremental_path
                                .join(format!("{dep_name}-{previous_hash}"))
                                .into(),
                        );
                    } else {
                        pathbuf_to_remove
                            .insert(incremental_path.join(format!("{dep_name}-{hash}")).into());
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert((hash, last_modified));
                }
            }
        }

        Ok(pathbuf_to_remove)
    }

    pub fn report(&self) -> String {
        let dep_info_files = self
            .dep_artifacts
            .iter()
            .filter(|artifact| Self::is_dep_info_file(&artifact.path))
            .count();
        let stored_fingerprint_hashes = self
            .fingerprint_dirs
            .iter()
            .filter(|dir| dir.fingerprint_hash.is_some())
            .count();
        let dep_bytes: u64 = self
            .dep_artifacts
            .iter()
            .map(|artifact| artifact.info.size)
            .sum();

        format!(
            "Beatrice Library Report:\n\
            - Fingerprint dirs on disk: {}\n\
            - Fingerprint dirs with stored hash: {}\n\
            - Deps entries on disk: {}\n\
            - Deps dep-info files kept untouched: {}\n\
            - Deps bytes on disk: {}",
            self.fingerprint_dirs.len(),
            stored_fingerprint_hashes,
            self.dep_artifacts.len(),
            dep_info_files,
            dep_bytes,
        )
    }

    fn is_dep_info_file(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "d")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_plan_cleanup_uses_exact_paths() {
        let beatrice = Beatrice {
            profile_dir: "/tmp/test".into(),
            fingerprint_dirs: vec![
                FingerprintDirectory {
                    path: PathBuf::from("/tmp/test/.fingerprint/live"),
                    fingerprint_hash: Some("hash".to_string()),
                },
                FingerprintDirectory {
                    path: PathBuf::from("/tmp/test/.fingerprint/stale"),
                    fingerprint_hash: Some("hash".to_string()),
                },
            ],
            dep_artifacts: vec![
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/live.rlib"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/live.d"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/stale.rlib"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/stale.d"),
                    info: ItemInfo { size: 10 },
                },
            ],
        };

        let live_dep_paths = HashSet::from([
            PathBuf::from("/tmp/test/deps/live.rlib"),
            PathBuf::from("/tmp/test/deps/live.d"),
        ]);
        let live_fingerprint_dirs = HashSet::from([PathBuf::from("/tmp/test/.fingerprint/live")]);
        let scan = ScanResult::from_live_sets(live_dep_paths, live_fingerprint_dirs);

        let plan = beatrice.plan_cleanup(&scan);

        assert!(
            !plan
                .deps_files
                .contains(&PathBuf::from("/tmp/test/deps/live.rlib"))
        );
        assert!(
            !plan
                .deps_files
                .contains(&PathBuf::from("/tmp/test/deps/live.d"))
        );
        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/test/deps/stale.rlib"))
        );
        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/test/deps/stale.d"))
        );
        assert!(
            !plan
                .fingerprint_dirs
                .contains(&PathBuf::from("/tmp/test/.fingerprint/live"))
        );
        assert!(
            plan.fingerprint_dirs
                .contains(&PathBuf::from("/tmp/test/.fingerprint/stale"))
        );
    }

    #[test]
    fn test_plan_cleanup_removes_stale_same_package_hashes_when_not_kept() {
        let beatrice = Beatrice {
            profile_dir: "/tmp/test".into(),
            fingerprint_dirs: vec![
                FingerprintDirectory {
                    path: PathBuf::from("/tmp/test/.fingerprint/foo-livehash"),
                    fingerprint_hash: Some("livehash".to_string()),
                },
                FingerprintDirectory {
                    path: PathBuf::from("/tmp/test/.fingerprint/foo-oldhash"),
                    fingerprint_hash: Some("oldhash".to_string()),
                },
            ],
            dep_artifacts: vec![
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libfoo-livehash.rlib"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libfoo-livehash.d"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libfoo-oldhash.rlib"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libfoo-oldhash.d"),
                    info: ItemInfo { size: 10 },
                },
            ],
        };

        let scan = ScanResult::from_live_sets(
            HashSet::from([
                PathBuf::from("/tmp/test/deps/libfoo-livehash.rlib"),
                PathBuf::from("/tmp/test/deps/libfoo-livehash.d"),
            ]),
            HashSet::from([PathBuf::from("/tmp/test/.fingerprint/foo-livehash")]),
        );

        let plan = beatrice.plan_cleanup(&scan);

        assert!(
            !plan
                .deps_files
                .contains(&PathBuf::from("/tmp/test/deps/libfoo-livehash.rlib"))
        );
        assert!(
            !plan
                .deps_files
                .contains(&PathBuf::from("/tmp/test/deps/libfoo-livehash.d"))
        );
        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/test/deps/libfoo-oldhash.rlib"))
        );
        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/test/deps/libfoo-oldhash.d"))
        );
        assert!(
            !plan
                .fingerprint_dirs
                .contains(&PathBuf::from("/tmp/test/.fingerprint/foo-livehash"))
        );
        assert!(
            plan.fingerprint_dirs
                .contains(&PathBuf::from("/tmp/test/.fingerprint/foo-oldhash"))
        );
    }

    #[test]
    fn test_plan_cleanup_removes_unkept_hashed_binary() {
        let beatrice = Beatrice {
            profile_dir: "/tmp/test".into(),
            fingerprint_dirs: Vec::new(),
            dep_artifacts: vec![DepArtifact {
                path: PathBuf::from("/tmp/test/deps/demo-deadbeef"),
                info: ItemInfo { size: 10 },
            }],
        };

        let scan = ScanResult::from_live_sets(HashSet::new(), HashSet::new());
        let plan = beatrice.plan_cleanup(&scan);

        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/test/deps/demo-deadbeef"))
        );
    }

    #[test]
    fn test_plan_cleanup_keeps_exact_dep_info_path_only_when_present() {
        let beatrice = Beatrice {
            profile_dir: "/tmp/test".into(),
            fingerprint_dirs: Vec::new(),
            dep_artifacts: vec![
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libdemo-deadbeef.rlib"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libdemo-deadbeef.d"),
                    info: ItemInfo { size: 10 },
                },
            ],
        };

        let scan = ScanResult::from_live_sets(
            HashSet::from([PathBuf::from("/tmp/test/deps/libdemo-deadbeef.rlib")]),
            HashSet::new(),
        );

        let plan = beatrice.plan_cleanup(&scan);

        assert!(
            !plan
                .deps_files
                .contains(&PathBuf::from("/tmp/test/deps/libdemo-deadbeef.rlib"))
        );
        assert!(
            plan.deps_files
                .contains(&PathBuf::from("/tmp/test/deps/libdemo-deadbeef.d"))
        );
    }

    #[test]
    fn test_report_functionality() {
        let beatrice = Beatrice {
            profile_dir: "/tmp/test".into(),
            fingerprint_dirs: vec![
                FingerprintDirectory {
                    path: PathBuf::from("/tmp/test/.fingerprint/one"),
                    fingerprint_hash: Some("hash".to_string()),
                },
                FingerprintDirectory {
                    path: PathBuf::from("/tmp/test/.fingerprint/two"),
                    fingerprint_hash: None,
                },
            ],
            dep_artifacts: vec![
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libone.rlib"),
                    info: ItemInfo { size: 10 },
                },
                DepArtifact {
                    path: PathBuf::from("/tmp/test/deps/libone.d"),
                    info: ItemInfo { size: 10 },
                },
            ],
        };

        let report = beatrice.report();
        assert!(report.contains("Fingerprint dirs on disk: 2"));
        assert!(report.contains("Fingerprint dirs with stored hash: 1"));
        assert!(report.contains("Deps entries on disk: 2"));
        assert!(report.contains("Deps dep-info files kept untouched: 1"));
        assert!(report.contains("Deps bytes on disk: 20"));
    }
}
