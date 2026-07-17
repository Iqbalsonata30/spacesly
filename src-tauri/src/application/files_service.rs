use crate::infrastructure::files::{
    list_directory, read_file_at_root, set_workspace_root, workspace_root_path, write_file,
    FileEntry, FileSnapshot, FileWriteResult, LineEnding, TextEncoding, WorkspaceRoot,
};

#[derive(Clone)]
pub struct FilesService {
    root: WorkspaceRoot,
}

impl FilesService {
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }

    pub fn list_directory(
        &self,
        workspace_id: String,
        relative_path: String,
    ) -> Result<Vec<FileEntry>, String> {
        list_directory(&self.root, workspace_id, relative_path)
    }

    pub fn read_file(
        &self,
        workspace_id: String,
        relative_path: String,
    ) -> Result<FileSnapshot, String> {
        read_file_at_root(&self.root, workspace_id, relative_path)
    }

    pub fn write_file(
        &self,
        workspace_id: String,
        relative_path: String,
        content: String,
        expected_version: Option<String>,
        expected_root_revision: Option<u64>,
        encoding: TextEncoding,
        line_ending: LineEnding,
    ) -> Result<FileWriteResult, String> {
        write_file(
            &self.root,
            workspace_id,
            relative_path,
            content,
            expected_version,
            expected_root_revision,
            encoding,
            line_ending,
        )
    }

    pub fn workspace_root_path(&self, workspace_id: String) -> Result<String, String> {
        workspace_root_path(&self.root, workspace_id)
    }

    pub fn set_workspace_root(
        &self,
        workspace_id: String,
        absolute_path: String,
    ) -> Result<String, String> {
        set_workspace_root(&self.root, workspace_id, absolute_path)
    }
}
