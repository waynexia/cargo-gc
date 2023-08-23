use std::{collections::HashSet, fs};

use cargo_metadata::MetadataCommand;
use serde::Deserialize;

struct OutputCollection {
    file_names: HashSet<String>,
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
                set.insert(name);
            }
        }
        assert!(!set.is_empty(), "set of valid files should not be empty");
        Self { file_names: set }
    }
}

#[derive(Deserialize, Default)]
struct OutputItem {
    filenames: Option<Vec<String>>,
}

fn get_output_collection() -> HashSet<String> {
    let output = std::process::Command::new("cargo")
        .args(["build", "--message-format=json"])
        .output()
        .expect("failed to execute cargo build");
    let stdout = String::from_utf8(output.stdout).expect("failed to parse stdout");
    let collection = OutputCollection::from_json(&stdout);
    collection.file_names
}

fn main() {
    let referenced_files = get_output_collection();
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

            let ext = file
                .path()
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let full_file_path = file
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .to_string();

            if !referenced_files.contains(&full_file_path) && ext != "d" {
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
