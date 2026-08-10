use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::files::WorkspaceRoot;
use super::global_environment::{inject_global_environment, redact_global_environment_values};

const GIT_STATUS_CACHE_TTL: Duration = Duration::from_millis(500);

struct CachedGitStatus {
    refreshed_at: Instant,
    status: GitStatus,
}

struct GitStatusCache {
    state: Mutex<GitStatusCacheState>,
    ready: Condvar,
}

#[derive(Default)]
struct GitStatusCacheState {
    entries: HashMap<PathBuf, CachedGitStatus>,
    in_flight: HashSet<PathBuf>,
}

static GIT_STATUS_CACHE: OnceLock<GitStatusCache> = OnceLock::new();

static GIT_EXECUTABLE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

/// Resolves Git from the current environment and common package-manager profiles.
/// Successful paths are cached only while they remain executable; failures are
/// retried so installing Git does not require restarting Spacesly.
pub(crate) fn git_executable() -> Result<PathBuf, String> {
    let cache = GIT_EXECUTABLE.get_or_init(|| Mutex::new(None));
    let mut cached = cache.lock().map_err(|error| error.to_string())?;
    if cached.as_ref().is_some_and(|path| is_executable_file(path)) {
        return Ok(cached.as_ref().expect("executable was checked").clone());
    }
    *cached = resolve_git_executable();
    cached.clone().ok_or_else(|| {
        "git executable was not found on PATH or common installation locations. Install git or make it available to the application.".to_string()
    })
}

