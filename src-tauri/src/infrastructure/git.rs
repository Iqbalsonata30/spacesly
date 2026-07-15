use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::files::WorkspaceRoot;

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

pub fn workspace_git_info(root: &WorkspaceRoot) -> Result<GitWorkspaceInfo, String> {
    let workspace_root = root.path()?;
    git_info_for_path(&workspace_root)
}

pub fn git_info_for_path(path: &Path) -> Result<GitWorkspaceInfo, String> {
    let Some(repo_root) = git_repo_root(path)? else {
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
    let dirty_worktree = git_output(&repo_root, ["status", "--porcelain"])
        .map(|value| !value.trim().is_empty())
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
    branch: String,
) -> Result<GitWorkspaceInfo, String> {
    let workspace_root = root.path()?;
    let Some(repo_root) = git_repo_root(&workspace_root)? else {
        return Err("Workspace root is not inside a git repository.".to_string());
    };
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Branch name is required.".to_string());
    }

    let status = Command::new("git")
        .args(["checkout", branch])
        .current_dir(&repo_root)
        .output()
        .map_err(|error| format!("Failed to run git checkout: {error}"))?;
    if !status.status.success() {
        return Err(String::from_utf8_lossy(&status.stderr).trim().to_string());
    }

    workspace_git_info(root)
}

pub fn workspace_changed_files(root: &WorkspaceRoot) -> Result<Vec<GitChangedFile>, String> {
    let status = workspace_git_status(root)?;
    let mut files = status.staged;
    files.extend(status.unstaged);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub fn workspace_git_status(root: &WorkspaceRoot) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root)?;
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(&repo_root)
        .output()
        .map_err(|error| format!("Failed to run git status: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim_end();
        if line.len() < 4 {
            continue;
        }

        let index_status = line.as_bytes()[0] as char;
        let worktree_status = line.as_bytes()[1] as char;
        let (path, original_path) = parse_status_path(line[3..].trim());
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

pub fn stage_workspace_git_file(root: &WorkspaceRoot, path: String) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root)?;
    let path = normalized_file_path(path)?;
    run_git_dynamic(&repo_root, &["add", "--", &path])?;
    workspace_git_status(root)
}

pub fn stage_all_workspace_git_files(root: &WorkspaceRoot) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root)?;
    run_git(&repo_root, ["add", "."])?;
    workspace_git_status(root)
}

pub fn unstage_workspace_git_file(root: &WorkspaceRoot, path: String) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root)?;
    let path = normalized_file_path(path)?;
    run_git_dynamic(&repo_root, &["restore", "--staged", "--", &path])?;
    workspace_git_status(root)
}

pub fn unstage_all_workspace_git_files(root: &WorkspaceRoot) -> Result<GitStatus, String> {
    let repo_root = workspace_repo_root(root)?;
    run_git(&repo_root, ["restore", "--staged", "--", "."])?;
    workspace_git_status(root)
}

pub fn pull_workspace_git_changes(root: &WorkspaceRoot) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root)?;
    run_git(&repo_root, ["pull"])?;
    workspace_git_info(root)
}

pub fn commit_workspace_git_changes(
    root: &WorkspaceRoot,
    message: String,
) -> Result<CommitResult, String> {
    let repo_root = workspace_repo_root(root)?;
    let message = message.trim();
    if message.is_empty() {
        return Err("Commit message is required.".to_string());
    }

    run_git(&repo_root, ["commit", "-m", message])?;
    let hash = git_output(&repo_root, ["rev-parse", "HEAD"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Failed to read commit hash after commit.".to_string())?;

    Ok(CommitResult {
        hash,
        message: message.to_string(),
    })
}

pub fn push_workspace_git_changes(root: &WorkspaceRoot) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root)?;
    run_git(&repo_root, ["push"])?;
    workspace_git_info(root)
}

pub fn merge_workspace_git_branch(
    root: &WorkspaceRoot,
    branch: String,
) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root)?;
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Merge branch is required.".to_string());
    }

    run_git(&repo_root, ["merge", branch, "--no-edit"])?;
    workspace_git_info(root)
}

pub fn rebase_workspace_git_branch(
    root: &WorkspaceRoot,
    branch: String,
) -> Result<GitWorkspaceInfo, String> {
    let repo_root = workspace_repo_root(root)?;
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("Rebase branch is required.".to_string());
    }

    run_git(&repo_root, ["rebase", branch])?;
    workspace_git_info(root)
}

fn git_repo_root(path: &Path) -> Result<Option<PathBuf>, String> {
    let output = git_output(path, ["rev-parse", "--show-toplevel"]);
    match output {
        Some(output) => {
            let root = output.trim();
            if root.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PathBuf::from(root)))
            }
        }
        None => Ok(None),
    }
}

fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("Failed to run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn run_git_dynamic(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("Failed to run git {}: {error}", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
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

fn parse_status_path(value: &str) -> (String, Option<String>) {
    if let Some((original_path, path)) = value.split_once(" -> ") {
        (
            path.trim().to_string(),
            Some(original_path.trim().to_string()),
        )
    } else {
        (value.trim().to_string(), None)
    }
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

fn workspace_repo_root(root: &WorkspaceRoot) -> Result<PathBuf, String> {
    let workspace_root = root.path()?;
    git_repo_root(&workspace_root)?
        .ok_or_else(|| "Workspace root is not inside a git repository.".to_string())
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
