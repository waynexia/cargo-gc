use std::{
    collections::{HashMap, HashSet},
    fs,
    time::SystemTime,
};

use cargo_metadata::MetadataCommand;

fn main() {
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
                    files_to_remove.insert(exist_full_path);
                    newest_files.insert((crate_name, ext), (last_modified, full_file_path));
                } else {
                    files_to_remove.insert(exist_full_path);
                }
            } else {
                newest_files.insert((crate_name, ext), (last_modified, full_file_path));
            }
        }

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