fn resolve_git_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("git");
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.push(home.join(".nix-profile").join("bin").join("git"));
        candidates.push(home.join(".local").join("bin").join("git"));
    }
    if let Some(user) = std::env::var_os("USER") {
        candidates.push(
            PathBuf::from("/nix/var/nix/profiles/per-user")
                .join(user)
                .join("bin")
                .join("git"),
        );
    }
    candidates.push(PathBuf::from("/nix/var/nix/profiles/default/bin/git"));
    candidates.push(PathBuf::from("/opt/homebrew/bin/git"));
    candidates.push(PathBuf::from("/opt/local/bin/git"));
    candidates.push(PathBuf::from("/usr/local/bin/git"));
    candidates.push(PathBuf::from("/usr/bin/git"));
    candidates.push(PathBuf::from("/bin/git"));
    candidates.into_iter().find(|path| is_executable_file(path))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct GitWorkspaceInfo {
    pub is_git_repo: bool,
    pub repo_root: Option<String>,
    pub current_branch: Option<String>,
    pub branches: Vec<String>,
    pub head_commit: Option<String>,
    pub upstream_branch: Option<String>,
    pub dirty_worktree: bool,
    pub ahead_count: u32,
    pub behind_count: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct GitChangedFile {
    pub path: String,
    pub status: String,
    pub original_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GitStatus {
    pub staged: Vec<GitChangedFile>,
    pub unstaged: Vec<GitChangedFile>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommitResult {
    pub hash: String,
    pub message: String,
}

pub fn workspace_git_info(
    root: &WorkspaceRoot,
    workspace_id: String,
) -> Result<GitWorkspaceInfo, String> {
    let workspace_root = root.path(&workspace_id)?;
    git_info_for_path(&workspace_root)
}

pub fn git_info_for_path(path: &Path) -> Result<GitWorkspaceInfo, String> {
    let _metric =
        crate::infrastructure::performance::span("workspace_repository_context_ms", "workspace");
    let Some(repo_root) = repository_root_at(path)? else {
        return Ok(GitWorkspaceInfo {
            is_git_repo: false,
            repo_root: None,
            current_branch: None,
            branches: Vec::new(),
            head_commit: None,
            upstream_branch: None,
            dirty_worktree: false,
            ahead_count: 0,
            behind_count: 0,
        });
    };

    let current_branch = git_output(&repo_root, ["rev-parse", "--abbrev-ref", "HEAD"])
        .and_then(|value| normalize_branch_name(&value));
    let branches = git_output(
        &repo_root,
        ["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .map(|output| {
        let mut branches: Vec<String> = output
            .lines()
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .map(ToString::to_string)
            .collect();
        branches.sort_unstable();
        branches.dedup();
        branches
    })
    .unwrap_or_default();
    let head_commit = git_output(&repo_root, ["rev-parse", "HEAD"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let upstream_branch = git_output(
        &repo_root,
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());
    let dirty_worktree = git_status_for_repo(&repo_root)
        .map(|status| !status.staged.is_empty() || !status.unstaged.is_empty())
        .unwrap_or(false);
    let (ahead_count, behind_count) = if upstream_branch.is_some() {
        git_output(
            &repo_root,
            ["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        )
        .and_then(|value| parse_ahead_behind(&value))
        .unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    Ok(GitWorkspaceInfo {
        is_git_repo: true,
        repo_root: Some(repo_root.to_string_lossy().to_string()),
        current_branch,
        branches,
        head_commit,
        upstream_branch,
        dirty_worktree,
        ahead_count,
        behind_count,
    })
}

pub fn checkout_workspace_git_branch(
    root: &WorkspaceRoot,
    workspace_id: String,
    branch: String,
) -> Result<GitWorkspaceInfo, String> {
    let workspace_root = root.path(&workspace_id)?;
    let Some(repo_root) = repository_root_at(&workspace_root)? else {
        return Err("Workspace root is not inside a git repository.".to_string());
    };
    let branch = validated_branch_name(&repo_root, &branch)?;

    let mut command = Command::new(git_executable()?);
    inject_global_environment(&mut command);
    let status = command
        .args(["switch", "--", branch.as_str()])
        .current_dir(&repo_root)
        .output()
        .map_err(|error| format!("Failed to run git switch: {error}"))?;
    if !status.status.success() {
        return Err(redact_global_environment_values(
            String::from_utf8_lossy(&status.stderr).trim(),
        ));
    }

    invalidate_git_status_for_repo(&repo_root);
    workspace_git_info(root, workspace_id)
}

pub fn workspace_git_status(
    root: &WorkspaceRoot,
    workspace_id: String,
) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    git_status_for_repo(&repo_root)
}

fn git_status_for_repo(repo_root: &Path) -> Result<GitStatus, String> {
    let cache = git_status_cache();
    loop {
        let mut state = cache.state.lock().map_err(|error| error.to_string())?;
        if let Some(cached) = state.entries.get(repo_root) {
            if cached.refreshed_at.elapsed() <= GIT_STATUS_CACHE_TTL {
                return Ok(cached.status.clone());
            }
        }

        if state.in_flight.insert(repo_root.to_path_buf()) {
            break;
        }
        state = cache.ready.wait(state).map_err(|error| error.to_string())?;
        drop(state);
    }

    let result = load_git_status(repo_root);
    let mut state = cache.state.lock().map_err(|error| error.to_string())?;
    state.in_flight.remove(repo_root);
    if let Ok(status) = &result {
        state.entries.insert(
            repo_root.to_path_buf(),
            CachedGitStatus {
                refreshed_at: Instant::now(),
                status: status.clone(),
            },
        );
    }
    cache.ready.notify_all();
    result
}

fn load_git_status(repo_root: &Path) -> Result<GitStatus, String> {
    let mut command = Command::new(git_executable()?);
    inject_global_environment(&mut command);
    let output = command
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=normal"])
        .current_dir(repo_root)
        .output()
        .map_err(|error| format!("Failed to run git status: {error}"))?;
    if !output.status.success() {
        return Err(redact_global_environment_values(
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut entries = output.stdout.split(|byte| *byte == 0).peekable();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }

        let index_status = entry[0] as char;
        let worktree_status = entry[1] as char;
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        let original_path =
            if matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C') {
                entries
                    .next()
                    .map(|value| String::from_utf8_lossy(value).to_string())
                    .filter(|value| !value.is_empty())
            } else {
                None
            };
        if path.is_empty() {
            continue;
        }

        if index_status == '?' && worktree_status == '?' {
            unstaged.push(GitChangedFile {
                path,
                status: "U".to_string(),
                original_path,
            });
            continue;
        }

        if index_status != ' ' {
            staged.push(GitChangedFile {
                path: path.clone(),
                status: normalize_git_status(index_status),
                original_path: original_path.clone(),
            });
        }

        if worktree_status != ' ' {
            unstaged.push(GitChangedFile {
                path,
                status: normalize_git_status(worktree_status),
                original_path,
            });
        }
    }

    staged.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(GitStatus { staged, unstaged })
}

pub fn stage_workspace_git_file(
    root: &WorkspaceRoot,
    workspace_id: String,
    path: String,
) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    let path = normalized_file_path(path)?;
    run_git_dynamic(&repo_root, &["add", "--", &path])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_status(root, workspace_id)
}

pub fn stage_all_workspace_git_files(
    root: &WorkspaceRoot,
    workspace_id: String,
) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    run_git(&repo_root, ["add", "."])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_status(root, workspace_id)
}

pub fn unstage_workspace_git_file(
    root: &WorkspaceRoot,
    workspace_id: String,
    path: String,
) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    let path = normalized_file_path(path)?;
    run_git_dynamic(&repo_root, &["restore", "--staged", "--", &path])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_status(root, workspace_id)
}

pub fn unstage_all_workspace_git_files(
    root: &WorkspaceRoot,
    workspace_id: String,
) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    run_git(&repo_root, ["restore", "--staged", "--", "."])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_status(root, workspace_id)
}

pub fn pull_workspace_git_changes(
    root: &WorkspaceRoot,
    workspace_id: String,
) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    run_git(&repo_root, ["pull"])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_info(root, workspace_id)
}

pub fn commit_workspace_git_changes(
    root: &WorkspaceRoot,
    workspace_id: String,
    message: String,
) -> Result<CommitResult, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    let message = message.trim();
    if message.is_empty() {
        return Err("Commit message is required.".to_string());
    }

    run_git(&repo_root, ["commit", "-m", message])?;
    invalidate_git_status_for_repo(&repo_root);
    let hash = git_output(&repo_root, ["rev-parse", "HEAD"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Failed to read commit hash after commit.".to_string())?;

    Ok(CommitResult {
        hash,
        message: message.to_string(),
    })
}

pub fn push_workspace_git_changes(
    root: &WorkspaceRoot,
    workspace_id: String,
) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    run_git(&repo_root, ["push"])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_info(root, workspace_id)
}

pub fn merge_workspace_git_branch(
    root: &WorkspaceRoot,
    workspace_id: String,
    branch: String,
) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    let branch = validated_branch_name(&repo_root, &branch)?;

    run_git_dynamic(&repo_root, &["merge", "--no-edit", "--", &branch])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_info(root, workspace_id)
}

pub fn rebase_workspace_git_branch(
    root: &WorkspaceRoot,
    workspace_id: String,
    branch: String,
) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    let branch = validated_branch_name(&repo_root, &branch)?;

    run_git_dynamic(&repo_root, &["rebase", "--", &branch])?;
    invalidate_git_status_for_repo(&repo_root);
    workspace_git_info(root, workspace_id)
}

