use super::files::WorkspaceRoot;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub const WORKSPACE_FILE_CHANGE_EVENT: &str = "workspace-file-change";

#[derive(Clone, Debug, Serialize)]
pub struct WorkspaceFileChange {
    pub workspace_id: String,
    pub kind: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Default)]
pub struct FileWatchRegistry {
    watchers: Arc<Mutex<HashMap<String, RecommendedWatcher>>>,
}

impl FileWatchRegistry {
    pub fn watch(
        &self,
        app: AppHandle,
        roots: &WorkspaceRoot,
        workspace_id: String,
    ) -> Result<(), String> {
        let root = roots.path(&workspace_id)?;
        let event_root = root.clone();
        let event_workspace_id = workspace_id.clone();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                return;
            };
            let Some(kind) = event_kind(&event.kind) else {
                return;
            };
            let paths = relative_event_paths(&event_root, event.paths);
            if paths.is_empty() {
                return;
            }
            let _ = app.emit(
                WORKSPACE_FILE_CHANGE_EVENT,
                WorkspaceFileChange {
                    workspace_id: event_workspace_id.clone(),
                    kind: kind.to_string(),
                    paths,
                },
            );
        })
        .map_err(|error| format!("Failed to create workspace file watcher: {error}"))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|error| format!("Failed to watch workspace files: {error}"))?;
        self.watchers
            .lock()
            .map_err(|error| error.to_string())?
            .insert(workspace_id, watcher);
        Ok(())
    }

    pub fn unwatch(&self, workspace_id: &str) -> Result<bool, String> {
        Ok(self
            .watchers
            .lock()
            .map_err(|error| error.to_string())?
            .remove(workspace_id)
            .is_some())
    }
}

fn event_kind(kind: &EventKind) -> Option<&'static str> {
    match kind {
        EventKind::Create(_) => Some("created"),
        EventKind::Modify(_) => Some("modified"),
        EventKind::Remove(_) => Some("removed"),
        EventKind::Access(_) | EventKind::Other | EventKind::Any => None,
    }
}

fn relative_event_paths(root: &Path, paths: Vec<PathBuf>) -> Vec<String> {
    let mut relative = paths
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".spacesly-save-") && name.ends_with(".tmp"))
        })
        .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_path_buf))
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    relative.sort();
    relative.dedup();
    relative
}

#[cfg(test)]
mod tests {
    use super::{event_kind, relative_event_paths};
    use notify::event::{CreateKind, ModifyKind};
    use notify::EventKind;
    use std::path::PathBuf;

    #[test]
    fn classifies_only_content_relevant_events() {
        assert_eq!(
            event_kind(&EventKind::Create(CreateKind::File)),
            Some("created")
        );
        assert_eq!(
            event_kind(&EventKind::Modify(ModifyKind::Data(
                notify::event::DataChange::Content
            ))),
            Some("modified")
        );
        assert_eq!(
            event_kind(&EventKind::Access(notify::event::AccessKind::Any)),
            None
        );
    }

    #[test]
    fn normalizes_paths_and_ignores_atomic_save_temporary_files() {
        let root = PathBuf::from("/workspace");
        assert_eq!(
            relative_event_paths(
                &root,
                vec![
                    root.join("src/main.rs"),
                    root.join(".spacesly-save-1-2-3.tmp"),
                    PathBuf::from("/outside/file.txt"),
                ]
            ),
            vec!["src/main.rs"]
        );
    }
}
