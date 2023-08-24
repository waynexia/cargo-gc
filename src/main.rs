use std::{collections::HashSet, fs, path::PathBuf};

use cargo_metadata::MetadataCommand;
use serde::Deserialize;

type Figureprints = HashSet<(String, String)>;

struct OutputCollection {
    /// (Names, Fingerprints)
    deps_figureprints: Figureprints,
}

impl OutputCollection {
    fn from_json(json: &str) -> Self {
        let result = json
            .lines()
            .map(|raw| serde_json::from_str(raw).expect("failed to deserialize build graph json"))
            .collect::<Vec<OutputItem>>();

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
                if let Some((name, figureprint)) = extract_figureprint(&file_stem) {
                    set.insert((name.to_string(), figureprint.to_string()));
                }
            }
        }
        assert!(!set.is_empty(), "set of valid files should not be empty");
        Self {
            deps_figureprints: set,
        }
    }
}

fn extract_figureprint(file_stem: &str) -> Option<(String, String)> {
    file_stem
        .rsplit_once('-')
        .map(|(name, figureprint)| (name.to_string(), figureprint.to_string()))
}

#[derive(Deserialize, Default)]
struct OutputItem {
    filenames: Option<Vec<String>>,
}

fn get_figureprints() -> Figureprints {
    let output = std::process::Command::new("cargo")
        .args(["build", "--message-format=json"])
        .output()
        .expect("failed to execute cargo build");
    let stdout = String::from_utf8(output.stdout).expect("failed to parse stdout");
    let collection = OutputCollection::from_json(&stdout);
    collection.deps_figureprints
}

fn main() {
    let figureprints = get_figureprints();
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .expect("failed to retrieve cargo metadata");
    let target_path = metadata.target_directory;
    let targets = fs::read_dir(target_path)
        .expect("failed to read target directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect::<Vec<_>>();

    for target in targets {
        let deps_path = target.join("deps");
        let files_iter = fs::read_dir(deps_path);
        if files_iter.is_err() {
            continue;
        }
        let mut files_to_remove = HashSet::new();

        // Find the newest file for each crate
        for file in files_iter.unwrap() {
            let file = file.unwrap();
            if file.file_type().unwrap().is_dir() {
                continue;
            }

            let path = file.path();
            let ext = path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let full_file_path = path.canonicalize().unwrap().to_string_lossy().to_string();
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let (name, figureprint) = extract_figureprint(&stem)
                .expect(&format!("invalid file name: {}, files under deps should contains crate name and figureprint", stem));

            if !figureprints.contains(&(name, figureprint)) && ext != "d" {
                files_to_remove.insert(full_file_path.clone());
            }
        }

        println!("files to remove {files_to_remove:#?}");

        // Remove old files
        let mut failed = 0;
        let total = files_to_remove.len();
        for file in files_to_remove {
            if let Err(e) = fs::remove_file(file) {
                failed += 1;
                println!("failed to remove file: {}", e);
            };
        }

        println!("removed {} files from {}", total - failed, target.display());
    }
}
