use crate::infrastructure::files::WorkspaceRoot;
use crate::infrastructure::git::{
    checkout_workspace_git_branch, workspace_git_info, GitWorkspaceInfo,
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

    pub fn checkout_branch(&self, branch: String) -> Result<GitWorkspaceInfo, String> {
        checkout_workspace_git_branch(&self.root, branch)
    }
}
