use std::{
    collections::{HashMap, HashSet},
    fs,
    time::SystemTime,
};

use cargo_metadata::MetadataCommand;
use serde::Deserialize;

#[derive(Deserialize)]
struct OutputCollection {
    invocations: Vec<Invocation>,
}

impl OutputCollection {
    fn from_json(json: &str) -> Self {
        serde_json::from_str(json).expect("failed to deserialize build graph json")
    }

    fn into_hashset(self) -> HashSet<String> {
        let mut set = HashSet::new();
        for invocation in self.invocations {
            for output in invocation.outputs {
                set.insert(output);
            }
        }
        set
    }
}

#[derive(Deserialize)]
struct Invocation {
    outputs: Vec<String>,
}

fn get_output_collection() -> HashSet<String> {
    let output = std::process::Command::new("cargo")
        .args(&["build", "--build-plan", "-Z", "unstable-options"])
        .output()
        .expect("failed to execute cargo build");
    let stdout = String::from_utf8(output.stdout).expect("failed to parse stdout");
    let stderr = String::from_utf8(output.stderr).expect("failed to parse stderr");
    if !stderr.is_empty() {
        panic!("unexpected error: {}", stderr)
    }
    let collection = OutputCollection::from_json(&stdout);
    collection.into_hashset()
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
        // (crate_name, ext) => (last_modified, full_path)
        let mut newest_files = HashMap::<(String, String), (SystemTime, String)>::new();

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
            let last_modified = file.metadata().unwrap().modified().unwrap();
            let full_file_path = file
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .to_string();
            // remove tailing hash
            let crate_name = file
                .file_name()
                .into_string()
                .unwrap()
                .rsplit_once('-')
                .unwrap()
                .0
                .to_string();

            if newest_files.contains_key(&(crate_name.clone(), ext.clone())) {
                let (exist_last_modified, exist_full_path) = newest_files
                    .get(&(crate_name.clone(), ext.clone()))
                    .unwrap()
                    .clone();
                if last_modified > exist_last_modified {
                    newest_files.insert((crate_name, ext.clone()), (last_modified, full_file_path));
                    if !referenced_files.contains(&exist_full_path) && ext != "d" {
                        files_to_remove.insert(exist_full_path.clone());
                    }
                } else if !referenced_files.contains(&full_file_path) && ext != "d" {
                    files_to_remove.insert(full_file_path);
                }
            } else {
                newest_files.insert((crate_name, ext), (last_modified, full_file_path));
            }
        }

        println!("{files_to_remove:#?}");

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
