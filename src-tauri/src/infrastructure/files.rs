use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_READ_BYTES: u64 = 1_000_000;
const MAX_DIRECTORY_ENTRIES: usize = 1_000;

#[derive(Clone, Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRoot {
    path: Arc<Mutex<PathBuf>>,
}

impl WorkspaceRoot {
    pub fn home() -> Result<Self, String> {
        let path = home_dir()?
            .canonicalize()
            .map_err(|error| format!("Failed to canonicalize home directory: {error}"))?;
        Ok(Self{
            path: Arc::new(Mutex::new(path)),
        })
    }

    pub fn path(&self) -> Result<PathBuf, String> {
        self.path
            .lock()
            .map_err(|error| error.to_string())
            .map(|path| path.clone())
    }

    pub fn set_path(&self, path: PathBuf) -> Result<(), String> {
        let resolved = path
            .canonicalize()
            .map_err(|error| format!("Failed to canonicalize selected path: {error}"))?;
        if !resolved.is_dir() {
            return Err("Selected path is not a directory.".to_string());
        }

        *self.path.lock().map_err(|error| error.to_string())? = resolved;
        Ok(())
    }
}

pub fn workspace_root_path(root: &WorkspaceRoot, _workspace_id: String) -> Result<String, String> {
    Ok(root.path()?.to_string_lossy().to_string())
}

pub fn set_workspace_root(
    root: &WorkspaceRoot,
    absolute_path: String,
) -> Result<String, String> {
    let path = PathBuf::from(absolute_path);
    if !path.is_absolute() {
        return Err("Workspace root must be an absolute path.".to_string());
    }
    root.set_path( path)?;
    workspace_root_path(root, "workspace-personal".to_string())
}

pub fn list_directory(
    root: &WorkspaceRoot,
    _workspace_id: String,
    relative_path: String,
) -> Result<Vec<FileEntry>, String> {
    let root = root.path()?;
    let directory = resolve_workspace_path(&root, &relative_path)?;
    if !directory.is_dir() {
        return Err("Path is not a directory.".to_string());
    }

    let mut entries = fs::read_dir(&directory)
        .map_err(|error| format!("Failed to read directory: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| file_entry(&root, entry.path()).ok())
        .collect::<Vec<_>>();

    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    entries.truncate(MAX_DIRECTORY_ENTRIES);
    Ok(entries)
}

pub fn read_file_at_root(
    root: &WorkspaceRoot,
    _workspace_id: String,
    relative_path: String,
) -> Result<String, String> {
    let root = root.path()?;
    let path = resolve_workspace_path(&root, &relative_path)?;
    let metadata =
        fs::metadata(&path).map_err(|error| format!("Failed to read file metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("Path is not a file.".to_string());
    }
    if metadata.len() > MAX_READ_BYTES {
        return Err(format!(
            "File is too large to open safely in the editor ({} bytes, limit {}).",
            metadata.len(),
            MAX_READ_BYTES
        ));
    }

    let bytes = fs::read(&path).map_err(|error| format!("Failed to read file: {error}"))?;
    if bytes.contains(&0) {
        return Err("Binary files are not supported in the editor.".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8.".to_string())
}

pub fn write_file(
    root: &WorkspaceRoot,
    _workspace_id: String,
    relative_path: String,
    content: String,
) -> Result<(), String> {
    let root = root.path()?;
    let path = resolve_workspace_path_for_write(&root, &relative_path)?;
    if path.is_dir() {
        return Err("Cannot write over a directory.".to_string());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create parent directory: {error}"))?;
    }
    fs::write(&path, content).map_err(|error| format!("Failed to write file: {error}"))
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "Cannot resolve home directory.".to_string())
}

fn validate_relative_path(relative_path: &str) -> Result<PathBuf, String> {
    let trimmed = relative_path.trim_matches('/');
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err("Absolute paths are not allowed.".to_string());
    }

    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("Path traversal is not allowed.".to_string());
            }
        }
    }
    Ok(clean)
}

fn resolve_workspace_path(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let candidate = root.join(validate_relative_path(relative_path)?);
    let resolved = candidate
        .canonicalize()
        .map_err(|error| format!("Path does not exist: {error}"))?;
    if !resolved.starts_with(root) {
        return Err("Path escapes the workspace root.".to_string());
    }
    Ok(resolved)
}

fn resolve_workspace_path_for_write(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let relative = validate_relative_path(relative_path)?;
    if relative.as_os_str().is_empty() {
        return Err("File path is required.".to_string());
    }
    let path = root.join(relative);
    if path.exists() {
        let resolved = path
            .canonicalize()
            .map_err(|error| format!("Failed to resolve file path: {error}"))?;
        if !resolved.starts_with(root) {
            return Err("Path escapes the workspace root.".to_string());
        }
        return Ok(path);
    }
    let parent = path.parent().unwrap_or(root);
    let resolved_parent = if parent.exists() {
        parent
            .canonicalize()
            .map_err(|error| format!("Failed to resolve parent directory: {error}"))?
    } else {
        let existing_parent = parent
            .ancestors()
            .find(|ancestor| ancestor.exists())
            .ok_or_else(|| "No existing parent directory found.".to_string())?;
        existing_parent
            .canonicalize()
            .map_err(|error| format!("Failed to resolve parent directory: {error}"))?
    };
    if !resolved_parent.starts_with(root) {
        return Err("Path escapes the workspace root.".to_string());
    }
    Ok(path)
}

fn file_entry(root: &Path, path: PathBuf) -> Result<FileEntry, String> {
    let metadata =
        fs::metadata(&path).map_err(|error| format!("Failed to read file metadata: {error}"))?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Path escapes the workspace root.".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Invalid file name.".to_string())?
        .to_string();

    Ok(FileEntry {
        name,
        path: relative,
        is_dir: metadata.is_dir(),
        size: metadata.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::validate_relative_path;

    #[test]
    fn rejects_parent_traversal() {
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("src/../../secret").is_err());
    }

    #[test]
    fn normalizes_current_directory_segments() {
        assert_eq!(
            validate_relative_path("./src/./main.rs")
                .unwrap()
                .to_string_lossy(),
            "src/main.rs"
        );
    }
}
