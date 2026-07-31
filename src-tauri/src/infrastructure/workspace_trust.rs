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
        self.status_path(&roots.path(workspace_id)?)
    }

    pub fn status_path(&self, path: &Path) -> Result<WorkspaceTrustStatus, String> {
        let path = canonical_workspace_path(path)?;
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
        self.trust_path(&roots.path(workspace_id)?)
    }

    pub fn trust_path(&self, path: &Path) -> Result<WorkspaceTrustStatus, String> {
        let path = canonical_workspace_path(path)?;
        if is_home_directory(&path) {
            return Err(
                "The home directory cannot be trusted as an AI execution workspace. Open a specific project folder first."
                    .to_string(),
            );
        }
        self.trusted_paths
            .lock()
            .map_err(|error| error.to_string())?
            .insert(path.clone());
        self.status_path(&path)
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

    pub fn require_trusted_path(&self, path: &Path) -> Result<PathBuf, String> {
        let status = self.status_path(path)?;
        if !status.trusted {
            return Err(format!(
                "Workspace is not trusted for AI tool execution: {}",
                status.path
            ));
        }
        Ok(PathBuf::from(status.path))
    }
}

fn canonical_workspace_path(path: &Path) -> Result<PathBuf, String> {
    let expanded;
    let path = if path == Path::new("~") || path.starts_with("~/") {
        let relative = path
            .strip_prefix("~")
            .expect("home-relative path prefix was checked");
        expanded = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not configured for the AI working directory.".to_string())?
            .join(relative);
        expanded.as_path()
    } else {
        path
    };
    if !path.is_absolute() {
        return Err("AI working directory must be an absolute or home-relative path.".to_string());
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("Failed to canonicalize AI working directory: {error}"))?;
    if !path.is_dir() {
        return Err("AI working directory must be a directory.".to_string());
    }
    Ok(path)
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

    #[test]
    fn configured_working_directory_has_independent_exact_trust() {
        let base = std::env::temp_dir().join(format!(
            "spacesly-configured-workdir-trust-{}",
            std::process::id()
        ));
        let workspace = base.join("workspace");
        let configured = base.join("configured");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&configured).unwrap();
        let roots = WorkspaceRoot::home().unwrap();
        roots
            .set_path("workspace-personal", workspace.clone())
            .unwrap();
        let registry = WorkspaceTrustRegistry::default();

        registry.trust_path(&configured).unwrap();

        assert_eq!(
            registry.require_trusted_path(&configured).unwrap(),
            configured.canonicalize().unwrap()
        );
        assert!(registry
            .require_trusted(&roots, "workspace-personal")
            .is_err());

        let _ = fs::remove_dir_all(base);
    }
}
