use serde::Deserialize;
use std::error::Error;
use std::time::Duration;

use super::mcp::{JiraAuthConfig, JiraBoard, JiraIssue};

/// Fetches Jira agile boards directly from Jira REST.
pub fn fetch_boards(
    auth: &JiraAuthConfig,
    project_key: Option<&str>,
    board_name: Option<&str>,
) -> Result<Vec<JiraBoard>, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Failed to create Jira HTTP client: {error}"))?;
    let base_url = normalized_base_url(&auth.base_url)?;
    let mut start_at = 0;
    let mut boards = Vec::new();

    loop {
        let mut request = client
            .get(format!("{base_url}/rest/agile/1.0/board"))
            .query(&[
                ("startAt", start_at.to_string()),
                ("maxResults", "50".to_string()),
            ]);

        if let Some(project_key) = project_key.filter(|value| !value.trim().is_empty()) {
            request = request.query(&[("projectKeyOrId", project_key)]);
        }

        if let Some(board_name) = board_name.filter(|value| !value.trim().is_empty()) {
            request = request.query(&[("name", board_name)]);
        }

        let page: BoardPage = send(auth, request)?;
        boards.extend(page.values.into_iter().map(|board| JiraBoard {
            id: board.id.to_string(),
            name: board.name,
            board_type: board.board_type,
        }));

        if page.is_last.unwrap_or(true) || start_at + page.max_results >= page.total.unwrap_or(0) {
            break;
        }
        start_at += page.max_results;
    }

    Ok(boards)
}

/// Fetches issues from a Jira agile board directly from Jira REST.
#[allow(dead_code)]
pub fn fetch_board_issues(
    auth: &JiraAuthConfig,
    board_id: &str,
    jql: &str,
) -> Result<Vec<JiraIssue>, String> {
    fetch_board_issues_paginated(auth, board_id, jql, 25, 1)
}

/// Transitions a Jira issue to the requested target status when a matching transition exists.
pub fn transition_issue(
    auth: &JiraAuthConfig,
    issue_key: &str,
    target_status: &str,
) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Failed to create Jira HTTP client: {error}"))?;
    let base_url = normalized_base_url(&auth.base_url)?;
    let transitions_url = format!("{base_url}/rest/api/2/issue/{issue_key}/transitions");
    let transitions: TransitionPage = send(auth, client.get(&transitions_url))?;
    let Some(transition) = transitions
        .transitions
        .iter()
        .find(|transition| status_matches(&transition.to.name, target_status))
        .or_else(|| {
            transitions
                .transitions
                .iter()
                .find(|transition| status_matches(&transition.name, target_status))
        })
    else {
        if issue_is_already_in_status(auth, &client, &base_url, issue_key, target_status)? {
            return Ok(());
        }

        return Err({
            let available = transitions
                .transitions
                .iter()
                .map(|transition| format!("{} -> {}", transition.name, transition.to.name))
                .collect::<Vec<_>>()
                .join(", ");
            format!("No Jira transition to '{target_status}' is available for {issue_key}. Available transitions: {available}")
        });
    };

    let body = serde_json::json!({
        "transition": {
            "id": transition.id
        }
    });

    send_empty(auth, client.post(&transitions_url).json(&body))
}

fn issue_is_already_in_status(
    auth: &JiraAuthConfig,
    client: &reqwest::blocking::Client,
    base_url: &str,
    issue_key: &str,
    target_status: &str,
) -> Result<bool, String> {
    let issue_url = format!("{base_url}/rest/api/2/issue/{issue_key}");
    let issue: RestIssue = send(auth, client.get(issue_url))?;
    Ok(status_matches(&issue.fields.status.name, target_status))
}

/// Assigns a Jira issue to the configured user.
pub fn assign_issue(auth: &JiraAuthConfig, issue_key: &str) -> Result<(), String> {
    if auth.username.trim().is_empty() {
        return Err(
            "Cannot assign Jira issue because username/email is empty in Settings.".to_string(),
        );
    }

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Failed to create Jira HTTP client: {error}"))?;
    let base_url = normalized_base_url(&auth.base_url)?;
    let assignee_url = format!("{base_url}/rest/api/2/issue/{issue_key}/assignee");
    let body = serde_json::json!({
        "name": auth.username
    });

    send_empty(auth, client.put(&assignee_url).json(&body))
}

/// Adds a Jira comment to an issue.
pub fn add_comment(auth: &JiraAuthConfig, issue_key: &str, comment: &str) -> Result<(), String> {
    if comment.trim().is_empty() {
        return Err("Cannot add an empty Jira comment.".to_string());
    }

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Failed to create Jira HTTP client: {error}"))?;
    let base_url = normalized_base_url(&auth.base_url)?;
    let comment_url = format!("{base_url}/rest/api/2/issue/{issue_key}/comment");
    let body = serde_json::json!({
        "body": comment
    });

    send_empty(auth, client.post(&comment_url).json(&body))
}

