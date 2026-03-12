use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Normalize package/file names so Cargo package ids and artifact filenames
/// can be compared safely.
pub fn normalize_package_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Convert profile name to target directory name
/// The 'dev' profile maps to 'debug' directory, all others map directly
pub fn profile_to_dir(profile: &str) -> &str {
    if profile == "dev" { "debug" } else { profile }
}

#[derive(Default)]
pub struct RemovalStats {
    pub removed_paths: usize,
    pub reclaimed_bytes: u64,
    pub failed_paths: usize,
}

impl RemovalStats {
    pub fn merge(&mut self, other: Self) {
        self.removed_paths += other.removed_paths;
        self.reclaimed_bytes += other.reclaimed_bytes;
        self.failed_paths += other.failed_paths;
    }
}

pub fn path_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }

    let Ok(dir_iter) = fs::read_dir(path) else {
        return 0;
    };
    dir_iter
        .filter_map(|entry| entry.ok())
        .map(|entry| path_size(&entry.path()))
        .sum()
}

pub fn remove_files(paths: &HashSet<PathBuf>) -> RemovalStats {
    let mut stats = RemovalStats::default();

    for path in paths {
        let size = path_size(path);
        let removal = match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
            _ => fs::remove_file(path),
        };
        match removal {
            Ok(()) => {
                stats.removed_paths += 1;
                stats.reclaimed_bytes += size;
            }
            Err(err) => {
                stats.failed_paths += 1;
                println!("failed to remove file {}: {err}", path.display());
            }
        }
    }

    stats
}

pub fn remove_dirs(paths: &HashSet<PathBuf>) -> RemovalStats {
    let mut stats = RemovalStats::default();

    for path in paths {
        let size = path_size(path);
        match fs::remove_dir_all(path) {
            Ok(()) => {
                stats.removed_paths += 1;
                stats.reclaimed_bytes += size;
            }
            Err(err) => {
                stats.failed_paths += 1;
                println!("failed to remove directory {}: {err}", path.display());
            }
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_package_name() {
        assert_eq!(normalize_package_name("git2-curl"), "git2_curl");
        assert_eq!(normalize_package_name("git2_curl"), "git2_curl");
        assert_eq!(normalize_package_name("simple"), "simple");
    }

    #[test]
    fn test_profile_to_dir() {
        assert_eq!(profile_to_dir("dev"), "debug");
        assert_eq!(profile_to_dir("release"), "release");
        assert_eq!(profile_to_dir("custom"), "custom");
        assert_eq!(profile_to_dir("test"), "test");
    }

    #[test]
    fn test_path_size_on_missing_path() {
        assert_eq!(
            path_size(Path::new("/tmp/cargo-gc-utils-definitely-missing")),
            0
        );
    }
}
