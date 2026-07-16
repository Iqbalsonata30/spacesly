use crate::infrastructure::files::WorkspaceRoot;
use crate::infrastructure::git::{
    checkout_workspace_git_branch, workspace_git_info, GitWorkspaceInfo,
};
use crate::infrastructure::git::{
    commit_workspace_git_changes, merge_workspace_git_branch, pull_workspace_git_changes,
    push_workspace_git_changes, rebase_workspace_git_branch, stage_all_workspace_git_files,
    stage_workspace_git_file, unstage_all_workspace_git_files, unstage_workspace_git_file,
    workspace_git_status, CommitResult, GitStatus,
};

#[derive(Clone)]
pub struct GitService {
    root: WorkspaceRoot,
}

impl GitService {
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }

    pub fn workspace_git_info(&self, workspace_id: String) -> Result<GitWorkspaceInfo, String> {
        workspace_git_info(&self.root, workspace_id)
    }

    pub fn status(&self, workspace_id: String) -> Result<GitStatus, String> {
        workspace_git_status(&self.root, workspace_id)
    }

    pub fn stage_file(&self, workspace_id: String, path: String) -> Result<GitStatus, String> {
        stage_workspace_git_file(&self.root, workspace_id, path)
    }

    pub fn stage_all(&self, workspace_id: String) -> Result<GitStatus, String> {
        stage_all_workspace_git_files(&self.root, workspace_id)
    }

    pub fn unstage_file(&self, workspace_id: String, path: String) -> Result<GitStatus, String> {
        unstage_workspace_git_file(&self.root, workspace_id, path)
    }

    pub fn unstage_all(&self, workspace_id: String) -> Result<GitStatus, String> {
        unstage_all_workspace_git_files(&self.root, workspace_id)
    }

    pub fn checkout_branch(
        &self,
        workspace_id: String,
        branch: String,
    ) -> Result<GitWorkspaceInfo, String> {
        checkout_workspace_git_branch(&self.root, workspace_id, branch)
    }

    pub fn pull_changes(&self, workspace_id: String) -> Result<GitWorkspaceInfo, String> {
        pull_workspace_git_changes(&self.root, workspace_id)
    }

    pub fn commit_changes(
        &self,
        workspace_id: String,
        message: String,
    ) -> Result<CommitResult, String> {
        commit_workspace_git_changes(&self.root, workspace_id, message)
    }

    pub fn push_changes(&self, workspace_id: String) -> Result<GitWorkspaceInfo, String> {
        push_workspace_git_changes(&self.root, workspace_id)
    }

    pub fn merge_branch(
        &self,
        workspace_id: String,
        branch: String,
    ) -> Result<GitWorkspaceInfo, String> {
        merge_workspace_git_branch(&self.root, workspace_id, branch)
    }

    pub fn rebase_branch(
        &self,
        workspace_id: String,
        branch: String,
    ) -> Result<GitWorkspaceInfo, String> {
        rebase_workspace_git_branch(&self.root, workspace_id, branch)
    }
}
