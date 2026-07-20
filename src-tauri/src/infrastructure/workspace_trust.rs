use super::files::WorkspaceRoot;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct WorkspaceTrustStatus {
    pub path: String,
    pub trusted: bool,
}

#[derive(Clone, Default)]
pub struct WorkspaceTrustRegistry {
    trusted_paths: Arc<Mutex<HashSet<PathBuf>>>,
}

impl WorkspaceTrustRegistry {
    pub fn status(
        &self,
        roots: &WorkspaceRoot,
        workspace_id: &str,
    ) -> Result<WorkspaceTrustStatus, String> {
        let path = roots.path(workspace_id)?;
        let trusted = self
            .trusted_paths
            .lock()
            .map_err(|error| error.to_string())?
            .contains(&path);
        Ok(WorkspaceTrustStatus {
            path: path.to_string_lossy().to_string(),
            trusted,
        })
    }

    pub fn trust(
        &self,
        roots: &WorkspaceRoot,
        workspace_id: &str,
    ) -> Result<WorkspaceTrustStatus, String> {
        let path = roots.path(workspace_id)?;
        if is_home_directory(&path) {
            return Err(
                "The home directory cannot be trusted as an AI execution workspace. Open a specific project folder first."
                    .to_string(),
            );
        }
        self.trusted_paths
            .lock()
            .map_err(|error| error.to_string())?
            .insert(path);
        self.status(roots, workspace_id)
    }

    pub fn require_trusted(
        &self,
        roots: &WorkspaceRoot,
        workspace_id: &str,
    ) -> Result<PathBuf, String> {
        let status = self.status(roots, workspace_id)?;
        if !status.trusted {
            return Err(format!(
                "Workspace is not trusted for AI tool execution: {}",
                status.path
            ));
        }
        Ok(PathBuf::from(status.path))
    }
}

fn is_home_directory(path: &Path) -> bool {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| home.canonicalize().ok())
        .is_some_and(|home| home == path)
}

#[cfg(test)]
mod tests {
    use super::WorkspaceTrustRegistry;
    use crate::infrastructure::files::WorkspaceRoot;
    use std::fs;

    #[test]
    fn trust_is_bound_to_the_canonical_workspace_root() {
        let directory = std::env::temp_dir().join(format!("spacesly-trust-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let roots = WorkspaceRoot::home().unwrap();
        roots
            .set_path("workspace-personal", directory.clone())
            .unwrap();
        let registry = WorkspaceTrustRegistry::default();

        assert!(
            !registry
                .status(&roots, "workspace-personal")
                .unwrap()
                .trusted
        );
        assert!(
            registry
                .trust(&roots, "workspace-personal")
                .unwrap()
                .trusted
        );
        assert_eq!(
            registry
                .require_trusted(&roots, "workspace-personal")
                .unwrap(),
            directory.canonicalize().unwrap()
        );

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn home_directory_is_never_implicitly_trustable() {
        let roots = WorkspaceRoot::home().unwrap();
        let registry = WorkspaceTrustRegistry::default();
        assert!(registry.trust(&roots, "workspace-personal").is_err());
    }
}
