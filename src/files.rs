use std::{
    fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

/// Reads one JSON value from a caller-owned path.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

/// Atomically replaces one formatted JSON value, creating its parent directory when needed.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;

        let content = format!("{}\n", serde_json::to_string_pretty(value)?);
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "JSON path has no file name")
        })?;
        for attempt in 0..100 {
            let temporary = parent.join(format!(
                ".{}.{}.{attempt}.tmp",
                name.to_string_lossy(),
                std::process::id()
            ));
            let mut file = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            if let Err(error) = file
                .write_all(content.as_bytes())
                .and_then(|_| file.sync_all())
            {
                drop(file);
                let _ = fs::remove_file(&temporary);
                return Err(error.into());
            }
            drop(file);
            fs::rename(temporary, path)?;
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique temporary JSON file",
        )
        .into());
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "JSON path has no parent directory",
    )
    .into())
}

/// Lists regular JSON files in a directory in a stable order.
pub fn json_files(directory: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_json_replaces_the_complete_value() {
        let directory =
            std::env::temp_dir().join(format!("sigmarift-files-{}", std::process::id()));
        let path = directory.join("state.json");

        write_json(&path, &serde_json::json!({ "version": 1 })).unwrap();
        write_json(&path, &serde_json::json!({ "version": 2 })).unwrap();

        let stored: serde_json::Value = read_json(&path).unwrap();
        assert_eq!(stored["version"], 2);
        assert!(
            json_files(&directory)
                .unwrap()
                .iter()
                .all(|file| file == &path)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