/// Fetches issues from a Jira agile board with bounded pagination.
pub fn fetch_board_issues_paginated(
    auth: &JiraAuthConfig,
    board_id: &str,
    jql: &str,
    page_size: u32,
    max_pages: u32,
) -> Result<Vec<JiraIssue>, String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Failed to create Jira HTTP client: {error}"))?;
    let base_url = normalized_base_url(&auth.base_url)?;
    let browse_base_url = base_url.clone();
    let page_size = page_size.clamp(1, 100) as i64;
    let max_pages = max_pages.max(1) as i64;
    let mut start_at = 0;
    let mut page_count = 0;
    let mut issues = Vec::new();

    loop {
        let mut request = client
            .get(format!("{base_url}/rest/agile/1.0/board/{board_id}/issue"))
            .query(&[
                ("startAt", start_at.to_string()),
                ("maxResults", page_size.to_string()),
            ]);

        if !jql.trim().is_empty() {
            request = request.query(&[("jql", jql)]);
        }

        let page: IssuePage = match send(auth, request) {
            Ok(page) => page,
            Err(_error) if !issues.is_empty() => return Ok(issues),
            Err(error) => return Err(error),
        };
        issues.extend(page.issues.into_iter().map(|issue| JiraIssue {
            url: Some(format!("{browse_base_url}/browse/{}", issue.key)),
            key: issue.key,
            summary: issue.fields.summary,
            description: issue.fields.description.and_then(description_text),
            status: issue.fields.status.name,
            issue_type: issue.fields.issue_type.name,
            labels: issue.fields.labels,
        }));

        page_count += 1;

        if page_count >= max_pages || start_at + page.max_results >= page.total {
            break;
        }
        start_at += page.max_results;
    }

    Ok(issues)
}

fn send<T: for<'de> Deserialize<'de>>(
    auth: &JiraAuthConfig,
    request: reqwest::blocking::RequestBuilder,
) -> Result<T, String> {
    let request = match auth.auth_mode.as_str() {
        "pat" => request.bearer_auth(&auth.personal_access_token),
        "password" => request.basic_auth(&auth.username, Some(&auth.password)),
        _ => request.basic_auth(&auth.username, Some(&auth.api_token)),
    };

    let response = request.send().map_err(|error| {
        format!(
            "Failed to call Jira REST API. {}",
            describe_reqwest_error(&error)
        )
    })?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("Failed to read Jira REST response: {error}"))?;

    if !status.is_success() {
        return Err(format!("Jira REST API returned HTTP {status}: {body}"));
    }

    serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse Jira REST response: {error}. Body: {body}"))
}

fn send_empty(
    auth: &JiraAuthConfig,
    request: reqwest::blocking::RequestBuilder,
) -> Result<(), String> {
    let request = apply_auth(auth, request);
    let response = request.send().map_err(|error| {
        format!(
            "Failed to call Jira REST API. {}",
            describe_reqwest_error(&error)
        )
    })?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("Failed to read Jira REST response: {error}"))?;

    if !status.is_success() {
        return Err(format!("Jira REST API returned HTTP {status}: {body}"));
    }

    Ok(())
}

fn apply_auth(
    auth: &JiraAuthConfig,
    request: reqwest::blocking::RequestBuilder,
) -> reqwest::blocking::RequestBuilder {
    match auth.auth_mode.as_str() {
        "pat" => request.bearer_auth(&auth.personal_access_token),
        "password" => request.basic_auth(&auth.username, Some(&auth.password)),
        _ => request.basic_auth(&auth.username, Some(&auth.api_token)),
    }
}

fn describe_reqwest_error(error: &reqwest::Error) -> String {
    let mut details = vec![error.to_string()];
    let mut source = error.source();

    while let Some(error) = source {
        details.push(error.to_string());
        source = error.source();
    }

    let hint = if error.is_timeout() {
        "Hint: Jira did not respond before the 15 second timeout. Cached cards remain available; try a narrower JQL or sync fewer pages."
    } else if error.is_connect() {
        "Hint: Spacesly could not connect to Jira. Check VPN, proxy, DNS, or corporate certificate setup."
    } else if error.is_request() {
        "Hint: The Jira request could not be built or sent. Check the Jira URL format."
    } else {
        "Hint: Check Jira URL, VPN/proxy, and certificate access from this desktop app."
    };

    format!("{} {hint}", details.join(" | caused by: "))
}

fn normalized_base_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Jira URL is required.".to_string());
    }
    Ok(trimmed.to_string())
}

fn status_matches(actual: &str, target: &str) -> bool {
    let actual = normalize_status(actual);
    let target = normalize_status(target);
    actual == target || actual.contains(&target) || target.contains(&actual)
}

fn normalize_status(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn description_text(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text).filter(|text| !text.trim().is_empty()),
        serde_json::Value::Object(object) => object
            .get("content")
            .and_then(|content| content.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| description_text(item.clone()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|text| !text.trim().is_empty()),
        serde_json::Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|item| description_text(item.clone()))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .filter(|text| !text.trim().is_empty()),
        _ => None,
    }
}

#[derive(Deserialize)]
struct BoardPage {
    #[serde(rename = "maxResults")]
    max_results: i64,
    #[serde(rename = "isLast")]
    is_last: Option<bool>,
    total: Option<i64>,
    values: Vec<RestBoard>,
}

#[derive(Deserialize)]
struct RestBoard {
    id: i64,
    name: String,
    #[serde(rename = "type")]
    board_type: String,
}

#[derive(Deserialize)]
struct IssuePage {
    #[serde(rename = "maxResults")]
    max_results: i64,
    total: i64,
    issues: Vec<RestIssue>,
}

#[derive(Deserialize)]
struct RestIssue {
    key: String,
    fields: RestIssueFields,
}

#[derive(Deserialize)]
struct RestIssueFields {
    summary: String,
    description: Option<serde_json::Value>,
    #[serde(default)]
    labels: Vec<String>,
    status: NamedValue,
    #[serde(rename = "issuetype")]
    issue_type: NamedValue,
}

#[derive(Deserialize)]
struct NamedValue {
    name: String,
}

#[derive(Deserialize)]
struct TransitionPage {
    transitions: Vec<Transition>,
}

#[derive(Deserialize)]
struct Transition {
    id: String,
    name: String,
    to: NamedValue,
}
