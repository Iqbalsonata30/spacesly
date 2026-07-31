use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_READ_BYTES: u64 = 1_000_000;
const MAX_DIRECTORY_ENTRIES: usize = 1_000;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileSnapshot {
    pub content: String,
    pub version: String,
    pub root_revision: u64,
    pub encoding: TextEncoding,
    pub line_ending: LineEnding,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileWriteResult {
    pub version: String,
    pub root_revision: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TextEncoding {
    Utf8,
    Utf8Bom,
    Utf16le,
    Utf16be,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Lf,
    Crlf,
}

#[derive(Clone, Debug)]
struct RootEntry {
    path: PathBuf,
    revision: u64,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRoot {
    paths: Arc<Mutex<HashMap<String, RootEntry>>>,
}

impl WorkspaceRoot {
    pub fn home() -> Result<Self, String> {
        let path = home_dir()?
            .canonicalize()
            .map_err(|error| format!("Failed to canonicalize home directory: {error}"))?;
        let mut paths = HashMap::new();
        paths.insert(
            "workspace-personal".to_string(),
            RootEntry { path, revision: 1 },
        );
        Ok(Self {
            paths: Arc::new(Mutex::new(paths)),
        })
    }

    pub fn path(&self, workspace_id: &str) -> Result<PathBuf, String> {
        Ok(self.snapshot(workspace_id)?.0)
    }

    pub fn revision(&self, workspace_id: &str) -> Result<u64, String> {
        Ok(self.snapshot(workspace_id)?.1)
    }

    fn snapshot(&self, workspace_id: &str) -> Result<(PathBuf, u64), String> {
        let paths = self.paths.lock().map_err(|error| error.to_string())?;
        let entry = paths
            .get(workspace_id)
            .ok_or_else(|| "Workspace root is not configured.".to_string())?;
        Ok((entry.path.clone(), entry.revision))
    }

    pub fn set_path(&self, workspace_id: &str, path: PathBuf) -> Result<(), String> {
        let resolved = path
            .canonicalize()
            .map_err(|error| format!("Failed to canonicalize selected path: {error}"))?;
        if !resolved.is_dir() {
            return Err("Selected path is not a directory.".to_string());
        }

        let mut paths = self.paths.lock().map_err(|error| error.to_string())?;
        let revision = paths
            .get(workspace_id)
            .map(|entry| entry.revision.saturating_add(1))
            .unwrap_or(1);
        paths.insert(
            workspace_id.to_string(),
            RootEntry {
                path: resolved,
                revision,
            },
        );
        Ok(())
    }
}

pub fn workspace_root_path(root: &WorkspaceRoot, workspace_id: String) -> Result<String, String> {
    Ok(root.path(&workspace_id)?.to_string_lossy().to_string())
}

pub fn set_workspace_root(
    root: &WorkspaceRoot,
    workspace_id: String,
    absolute_path: String,
) -> Result<String, String> {
    let path = PathBuf::from(absolute_path);
    if !path.is_absolute() {
        return Err("Workspace root must be an absolute path.".to_string());
    }
    root.set_path(&workspace_id, path)?;
    workspace_root_path(root, workspace_id)
}

pub fn list_directory(
    root: &WorkspaceRoot,
    workspace_id: String,
    relative_path: String,
) -> Result<Vec<FileEntry>, String> {
    let root = root.path(&workspace_id)?;
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
    workspace_id: String,
    relative_path: String,
) -> Result<FileSnapshot, String> {
    let (root, root_revision) = root.snapshot(&workspace_id)?;
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
    let version = file_version(&bytes);
    let (content, encoding) = decode_text(&bytes)?;
    let line_ending = detect_line_ending(&content);
    Ok(FileSnapshot {
        content: normalize_line_endings(&content),
        version,
        root_revision,
        encoding,
        line_ending,
    })
}

pub fn write_file(
    root: &WorkspaceRoot,
    workspace_id: String,
    relative_path: String,
    content: String,
    expected_version: Option<String>,
    expected_root_revision: Option<u64>,
    encoding: TextEncoding,
    line_ending: LineEnding,
) -> Result<FileWriteResult, String> {
    write_file_authorized(
        root,
        workspace_id,
        relative_path,
        content,
        expected_version,
        expected_root_revision,
        encoding,
        line_ending,
        false,
        || Ok(()),
    )
}

pub(crate) fn write_file_authorized(
    root: &WorkspaceRoot,
    workspace_id: String,
    relative_path: String,
    content: String,
    expected_version: Option<String>,
    expected_root_revision: Option<u64>,
    encoding: TextEncoding,
    line_ending: LineEnding,
    require_secure_write: bool,
    mut authorize: impl FnMut() -> Result<(), String>,
) -> Result<FileWriteResult, String> {
    #[cfg(not(target_os = "linux"))]
    if require_secure_write {
        return Err(
            "Secure scheduler workspace writes are unavailable on this platform.".to_string(),
        );
    }
    let (root, root_revision) = root.snapshot(&workspace_id)?;
    if expected_root_revision.is_some_and(|expected| expected != root_revision) {
        return Err(
            "Workspace root changed after this document was opened. Reopen the file before saving."
                .to_string(),
        );
    }
    let path = resolve_workspace_path_for_write(&root, &relative_path)?;
    if path.is_dir() {
        return Err("Cannot write over a directory.".to_string());
    }
    if let Some(parent) = path.parent() {
        if require_secure_write && !parent.is_dir() {
            return Err(
                "Secure scheduler workspace writes require an existing parent directory."
                    .to_string(),
            );
        }
        authorize()?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create parent directory: {error}"))?;
    }
    if let Some(expected_version) = expected_version {
        let current = fs::read(&path)
            .map_err(|error| format!("Failed to verify current file before saving: {error}"))?;
        if file_version(&current) != expected_version {
            return Err("File changed on disk after it was opened. Reload or compare the file before saving.".to_string());
        }
    } else if path.exists() {
        return Err("File already exists. Open it before replacing its contents.".to_string());
    }

    let encoded = encode_text(&content, encoding, line_ending);
    if encoded.len() as u64 > MAX_READ_BYTES {
        return Err(format!(
            "File is too large to save safely ({} bytes, limit {}).",
            encoded.len(),
            MAX_READ_BYTES
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| "File has no parent directory.".to_string())?;
    let expected_parent = parent
        .canonicalize()
        .map_err(|error| format!("Failed to revalidate save directory: {error}"))?;
    if !expected_parent.starts_with(&root) {
        return Err("Save directory escapes the workspace root.".to_string());
    }
    authorize()?;
    atomic_write_workspace(
        &path,
        &encoded,
        &root,
        &expected_parent,
        require_secure_write,
    )?;
    Ok(FileWriteResult {
        version: file_version(&encoded),
        root_revision,
    })
}

fn decode_text(bytes: &[u8]) -> Result<(String, TextEncoding), String> {
    if let Some(content) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(content.to_vec())
            .map(|text| (text, TextEncoding::Utf8Bom))
            .map_err(|_| "File contains invalid UTF-8 after its BOM.".to_string());
    }
    if let Some(content) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(content, true).map(|text| (text, TextEncoding::Utf16le));
    }
    if let Some(content) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(content, false).map(|text| (text, TextEncoding::Utf16be));
    }
    if bytes.contains(&0) {
        return Err("Binary files are not supported in the editor.".to_string());
    }
    String::from_utf8(bytes.to_vec())
        .map(|text| (text, TextEncoding::Utf8))
        .map_err(|_| "File is not valid UTF-8 and has no supported encoding BOM.".to_string())
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err("UTF-16 file has an incomplete code unit.".to_string());
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&units).map_err(|_| "File contains invalid UTF-16.".to_string())
}

fn detect_line_ending(content: &str) -> LineEnding {
    if content.contains("\r\n") {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

fn encode_text(content: &str, encoding: TextEncoding, line_ending: LineEnding) -> Vec<u8> {
    let normalized = normalize_line_endings(content);
    let text = match line_ending {
        LineEnding::Lf => normalized,
        LineEnding::Crlf => normalized.replace('\n', "\r\n"),
    };
    match encoding {
        TextEncoding::Utf8 => text.into_bytes(),
        TextEncoding::Utf8Bom => [vec![0xEF, 0xBB, 0xBF], text.into_bytes()].concat(),
        TextEncoding::Utf16le => {
            let mut bytes = vec![0xFF, 0xFE];
            bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            bytes
        }
        TextEncoding::Utf16be => {
            let mut bytes = vec![0xFE, 0xFF];
            bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
            bytes
        }
    }
}

pub(crate) fn file_version(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    atomic_write_revalidated(path, content, None)
}

#[allow(unused_variables)]
fn atomic_write_workspace(
    path: &Path,
    content: &[u8],
    workspace_root: &Path,
    expected_parent: &Path,
    require_secure_write: bool,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        return atomic_write_linux(path, content, workspace_root, expected_parent, || {});
    }
    #[cfg(not(target_os = "linux"))]
    {
        if require_secure_write {
            return Err(
                "Secure scheduler workspace writes are unavailable on this platform.".to_string(),
            );
        }
        atomic_write_revalidated(path, content, Some(expected_parent))
    }
}

#[cfg(target_os = "linux")]
fn atomic_write_linux(
    path: &Path,
    content: &[u8],
    workspace_root: &Path,
    expected_parent: &Path,
    after_open: impl FnOnce(),
) -> Result<(), String> {
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    struct DirectoryFd(RawFd);
    impl Drop for DirectoryFd {
        fn drop(&mut self) {
            unsafe { libc::close(self.0) };
        }
    }

    let parent = path
        .parent()
        .ok_or_else(|| "File has no parent directory.".to_string())?;
    let parent_c = CString::new(parent.as_os_str().as_bytes())
        .map_err(|_| "Save directory contains an invalid NUL byte.".to_string())?;
    let directory_fd = unsafe {
        libc::open(
            parent_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if directory_fd < 0 {
        return Err(format!(
            "Failed to open secure save directory: {}",
            std::io::Error::last_os_error()
        ));
    }
    let directory_fd = DirectoryFd(directory_fd);
    let opened_parent = fs::canonicalize(format!("/proc/self/fd/{}", directory_fd.0))
        .map_err(|error| format!("Failed to verify secure save directory: {error}"))?;
    if opened_parent != expected_parent || !opened_parent.starts_with(workspace_root) {
        return Err("Save directory ancestry changed during the write.".to_string());
    }

    let file_name = path
        .file_name()
        .ok_or_else(|| "File has no file name.".to_string())?;
    let file_name = CString::new(file_name.as_bytes())
        .map_err(|_| "File name contains an invalid NUL byte.".to_string())?;
    let mut target_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let original_mode = if unsafe {
        libc::fstatat(
            directory_fd.0,
            file_name.as_ptr(),
            target_stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == 0
    {
        let stat = unsafe { target_stat.assume_init() };
        (stat.st_mode & libc::S_IFMT == libc::S_IFREG).then_some(stat.st_mode)
    } else {
        None
    };

    after_open();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_name = CString::new(format!(
        ".spacesly-save-{}-{timestamp}-{sequence}.tmp",
        std::process::id()
    ))
    .expect("generated temporary file name cannot contain NUL");
    let temp_fd = unsafe {
        libc::openat(
            directory_fd.0,
            temp_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if temp_fd < 0 {
        return Err(format!(
            "Failed to create secure temporary save file: {}",
            std::io::Error::last_os_error()
        ));
    }

    let result = (|| {
        let mut file = unsafe { File::from_raw_fd(temp_fd) };
        if let Some(mode) = original_mode {
            file.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|error| format!("Failed to preserve file permissions: {error}"))?;
        }
        file.write_all(content)
            .map_err(|error| format!("Failed to write temporary save file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Failed to flush temporary save file: {error}"))?;
        let renamed = unsafe {
            libc::renameat(
                directory_fd.0,
                temp_name.as_ptr(),
                directory_fd.0,
                file_name.as_ptr(),
            )
        };
        if renamed < 0 {
            return Err(format!(
                "Failed to atomically replace file: {}",
                std::io::Error::last_os_error()
            ));
        }
        if unsafe { libc::fsync(directory_fd.0) } < 0 {
            return Err(format!(
                "Failed to flush save directory: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    })();

    if result.is_err() {
        unsafe { libc::unlinkat(directory_fd.0, temp_name.as_ptr(), 0) };
    }
    result
}

fn atomic_write_revalidated(
    path: &Path,
    content: &[u8],
    expected_parent: Option<&Path>,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "File has no parent directory.".to_string())?;
    let revalidate_parent = || -> Result<(), String> {
        if let Some(expected) = expected_parent {
            let current = parent
                .canonicalize()
                .map_err(|error| format!("Failed to revalidate save directory: {error}"))?;
            if current != expected {
                return Err("Save directory ancestry changed during the write.".to_string());
            }
        }
        Ok(())
    };
    revalidate_parent()?;
    let original_permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(
        ".spacesly-save-{}-{timestamp}-{sequence}.tmp",
        std::process::id()
    ));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| format!("Failed to create temporary save file: {error}"))?;
        if let Some(permissions) = original_permissions {
            file.set_permissions(permissions)
                .map_err(|error| format!("Failed to preserve file permissions: {error}"))?;
        }
        file.write_all(content)
            .map_err(|error| format!("Failed to write temporary save file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Failed to flush temporary save file: {error}"))?;
        revalidate_parent()?;
        fs::rename(&temp_path, path)
            .map_err(|error| format!("Failed to atomically replace file: {error}"))
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "Cannot resolve home directory.".to_string())
}

fn validate_relative_path(relative_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative_path);
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
    use super::{
        list_directory, read_file_at_root, validate_relative_path, write_file, LineEnding,
        TextEncoding, WorkspaceRoot,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(target_os = "linux")]
    #[test]
    fn atomic_write_stays_in_opened_directory_when_parent_is_replaced_by_symlink() {
        use std::os::unix::fs::symlink;

        let base = tempfile::tempdir().expect("temporary workspace");
        let workspace = base.path().join("workspace");
        let inside = workspace.join("inside");
        let moved = workspace.join("moved");
        let outside = base.path().join("outside");
        fs::create_dir_all(&inside).expect("inside directory");
        fs::create_dir_all(&outside).expect("outside directory");

        super::atomic_write_linux(
            &inside.join("result.txt"),
            b"contained",
            &workspace.canonicalize().unwrap(),
            &inside.canonicalize().unwrap(),
            || {
                fs::rename(&inside, &moved).expect("move opened parent");
                symlink(&outside, &inside).expect("replace parent with outside symlink");
            },
        )
        .expect("directory-relative write succeeds in the opened directory");

        assert!(!outside.join("result.txt").exists());
        assert_eq!(fs::read(moved.join("result.txt")).unwrap(), b"contained");
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("src/../../secret").is_err());
        assert!(validate_relative_path("/home/user/secret").is_err());
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

    #[test]
    fn file_listing_uses_workspace_specific_root() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("spacesly-workspace-roots-{suffix}"));
        let first = base.join("first");
        let second = base.join("second");
        fs::create_dir_all(&first).expect("first workspace should be created");
        fs::create_dir_all(&second).expect("second workspace should be created");
        fs::write(first.join("first.txt"), "first").expect("first file should be written");
        fs::write(second.join("second.txt"), "second").expect("second file should be written");

        let root = WorkspaceRoot::home().expect("workspace root should initialize");
        root.set_path("workspace-one", first)
            .expect("first root should set");
        root.set_path("workspace-two", second)
            .expect("second root should set");

        let first_entries = list_directory(&root, "workspace-one".to_string(), "".to_string())
            .expect("first workspace should list");
        let second_entries = list_directory(&root, "workspace-two".to_string(), "".to_string())
            .expect("second workspace should list");

        assert!(first_entries.iter().any(|entry| entry.name == "first.txt"));
        assert!(!first_entries.iter().any(|entry| entry.name == "second.txt"));
        assert!(second_entries
            .iter()
            .any(|entry| entry.name == "second.txt"));
        assert!(!second_entries.iter().any(|entry| entry.name == "first.txt"));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn write_rejects_stale_file_versions() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("spacesly-file-version-{suffix}"));
        fs::create_dir_all(&base).expect("workspace should be created");
        fs::write(base.join("main.txt"), "first").expect("file should be created");
        let root = WorkspaceRoot::home().expect("workspace root should initialize");
        root.set_path("workspace", base.clone())
            .expect("workspace root should set");
        let snapshot = read_file_at_root(&root, "workspace".to_string(), "main.txt".to_string())
            .expect("file should read");
        fs::write(base.join("main.txt"), "external").expect("external edit should write");

        let result = write_file(
            &root,
            "workspace".to_string(),
            "main.txt".to_string(),
            "editor".to_string(),
            Some(snapshot.version),
            Some(snapshot.root_revision),
            TextEncoding::Utf8,
            LineEnding::Lf,
        );

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(base.join("main.txt")).unwrap(),
            "external"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn write_rejects_stale_root_revisions() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("spacesly-root-version-{suffix}"));
        let first = base.join("first");
        let second = base.join("second");
        fs::create_dir_all(&first).expect("first workspace should be created");
        fs::create_dir_all(&second).expect("second workspace should be created");
        fs::write(first.join("main.txt"), "first").expect("first file should be created");
        let root = WorkspaceRoot::home().expect("workspace root should initialize");
        root.set_path("workspace", first.clone())
            .expect("first root should set");
        let snapshot = read_file_at_root(&root, "workspace".to_string(), "main.txt".to_string())
            .expect("file should read");
        root.set_path("workspace", second.clone())
            .expect("second root should set");

        let result = write_file(
            &root,
            "workspace".to_string(),
            "main.txt".to_string(),
            "editor".to_string(),
            None,
            Some(snapshot.root_revision),
            TextEncoding::Utf8,
            LineEnding::Lf,
        );

        assert!(result.is_err());
        assert!(!second.join("main.txt").exists());
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn create_does_not_replace_an_existing_file() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("spacesly-create-conflict-{suffix}"));
        fs::create_dir_all(&base).expect("workspace should be created");
        fs::write(base.join("main.txt"), "existing").expect("file should be created");
        let root = WorkspaceRoot::home().expect("workspace root should initialize");
        root.set_path("workspace", base.clone())
            .expect("workspace root should set");

        let result = write_file(
            &root,
            "workspace".to_string(),
            "main.txt".to_string(),
            String::new(),
            None,
            None,
            TextEncoding::Utf8,
            LineEnding::Lf,
        );

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(base.join("main.txt")).unwrap(),
            "existing"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn preserves_utf16_and_crlf_through_editor_round_trip() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("spacesly-encoding-{suffix}"));
        fs::create_dir_all(&base).expect("workspace should be created");
        let root = WorkspaceRoot::home().expect("workspace root should initialize");
        root.set_path("workspace", base.clone())
            .expect("workspace root should set");

        write_file(
            &root,
            "workspace".to_string(),
            "main.txt".to_string(),
            "first\nsecond\n".to_string(),
            None,
            None,
            TextEncoding::Utf16le,
            LineEnding::Crlf,
        )
        .expect("encoded file should save");
        let bytes = fs::read(base.join("main.txt")).expect("encoded file should read");
        let snapshot = read_file_at_root(&root, "workspace".to_string(), "main.txt".to_string())
            .expect("encoded file should decode");

        assert!(bytes.starts_with(&[0xFF, 0xFE]));
        assert_eq!(snapshot.encoding, TextEncoding::Utf16le);
        assert_eq!(snapshot.line_ending, LineEnding::Crlf);
        assert_eq!(snapshot.content, "first\nsecond\n");
        let _ = fs::remove_dir_all(base);
    }
}