pub(crate) fn repository_root_at(path: &Path) -> Result<Option<PathBuf>, String> {
    let _metric =
        crate::infrastructure::performance::span("workspace_repository_discovery_ms", "workspace");
    let mut command = Command::new(git_executable()?);
    inject_global_environment(&mut command);
    let output = command
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .map_err(|error| format!("Failed to run git: {error}."))?;
    if !output.status.success() {
        return Ok(None);
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(root)))
    }
}

fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<String> {
    let git = git_executable().ok()?;
    let mut command = Command::new(git);
    inject_global_environment(&mut command);
    let output = command.args(args).current_dir(cwd).output().ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<(), String> {
    let mut command = Command::new(git_executable()?);
    inject_global_environment(&mut command);
    let output = command
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("Failed to run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(redact_global_environment_values(
            String::from_utf8_lossy(&output.stderr).trim(),
        ))
    }
}

fn run_git_dynamic(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new(git_executable()?);
    inject_global_environment(&mut command);
    let output = command
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("Failed to run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(redact_global_environment_values(
            String::from_utf8_lossy(&output.stderr).trim(),
        ))
    }
}

fn normalized_file_path(path: String) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        Err("File path is required.".to_string())
    } else {
        Ok(path.to_string())
    }
}

fn validated_branch_name(repo_root: &Path, branch: &str) -> Result<String, String> {
    if branch.is_empty()
        || branch != branch.trim()
        || branch.starts_with('-')
        || matches!(branch, "@" | "HEAD")
        || branch.chars().any(char::is_control)
    {
        return Err("Branch name is not a canonical Git ref.".to_string());
    }
    let full_ref = format!("refs/heads/{branch}");
    let mut command = Command::new(git_executable()?);
    inject_global_environment(&mut command);
    let valid = command
        .args(["check-ref-format", full_ref.as_str()])
        .current_dir(repo_root)
        .status()
        .map_err(|error| format!("Failed to validate Git branch name: {error}"))?
        .success();
    if !valid {
        return Err("Branch name is not a canonical Git ref.".to_string());
    }
    Ok(branch.to_string())
}

