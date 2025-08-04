/// Normalize package name to underscore format for internal storage
/// All package names are stored in underscore format in Beatrice
pub fn normalize_package_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Convert profile name to target directory name
/// The 'dev' profile maps to 'debug' directory, all others map directly
pub fn profile_to_dir(profile: &str) -> &str {
    if profile == "dev" { "debug" } else { profile }
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
}
