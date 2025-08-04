/// Normalize package name to underscore format for internal storage
/// All package names are stored in underscore format in Beatrice
pub fn normalize_package_name(name: &str) -> String {
    name.replace('-', "_")
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
}
