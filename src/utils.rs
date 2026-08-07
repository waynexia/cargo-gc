use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Split a file stem like `libfoo-0123abcd...` (or `foo-0123abcd...d`) into
/// `(name, hash)`. Only accepts cargo's 16-hex-digit artifact hashes so that
/// unrelated names like `weird-name` are rejected.
pub fn extract_fingerprint(path: &Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_str()?;
    let (name, hash) = stem.rsplit_once('-')?;
    if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Some((name.to_string(), hash.to_string()))
    } else {
        None
    }
}

/// Convert profile name to target directory name
/// Cargo's built-in profiles map to these output directories.
pub fn profile_to_dir(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "bench" => "release",
        _ => profile,
    }
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
    fn test_extract_fingerprint() {
        assert_eq!(
            extract_fingerprint(Path::new("libfoo-0123456789abcdef.rlib")),
            Some(("libfoo".to_string(), "0123456789abcdef".to_string()))
        );
        assert_eq!(
            extract_fingerprint(Path::new("foo-0123456789abcdef.d")),
            Some(("foo".to_string(), "0123456789abcdef".to_string()))
        );
        assert_eq!(extract_fingerprint(Path::new("README")), None);
        assert_eq!(
            extract_fingerprint(Path::new("prefix-abc-not-a-hash")),
            None
        );
    }

    #[test]
    fn test_profile_to_dir() {
        assert_eq!(profile_to_dir("dev"), "debug");
        assert_eq!(profile_to_dir("test"), "debug");
        assert_eq!(profile_to_dir("bench"), "release");
        assert_eq!(profile_to_dir("release"), "release");
        assert_eq!(profile_to_dir("custom"), "custom");
    }

    #[test]
    fn test_path_size_on_missing_path() {
        assert_eq!(
            path_size(Path::new("/tmp/cargo-gc-utils-definitely-missing")),
            0
        );
    }
}
