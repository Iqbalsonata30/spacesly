use crate::infrastructure::files::WorkspaceRoot;
use crate::infrastructure::git::{checkout_workspace_git_branch, workspace_git_info, GitWorkspaceInfo};
use crate::infrastructure::git::{
    commit_workspace_git_changes, merge_workspace_git_branch, pull_workspace_git_changes,
    push_workspace_git_changes, rebase_workspace_git_branch, workspace_changed_files, CommitResult,
    GitChangedFile,
};

#[derive(Clone)]
pub struct GitService {
    root: WorkspaceRoot,
}

impl GitService {
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }

    pub fn workspace_git_info(&self) -> Result<GitWorkspaceInfo, String> {
        workspace_git_info(&self.root)
    }

    pub fn changed_files(&self) -> Result<Vec<GitChangedFile>, String> {
        workspace_changed_files(&self.root)
    }

    pub fn checkout_branch(&self, branch: String) -> Result<GitWorkspaceInfo, String> {
        checkout_workspace_git_branch(&self.root, branch)
    }

    pub fn pull_changes(&self) -> Result<GitWorkspaceInfo, String> {
        pull_workspace_git_changes(&self.root)
    }

    pub fn commit_changes(&self, message: String) -> Result<CommitResult, String> {
        commit_workspace_git_changes(&self.root, message)
    }

    pub fn push_changes(&self) -> Result<GitWorkspaceInfo, String> {
        push_workspace_git_changes(&self.root)
    }

    pub fn merge_branch(&self, branch: String) -> Result<GitWorkspaceInfo, String> {
        merge_workspace_git_branch(&self.root, branch)
    }

    pub fn rebase_branch(&self, branch: String) -> Result<GitWorkspaceInfo, String> {
        rebase_workspace_git_branch(&self.root, branch)
    }
}
