use cargo_metadata::camino::Utf8PathBuf;
use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    fs,
    time::SystemTime,
};

use anyhow::{Context, Result};
use cargo::core::compiler::Metadata;
use cargo_metadata::semver::Version;

use crate::extract_fingerprint;

pub struct Beatrice {
    profile_dir: Utf8PathBuf,
    #[allow(dead_code)]
    library: HashMap<Item, Vec<Metadata>>,
}

impl Beatrice {
    pub fn open(profile_dir: Utf8PathBuf) -> Self {
        Self {
            profile_dir,
            library: HashMap::new(),
        }
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
}

#[derive(Debug, Hash)]
struct Item {
    name: String,
    version: Version,
}
