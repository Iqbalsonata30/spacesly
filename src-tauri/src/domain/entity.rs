use serde::Serialize;

/// Unique identifier for a workspace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceId(pub String);

/// Unique identifier for a project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectId(pub String);

/// Unique identifier for a board.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoardId(pub String);

/// Unique identifier for a column.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ColumnId(pub String);

/// Unique identifier for a card.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CardId(pub String);

/// A workspace groups projects and their execution context.
#[derive(Clone, Debug, Serialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub projects: Vec<Project>,
}

/// A project contains one or more boards.
#[derive(Clone, Debug, Serialize)]
pub struct Project {
    pub id: ProjectId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub boards: Vec<Board>,
}

/// A board is a workflow projection made of ordered columns.
#[derive(Clone, Debug, Serialize)]
pub struct Board {
    pub id: BoardId,
    pub name: String,
    pub columns: Vec<Column>,
}

/// A board column represents a workflow state.
#[derive(Clone, Debug, Serialize)]
pub struct Column {
    pub id: ColumnId,
    pub name: String,
    pub intent: ColumnIntent,
    pub cards: Vec<Card>,
}

/// The semantic meaning of a column.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnIntent {
    Backlog,
    Ready,
    InProgress,
    Review,
    Done,
}

/// A card represents work imported from a tool or created locally.
#[derive(Clone, Debug, Serialize)]
pub struct Card {
    pub id: CardId,
    pub title: String,
    pub source: CardSource,
    pub url: Option<String>,
    pub labels: Vec<String>,
    pub description: String,
    pub assignee: Option<String>,
    pub priority: Priority,
    pub execution: ExecutionState,
}

/// Origin of the work item.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardSource {
    Jira { key: String },
    Local,
}

/// Priority used by the workspace board.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Medium,
    High,
}

/// Observable workflow execution state for a card.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionState {
    Idle,
    Queued,
    Running,
    Blocked { reason: String },
    Completed { summary: String },
}
