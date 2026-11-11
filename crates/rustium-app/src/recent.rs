//! The list of recently opened files shown on the start screen, kept in the local data directory.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::settings::data_dir;

/// Most files remembered.
pub(crate) const MAX_RECENT: usize = 8;

/// One remembered file and the name of its Scene's root Node, shown as secondary text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecentFile {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
}

/// A remembered file together with whether it was still on disk when last checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentItem {
    pub(crate) file: RecentFile,
    pub(crate) exists: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecentList {
    files: Vec<RecentFile>,
}

fn list_path() -> PathBuf {
    data_dir().join("recent.toml")
}

/// Reads the saved list; a missing or unreadable file gives an empty list.
pub(crate) fn load() -> Vec<RecentFile> {
    let Ok(text) = fs::read_to_string(list_path()) else {
        return Vec::new();
    };
    let mut files = match toml::from_str::<RecentList>(&text) {
        Ok(list) => list.files,
        Err(error) => {
            warn!(%error, "the recent files list is invalid, starting empty");
            Vec::new()
        }
    };
    files.truncate(MAX_RECENT);
    files
}

/// Writes the list; failures are logged because losing it must not interrupt editing.
pub(crate) fn save(files: &[RecentFile]) {
    let path = list_path();
    let list = RecentList { files: files.to_vec() };
    let result = toml::to_string_pretty(&list)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::write(&path, text).map_err(|error| error.to_string())
        });
    if let Err(error) = result {
        warn!(path = %path.display(), %error, "cannot save the recent files list");
    }
}

/// Puts `path` first, drops an older entry for the same file and keeps at most [`MAX_RECENT`].
pub(crate) fn remember(files: &mut Vec<RecentFile>, path: &Path, name: &str) {
    files.retain(|file| file.path != path);
    files.insert(
        0,
        RecentFile {
            path: path.to_path_buf(),
            name: name.to_owned(),
        },
    );
    files.truncate(MAX_RECENT);
}

/// Pairs each file with whether it exists.
pub(crate) fn check(files: Vec<RecentFile>) -> Vec<RecentItem> {
    files
        .into_iter()
        .map(|file| RecentItem {
            exists: file.path.is_file(),
            file,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_file_comes_first_and_duplicates_collapse() {
        let mut files = Vec::new();
        for index in 0..12 {
            remember(&mut files, Path::new(&format!("/models/{index}.ruv")), "Scene");
        }
        remember(&mut files, Path::new("/models/9.ruv"), "Renamed");
        assert_eq!(files.len(), MAX_RECENT);
        assert_eq!(files[0].path, Path::new("/models/9.ruv"));
        assert_eq!(files[0].name, "Renamed");
        assert_eq!(
            files
                .iter()
                .filter(|file| file.path == Path::new("/models/9.ruv"))
                .count(),
            1
        );
    }

    #[test]
    fn the_list_round_trips_through_toml() {
        let list = RecentList {
            files: vec![RecentFile {
                path: PathBuf::from("/a/b.ruv"),
                name: "Cottage".to_owned(),
            }],
        };
        let text = toml::to_string_pretty(&list).unwrap();
        assert_eq!(toml::from_str::<RecentList>(&text).unwrap().files, list.files);
    }

    #[test]
    fn missing_files_are_marked() {
        let items = check(vec![RecentFile {
            path: PathBuf::from("/definitely/not/here.ruv"),
            name: String::new(),
        }]);
        assert!(!items[0].exists);
    }
}
