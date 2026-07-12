use crate::domain::entity::{
    Board, BoardId, Card, CardId, CardSource, Column, ColumnId, ColumnIntent, ExecutionState,
    Priority, Project, ProjectId, Workspace, WorkspaceId,
};

/// Application service for workspace projections.
pub struct AppState {
    workspace: Workspace,
}

/// Normalized work item imported from an external tracker.
pub struct ImportedIssue {
    pub key: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: String,
    pub issue_type: String,
    pub url: Option<String>,
    pub labels: Vec<String>,
}

impl AppState {
    /// Builds the initial local workspace projection.
    pub fn new() -> Self {
        Self {
            workspace: seeded_workspace(),
        }
    }

    /// Returns the current workspace board projection.
    pub fn workspace(&self) -> Workspace {
        self.workspace.clone()
    }

    /// Returns a workspace projection with imported Jira issues mapped to board cards.
    pub fn workspace_with_imported_issues(&self, issues: &[ImportedIssue]) -> Workspace {
        let mut workspace = self.workspace.clone();

        if let Some(board) = workspace
            .projects
            .first_mut()
            .and_then(|project| project.boards.first_mut())
        {
            for issue in issues {
                let target_intent = intent_for_status(&issue.status);
                let card = card_from_issue(issue);

                if let Some(column) = board
                    .columns
                    .iter_mut()
                    .find(|column| intent_matches(&column.intent, &target_intent))
                {
                    column.cards.push(card);
                }
            }
        }

        workspace
    }
}

fn card_from_issue(issue: &ImportedIssue) -> Card {
    Card {
        id: CardId(format!("jira-{}", issue.key.to_lowercase())),
        title: issue.summary.clone(),
        source: CardSource::Jira {
            key: issue.key.clone(),
        },
        url: issue.url.clone(),
        labels: issue.labels.clone(),
        description: issue
            .description
            .clone()
            .unwrap_or_else(|| format!("{} imported from Jira.", issue.issue_type)),
        assignee: None,
        priority: Priority::Medium,
        execution: ExecutionState::Idle,
    }
}

fn intent_for_status(status: &str) -> ColumnIntent {
    let normalized = status.to_lowercase();

    if normalized.contains("progress") || normalized.contains("doing") {
        ColumnIntent::InProgress
    } else if normalized.contains("queued") || normalized.contains("queue") {
        ColumnIntent::Queued
    } else if normalized.contains("review") || normalized.contains("qa") {
        ColumnIntent::Queued
    } else if normalized.contains("done") || normalized.contains("closed") {
        ColumnIntent::Done
    } else if normalized.contains("ready") {
        ColumnIntent::Queued
    } else {
        ColumnIntent::Backlog
    }
}

fn intent_matches(left: &ColumnIntent, right: &ColumnIntent) -> bool {
    matches!(
        (left, right),
        (ColumnIntent::Backlog, ColumnIntent::Backlog)
            | (ColumnIntent::Queued, ColumnIntent::Queued)
            | (ColumnIntent::InProgress, ColumnIntent::InProgress)
            | (ColumnIntent::Review, ColumnIntent::Review)
            | (ColumnIntent::Done, ColumnIntent::Done)
    )
}

fn seeded_workspace() -> Workspace {
    let workspace_id = WorkspaceId("workspace-personal".to_string());

    Workspace {
        id: workspace_id.clone(),
        name: "Personal command center".to_string(),
        projects: vec![Project {
            id: ProjectId("project-spacesly".to_string()),
            workspace_id,
            name: "Spacesly".to_string(),
            boards: vec![Board {
                id: BoardId("board-daily-work".to_string()),
                name: "Daily work orchestration".to_string(),
                columns: vec![
                    Column {
                        id: ColumnId("column-backlog".to_string()),
                        name: "Backlog".to_string(),
                        intent: ColumnIntent::Backlog,
                        cards: vec![],
                    },
                    Column {
                        id: ColumnId("column-queued".to_string()),
                        name: "Queued".to_string(),
                        intent: ColumnIntent::Queued,
                        cards: vec![],
                    },
                    Column {
                        id: ColumnId("column-in-progress".to_string()),
                        name: "In progress".to_string(),
                        intent: ColumnIntent::InProgress,
                        cards: vec![],
                    },
                    Column {
                        id: ColumnId("column-done".to_string()),
                        name: "Done".to_string(),
                        intent: ColumnIntent::Done,
                        cards: vec![],
                    },
                ],
            }],
        }],
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_workspace_has_a_board_projection() {
        let state = AppState::new();
        let workspace = state.workspace();
        let project = &workspace.projects[0];
        let board = &project.boards[0];

        assert_eq!(workspace.name, "Personal command center");
        assert_eq!(board.columns.len(), 4);
        assert!(board.columns.iter().all(|column| column.cards.is_empty()));
    }

    #[test]
    fn imported_issues_are_added_to_matching_columns() {
        let state = AppState::new();
        let workspace = state.workspace_with_imported_issues(&[ImportedIssue {
            key: "SPC-44".to_string(),
            summary: "Fetch tickets".to_string(),
            description: None,
            status: "In Progress".to_string(),
            issue_type: "Task".to_string(),
            url: None,
            labels: Vec::new(),
        }]);
        let board = &workspace.projects[0].boards[0];
        let in_progress = board
            .columns
            .iter()
            .find(|column| matches!(column.intent, ColumnIntent::InProgress))
            .unwrap();

        assert!(in_progress
            .cards
            .iter()
            .any(|card| card.title == "Fetch tickets"));
    }
}
