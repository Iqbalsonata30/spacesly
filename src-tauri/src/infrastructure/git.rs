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
}

pub fn workspace_git_info(root: &WorkspaceRoot) -> Result<GitWorkspaceInfo, String> {
    let workspace_root = root.path()?;
    let Some(repo_root) = git_repo_root(&workspace_root)? else {
        return Ok(GitWorkspaceInfo {
            is_git_repo: false,
            repo_root: None,
            current_branch: None,
            branches: Vec::new(),
        });
    };

    let current_branch = git_output(&repo_root, ["rev-parse", "--abbrev-ref", "HEAD"])
        .and_then(|value| normalize_branch_name(&value));
    let branches = git_output(&repo_root, ["for-each-ref", "--format=%(refname:short)", "refs/heads"])
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

    Ok(GitWorkspaceInfo {
        is_git_repo: true,
        repo_root: Some(repo_root.to_string_lossy().to_string()),
        current_branch,
        branches,
    })
}

pub fn checkout_workspace_git_branch(root: &WorkspaceRoot, branch: String) -> Result<GitWorkspaceInfo, String> {
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

fn normalize_branch_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "HEAD" {
        None
    } else {
        Some(trimmed.to_string())
    }
}