fn normalize_git_status(status: char) -> String {
    match status {
        'A' => "A",
        'D' => "D",
        'R' => "R",
        'C' => "A",
        'U' => "U",
        '?' => "U",
        _ => "M",
    }
    .to_string()
}

fn workspace_repo_root(root: &WorkspaceRoot, workspace_id: &str) -> Result<PathBuf, String> {
    let workspace_root = root.path(workspace_id)?;
    repository_root_at(&workspace_root)?
        .ok_or_else(|| "Workspace root is not inside a git repository.".to_string())
}

pub fn invalidate_workspace_git_status(
    root: &WorkspaceRoot,
    workspace_id: String,
) -> Result<(), String> {
    let repo_root = workspace_repo_root(root, &workspace_id)?;
    invalidate_git_status_for_repo(&repo_root);
    Ok(())
}

fn git_status_cache() -> &'static GitStatusCache {
    GIT_STATUS_CACHE.get_or_init(|| GitStatusCache {
        state: Mutex::new(GitStatusCacheState::default()),
        ready: Condvar::new(),
    })
}

fn invalidate_git_status_for_repo(repo_root: &Path) {
    if let Ok(mut state) = git_status_cache().state.lock() {
        state.entries.remove(repo_root);
    }
}

fn normalize_branch_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "HEAD" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_ahead_behind(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.split_whitespace();
    let ahead = parts.next()?.parse().ok()?;
    let behind = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn branch_validation_rejects_option_injection_and_invalid_refs() {
        let repo = tempfile::tempdir().expect("temporary repository");
        for branch in [
            "--exec=touch /tmp/pwned",
            "-c core.pager=cat",
            " leading",
            "trailing ",
            "line\nbreak",
            "feature..branch",
            "feature/.lock",
            "feature@{1}",
            "feature\\branch",
            "@",
            "HEAD",
        ] {
            assert!(
                validated_branch_name(repo.path(), branch).is_err(),
                "accepted adversarial branch {branch:?}"
            );
        }
        assert_eq!(
            validated_branch_name(repo.path(), "feature/safe-branch").unwrap(),
            "feature/safe-branch"
        );
    }

    #[test]
    fn git_executable_resolves_to_a_real_binary() {
        let git = super::git_executable().expect("git should resolve");
        assert!(is_executable_file(&git), "resolved git is not executable");
        assert_eq!(super::git_executable().expect("git should re-resolve"), git);
    }

    #[test]
    fn status_cache_requires_invalidation_for_immediate_external_changes() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let repo = std::env::temp_dir().join(format!("spacesly-git-cache-{suffix}"));
        fs::create_dir_all(&repo).expect("temporary repository should be created");
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&repo)
            .status()
            .expect("git should start");
        assert!(init.success());
        fs::write(repo.join("new.txt"), "content").expect("test file should be written");
        fs::write(repo.join("with spaces.txt"), "content").expect("spaced file should be written");

        let first = git_status_for_repo(&repo).expect("status should load");
        assert_eq!(first.unstaged.len(), 2);
        assert!(first
            .unstaged
            .iter()
            .any(|file| file.path == "with spaces.txt"));
        fs::remove_file(repo.join("new.txt")).expect("test file should be removed");
        fs::remove_file(repo.join("with spaces.txt")).expect("spaced file should be removed");
        let cached = git_status_for_repo(&repo).expect("cached status should load");
        assert_eq!(cached.unstaged.len(), 2);

        invalidate_git_status_for_repo(&repo);
        let refreshed = git_status_for_repo(&repo).expect("status should refresh");
        assert!(refreshed.unstaged.is_empty());

        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn concurrent_status_requests_share_repository_in_flight_state() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let repo = std::env::temp_dir().join(format!("spacesly-git-coalesce-{suffix}"));
        fs::create_dir_all(&repo).expect("temporary repository should be created");
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&repo)
            .status()
            .expect("git should start");
        assert!(init.success());
        invalidate_git_status_for_repo(&repo);

        let barrier = Arc::new(Barrier::new(6));
        let handles = (0..6)
            .map(|_| {
                let barrier = barrier.clone();
                let repo = repo.clone();
                thread::spawn(move || {
                    barrier.wait();
                    git_status_for_repo(&repo).expect("status should load")
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("status request should not deadlock");
        }

        let cache = git_status_cache();
        let state = cache.state.lock().expect("cache should not be poisoned");
        assert!(!state.in_flight.contains(&repo));
        assert!(state.entries.contains_key(&repo));
        drop(state);
        let _ = fs::remove_dir_all(repo);
    }
}
