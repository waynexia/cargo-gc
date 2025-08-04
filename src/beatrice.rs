use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::SystemTime;

use anyhow::{Context, Result};
use cargo_metadata::camino::Utf8PathBuf;

use crate::extract_fingerprint;
use crate::utils::normalize_package_name;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ItemInfo {
    pub last_modified: SystemTime,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct FingerprintInfo {
    pub freshness: UnitFreshness,
}

#[derive(Debug, Clone)]
pub enum UnitFreshness {
    Fresh,
    Dirty(String), // reason for being dirty
    Unknown,
}

pub struct Beatrice {
    profile_dir: Utf8PathBuf,
    /// Nested HashMap for .fingerprint directory: name -> (metadata hash -> FingerprintInfo)
    #[allow(dead_code)]
    pub fingerprint_library: HashMap<String, HashMap<String, FingerprintInfo>>,
    /// Nested HashMap for deps directory: name -> (hash -> ItemInfo)
    #[allow(dead_code)]
    pub deps_library: HashMap<String, HashMap<String, ItemInfo>>,
}

impl Beatrice {
    pub fn open(profile_dir: Utf8PathBuf) -> Self {
        Self {
            profile_dir,
            fingerprint_library: HashMap::new(),
            deps_library: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn load_library(&mut self) -> Result<()> {
        self.fingerprint_library.clear();
        self.deps_library.clear();

        // Scan .fingerprint subdirectory
        let fingerprint_path = self.profile_dir.join(".fingerprint");
        if fingerprint_path.exists() {
            Self::scan_fingerprint_directory(&fingerprint_path, &mut self.fingerprint_library)?;
        }

        // Scan deps subdirectory
        let deps_path = self.profile_dir.join("deps");
        if deps_path.exists() {
            Self::scan_deps_directory(&deps_path, &mut self.deps_library)?;
        }

        Ok(())
    }

    /// Scan the fingerprint directory and populate the fingerprint library with item information.
    /// Normalizes package names from dash format (used in .fingerprint) to underscore format for storage.
    fn scan_fingerprint_directory(
        dir_path: &Utf8PathBuf,
        target_library: &mut HashMap<String, HashMap<String, FingerprintInfo>>,
    ) -> Result<()> {
        let dir_iter = fs::read_dir(dir_path)
            .with_context(|| format!("failed to read directory: {dir_path:?}"))?;

        for entry in dir_iter {
            let entry = entry.with_context(|| format!("failed to read entry in {dir_path:?}"))?;
            let entry_name = entry.file_name().to_string_lossy().to_string();

            // Extract name and hash from the entry name
            let Some((name, hash)) = extract_fingerprint(&entry_name) else {
                continue;
            };

            // Normalize package name to underscore format for internal storage
            let normalized_name = normalize_package_name(&name);

            let fingerprint_info = FingerprintInfo {
                freshness: UnitFreshness::Unknown,
            };

            // Insert into the nested HashMap structure using normalized name
            target_library
                .entry(normalized_name)
                .or_default()
                .insert(hash, fingerprint_info);
        }

        Ok(())
    }

    /// Scan the deps directory and populate the deps library with item information.
    /// Normalizes package names to underscore format for consistent storage.
    fn scan_deps_directory(
        dir_path: &Utf8PathBuf,
        target_library: &mut HashMap<String, HashMap<String, ItemInfo>>,
    ) -> Result<()> {
        let dir_iter = fs::read_dir(dir_path)
            .with_context(|| format!("failed to read directory: {dir_path:?}"))?;

        for entry in dir_iter {
            let entry = entry.with_context(|| format!("failed to read entry in {dir_path:?}"))?;
            let entry_path = entry.path();
            let entry_name = entry.file_name().to_string_lossy().to_string();

            // Extract name and hash from the entry name
            let Some((name, hash)) = extract_fingerprint(&entry_name) else {
                continue;
            };

            // Normalize package name to underscore format for internal storage
            let normalized_name = normalize_package_name(&name);

            // Get metadata for the entry
            let metadata = entry
                .metadata()
                .with_context(|| format!("failed to get metadata of {:?}", entry_path))?;

            let last_modified = metadata
                .modified()
                .with_context(|| format!("failed to get modified time of {:?}", entry_path))?;

            // Calculate size
            let size = if metadata.is_dir() {
                Self::calculate_dir_size(&entry_path)?
            } else {
                metadata.len()
            };

            let item_info = ItemInfo {
                last_modified,
                size,
            };

            // Insert into the nested HashMap structure using normalized name
            target_library
                .entry(normalized_name)
                .or_default()
                .insert(hash, item_info);
        }

        Ok(())
    }

    fn calculate_dir_size(dir_path: &std::path::Path) -> Result<u64> {
        let mut total_size = 0;

        fn visit_dir(dir: &std::path::Path, total: &mut u64) -> Result<()> {
            let dir_iter =
                fs::read_dir(dir).with_context(|| format!("failed to read directory: {dir:?}"))?;

            for entry in dir_iter {
                let entry = entry.with_context(|| format!("failed to read entry in {dir:?}"))?;
                let entry_path = entry.path();
                let metadata = entry
                    .metadata()
                    .with_context(|| format!("failed to get metadata of {:?}", entry_path))?;

                if metadata.is_dir() {
                    visit_dir(&entry_path, total)?;
                } else {
                    *total += metadata.len();
                }
            }
            Ok(())
        }

        visit_dir(dir_path, &mut total_size)?;
        Ok(total_size)
    }

    pub fn load_incremental(&mut self) -> Result<HashSet<String>> {
        let incremental_path = self.profile_dir.join("incremental");
        if !incremental_path.exists() {
            return Ok(HashSet::new());
        }

        let mut pathbuf_to_remove = HashSet::new();
        let mut latest_one: HashMap<String, (String, SystemTime)> = HashMap::new();

        // walk the first level of the incremental directory
        let dir_iter = fs::read_dir(incremental_path.clone()).with_context(|| {
            format!("failed to read incremental directory: {incremental_path:?}")
        })?;
        for dir in dir_iter {
            let dir = dir.with_context(|| format!("failed to read dir in {incremental_path:?}"))?;
            // only handle dir
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
            // get the last modified time of the dir
            let last_modified = dir
                .metadata()
                .with_context(|| format!("failed to get metadata of {:?}", dir.path()))?
                .modified()
                .with_context(|| format!("failed to get modified time of {:?}", dir.path()))?;
            // update the latest one
            match latest_one.entry(dep_name.clone()) {
                Entry::Occupied(mut entry) => {
                    let (prev_hash, prev_last_modified) = entry.get_mut();
                    if last_modified > *prev_last_modified {
                        *prev_hash = hash;
                        *prev_last_modified = last_modified;
                        pathbuf_to_remove
                            .insert(incremental_path.join(format!("{dep_name}-{prev_hash}")));
                    } else {
                        pathbuf_to_remove
                            .insert(incremental_path.join(format!("{dep_name}-{hash}")));
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert((hash, last_modified));
                }
            }
        }

        let to_remove = pathbuf_to_remove
            .into_iter()
            .map(|p| {
                Ok(p.canonicalize_utf8()
                    .with_context(|| format!("cannot canonicalize path {p:?}"))?
                    .to_string())
            })
            .collect::<Result<HashSet<_>>>()?;

        Ok(to_remove)
    }

    /// Update the freshness of a specific fingerprint
    /// Works with normalized (underscore) package names
    pub fn update_fingerprint_freshness(
        &mut self,
        name: &str,
        hash: &str,
        freshness: UnitFreshness,
    ) {
        let normalized_name = normalize_package_name(name);

        if let Some(hash_map) = self.fingerprint_library.get_mut(&normalized_name)
            && let Some(fingerprint_info) = hash_map.get_mut(hash)
        {
            fingerprint_info.freshness = freshness;
        }
    }

    /// Get the freshness of a specific fingerprint
    /// Works with normalized (underscore) package names
    pub fn get_fingerprint_freshness(&self, name: &str, hash: &str) -> Option<&UnitFreshness> {
        let normalized_name = normalize_package_name(name);

        self.fingerprint_library
            .get(&normalized_name)
            .and_then(|hash_map| hash_map.get(hash))
            .map(|fingerprint_info| &fingerprint_info.freshness)
    }

    /// Check if a package exists in the fingerprint library
    /// Works with normalized (underscore) package names
    pub fn has_package(&self, name: &str) -> bool {
        let normalized_name = normalize_package_name(name);
        self.fingerprint_library.contains_key(&normalized_name)
    }

    /// Get deps info for a package
    /// Works with normalized (underscore) package names
    pub fn get_deps_info(&self, name: &str, hash: &str) -> Option<&ItemInfo> {
        let normalized_name = normalize_package_name(name);

        self.deps_library
            .get(&normalized_name)
            .and_then(|hash_map| hash_map.get(hash))
    }

    /// Generate a report showing fingerprint and deps correspondence
    pub fn report(&self) -> String {
        let mut fresh_count = 0;
        let mut dirty_count = 0;
        let mut unknown_count = 0;

        let mut fresh_with_deps = 0;
        let mut dirty_with_deps = 0;
        let mut unknown_with_deps = 0;
        let mut fresh_without_deps = 0;
        let mut dirty_without_deps = 0;
        let mut unknown_without_deps = 0;

        // Analyze fingerprints and their correspondence with deps
        for (package_name, hash_map) in &self.fingerprint_library {
            for (hash, fingerprint_info) in hash_map {
                match &fingerprint_info.freshness {
                    UnitFreshness::Fresh => {
                        fresh_count += 1;
                        if self
                            .deps_library
                            .get(package_name)
                            .and_then(|deps| deps.get(hash))
                            .is_some()
                        {
                            fresh_with_deps += 1;
                        } else {
                            fresh_without_deps += 1;
                        }
                    }
                    UnitFreshness::Dirty(_) => {
                        dirty_count += 1;
                        if self
                            .deps_library
                            .get(package_name)
                            .and_then(|deps| deps.get(hash))
                            .is_some()
                        {
                            dirty_with_deps += 1;
                        } else {
                            dirty_without_deps += 1;
                        }
                    }
                    UnitFreshness::Unknown => {
                        unknown_count += 1;
                        if self
                            .deps_library
                            .get(package_name)
                            .and_then(|deps| deps.get(hash))
                            .is_some()
                        {
                            unknown_with_deps += 1;
                        } else {
                            unknown_without_deps += 1;
                        }
                    }
                }
            }
        }

        // Count deps items that don't have corresponding fingerprints
        let mut deps_without_fingerprints = 0;
        for (package_name, hash_map) in &self.deps_library {
            for hash in hash_map.keys() {
                if !self
                    .fingerprint_library
                    .get(package_name)
                    .map(|fp_map| fp_map.contains_key(hash))
                    .unwrap_or(false)
                {
                    deps_without_fingerprints += 1;
                }
            }
        }

        format!(
            "Beatrice Report:\n\
            \n\
            Fingerprint Analysis:\n\
            - Fresh: {} (with deps: {}, without deps: {})\n\
            - Dirty: {} (with deps: {}, without deps: {})\n\
            - Unknown: {} (with deps: {}, without deps: {})\n\
            - Total fingerprints: {}\n\
            \n\
            Correspondence Analysis:\n\
            - Deps items without fingerprints: {}\n\
            - Total deps items: {}",
            fresh_count,
            fresh_with_deps,
            fresh_without_deps,
            dirty_count,
            dirty_with_deps,
            dirty_without_deps,
            unknown_count,
            unknown_with_deps,
            unknown_without_deps,
            fresh_count + dirty_count + unknown_count,
            deps_without_fingerprints,
            self.deps_library.values().map(|m| m.len()).sum::<usize>()
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_report_functionality() {
        let mut beatrice = Beatrice::open("/tmp/test".into());

        // Setup some test data
        let mut fingerprint_map = HashMap::new();
        fingerprint_map.insert(
            "hash1".to_string(),
            FingerprintInfo {
                freshness: UnitFreshness::Fresh,
            },
        );
        fingerprint_map.insert(
            "hash2".to_string(),
            FingerprintInfo {
                freshness: UnitFreshness::Dirty("test reason".to_string()),
            },
        );
        fingerprint_map.insert(
            "hash3".to_string(),
            FingerprintInfo {
                freshness: UnitFreshness::Unknown,
            },
        );
        beatrice
            .fingerprint_library
            .insert("test_package".to_string(), fingerprint_map);

        let mut deps_map = HashMap::new();
        deps_map.insert(
            "hash1".to_string(),
            ItemInfo {
                last_modified: std::time::SystemTime::now(),
                size: 1024,
            },
        );
        deps_map.insert(
            "hash4".to_string(),
            ItemInfo {
                last_modified: std::time::SystemTime::now(),
                size: 2048,
            },
        );
        beatrice
            .deps_library
            .insert("test_package".to_string(), deps_map);

        let report = beatrice.report();

        // Basic checks that the report contains expected information
        assert!(report.contains("Fresh: 1"));
        assert!(report.contains("Dirty: 1"));
        assert!(report.contains("Unknown: 1"));
        assert!(report.contains("Deps items without fingerprints: 1"));
        assert!(report.contains("Total deps items: 2"));

        println!("Report:\n{}", report);
    }
}
