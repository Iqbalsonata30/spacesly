use ignore::WalkBuilder;
use regex::{NoExpand, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::files::{atomic_write, file_version};

const MAX_QUERY_CHARS: usize = 512;
const MAX_RESULTS: usize = 1_000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_WALK_ENTRIES: usize = 100_000;
const MAX_PREVIEW_CHARS: usize = 500;
const MAX_REPLACEMENT_BYTES: usize = 64 * 1024;
const MAX_REPLACE_FILES: usize = 200;
const MAX_REPLACEMENTS: usize = 5_000;

#[derive(Clone, Debug, Deserialize)]
pub struct WorkspaceSearchRequest {
    pub workspace_id: String,
    pub query: String,
    pub case_sensitive: bool,
    pub max_results: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceSearchResult {
    pub file_path: String,
    pub line: usize,
    pub character: usize,
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceSearchResponse {
    pub results: Vec<WorkspaceSearchResult>,
    pub files_searched: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkspaceReplacePreviewRequest {
    pub workspace_id: String,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct WorkspaceReplacePlanFile {
    pub file_path: String,
    pub version: String,
    pub replacement_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceReplacePreviewFile {
    pub file_path: String,
    pub version: String,
    pub replacement_count: usize,
    pub before_preview: String,
    pub after_preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceReplacePreviewResponse {
    pub files: Vec<WorkspaceReplacePreviewFile>,
    pub total_replacements: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkspaceReplaceApplyRequest {
    pub workspace_id: String,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub files: Vec<WorkspaceReplacePlanFile>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceReplaceAppliedFile {
    pub file_path: String,
    pub replacement_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceReplaceApplyResponse {
    pub files: Vec<WorkspaceReplaceAppliedFile>,
    pub total_replacements: usize,
}

struct PendingReplacement {
    file_path: String,
    path: PathBuf,
    version: String,
    replacement_count: usize,
    content: Vec<u8>,
}

struct ReplacementScan {
    response: WorkspaceReplacePreviewResponse,
    pending: Vec<PendingReplacement>,
}

pub fn search_workspace(
    root: &Path,
    query: &str,
    case_sensitive: bool,
    max_results: usize,
) -> Result<WorkspaceSearchResponse, String> {
    validate_query(query)?;
    let matcher = literal_matcher(query, case_sensitive)?;
    let max_results = max_results.clamp(1, MAX_RESULTS);
    let mut response = WorkspaceSearchResponse {
        results: Vec::new(),
        files_searched: 0,
        truncated: false,
    };
    let mut walked_entries = 0;

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(true)
        .follow_links(false);

    for entry in builder.build().filter_map(Result::ok) {
        walked_entries += 1;
        if walked_entries > MAX_WALK_ENTRIES {
            response.truncated = true;
            break;
        }
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let path = entry.path();
        let Some(relative_path) = path
            .strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .map(|path| path.replace('\\', "/"))
        else {
            continue;
        };
        let Some((_, content)) = read_searchable_text(path) else {
            continue;
        };
        response.files_searched += 1;

        if append_matches(
            &mut response.results,
            &matcher,
            &relative_path,
            &content,
            max_results,
        ) {
            response.truncated = true;
            break;
        }
    }

    Ok(response)
}

pub fn preview_workspace_replace(
    root: &Path,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> Result<WorkspaceReplacePreviewResponse, String> {
    Ok(scan_workspace_replace(root, query, replacement, case_sensitive)?.response)
}

pub fn apply_workspace_replace(
    root: &Path,
    request: WorkspaceReplaceApplyRequest,
) -> Result<WorkspaceReplaceApplyResponse, String> {
    validate_replace_input(&request.query, &request.replacement)?;
    if request.truncated {
        return Err(
            "Truncated replacement plans cannot be applied. Run a narrower preview first."
                .to_string(),
        );
    }
    if request.files.len() > MAX_REPLACE_FILES {
        return Err(format!(
            "Replacement plan exceeds the {MAX_REPLACE_FILES}-file limit."
        ));
    }

    let mut submitted = HashMap::new();
    let mut submitted_total = 0usize;
    for file in &request.files {
        if file.replacement_count == 0 {
            return Err(format!(
                "Replacement plan entry '{}' has no replacements.",
                file.file_path
            ));
        }
        submitted_total = submitted_total
            .checked_add(file.replacement_count)
            .ok_or_else(|| "Replacement count overflowed.".to_string())?;
        if submitted_total > MAX_REPLACEMENTS {
            return Err(format!(
                "Replacement plan exceeds the {MAX_REPLACEMENTS}-replacement limit."
            ));
        }
        let resolved = resolve_existing_file(root, &file.file_path)?;
        if submitted
            .insert(file.file_path.clone(), (file, resolved))
            .is_some()
        {
            return Err(format!(
                "Replacement plan contains duplicate path '{}'.",
                file.file_path
            ));
        }
    }

    let scan = scan_workspace_replace(
        root,
        &request.query,
        &request.replacement,
        request.case_sensitive,
    )?;
    if scan.response.truncated {
        return Err("The current replacement plan is truncated and cannot be applied. Run a narrower preview first."
            .to_string());
    }
    if scan.pending.len() != submitted.len() {
        return Err("Workspace matches changed on disk after the replacement preview. Preview again before applying."
            .to_string());
    }

    let pending_by_path = scan
        .pending
        .into_iter()
        .map(|file| (file.file_path.clone(), file))
        .collect::<HashMap<_, _>>();
    let mut writes = Vec::with_capacity(request.files.len());
    for planned in &request.files {
        let (_, resolved) = submitted
            .get(&planned.file_path)
            .expect("submitted plan was indexed above");
        let current = pending_by_path.get(&planned.file_path).ok_or_else(|| {
            format!(
                "Workspace matches changed on disk for '{}'. Preview again before applying.",
                planned.file_path
            )
        })?;
        if &current.path != resolved {
            return Err(format!(
                "File path '{}' changed on disk after the replacement preview. Preview again before applying.",
                planned.file_path
            ));
        }
        if current.version != planned.version {
            return Err(format!(
                "File '{}' changed on disk after the replacement preview. Preview again before applying.",
                planned.file_path
            ));
        }
        if current.replacement_count != planned.replacement_count {
            return Err(format!(
                "Replacement count for '{}' changed on disk after the preview. Preview again before applying.",
                planned.file_path
            ));
        }
        writes.push(current);
    }

    // Validate the complete plan once more before the first mutation so a conflict
    // cannot leave unrelated planned files partially updated.
    for file in &writes {
        let bytes = fs::read(&file.path).map_err(|error| {
            format!(
                "Failed to verify '{}' before applying replacements: {error}",
                file.file_path
            )
        })?;
        if file_version(&bytes) != file.version {
            return Err(format!(
                "File '{}' changed on disk after the replacement preview. Preview again before applying.",
                file.file_path
            ));
        }
    }

    let mut changed = Vec::with_capacity(writes.len());
    for file in writes {
        if let Err(error) = atomic_write(&file.path, &file.content) {
            return Err(format!(
                "Failed to apply replacements to '{}': {error}. Each individual file write is atomic, but multi-file OS writes cannot be globally atomic; {} earlier file(s) may already have changed.",
                file.file_path,
                changed.len()
            ));
        }
        changed.push(WorkspaceReplaceAppliedFile {
            file_path: file.file_path.clone(),
            replacement_count: file.replacement_count,
        });
    }

    Ok(WorkspaceReplaceApplyResponse {
        files: changed,
        total_replacements: submitted_total,
    })
}

fn scan_workspace_replace(
    root: &Path,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
) -> Result<ReplacementScan, String> {
    validate_replace_input(query, replacement)?;
    let matcher = literal_matcher(query, case_sensitive)?;
    let mut response = WorkspaceReplacePreviewResponse {
        files: Vec::new(),
        total_replacements: 0,
        truncated: false,
    };
    let mut pending = Vec::new();
    let mut walked_entries = 0;

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(true)
        .follow_links(false);

    for entry in builder.build().filter_map(Result::ok) {
        walked_entries += 1;
        if walked_entries > MAX_WALK_ENTRIES {
            response.truncated = true;
            break;
        }
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let path = entry.path();
        let Some(relative_path) = path
            .strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .map(|path| path.replace('\\', "/"))
        else {
            continue;
        };
        let Some((bytes, content)) = read_searchable_text(path) else {
            continue;
        };
        let replacement_count = matcher.find_iter(&content).count();
        if replacement_count == 0 {
            continue;
        }
        if response.files.len() == MAX_REPLACE_FILES
            || replacement_count > MAX_REPLACEMENTS - response.total_replacements
        {
            response.truncated = true;
            break;
        }

        let first_match = matcher
            .find(&content)
            .expect("a positive replacement count has a first match");
        let replaced = matcher
            .replace_all(&content, NoExpand(replacement))
            .into_owned();
        let version = file_version(&bytes);
        response.files.push(WorkspaceReplacePreviewFile {
            file_path: relative_path.clone(),
            version: version.clone(),
            replacement_count,
            before_preview: capped_content_preview(&content, first_match.start()),
            after_preview: capped_content_preview(&replaced, first_match.start()),
        });
        response.total_replacements += replacement_count;
        pending.push(PendingReplacement {
            file_path: relative_path,
            path: path.to_path_buf(),
            version,
            replacement_count,
            content: replaced.into_bytes(),
        });
    }

    Ok(ReplacementScan { response, pending })
}

fn validate_query(query: &str) -> Result<(), String> {
    if query.trim().is_empty() {
        return Err("Search query must not be blank.".to_string());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!(
            "Search query exceeds the {MAX_QUERY_CHARS}-character limit."
        ));
    }
    Ok(())
}

fn validate_replace_input(query: &str, replacement: &str) -> Result<(), String> {
    validate_query(query)?;
    if replacement.len() > MAX_REPLACEMENT_BYTES {
        return Err(format!(
            "Replacement exceeds the {MAX_REPLACEMENT_BYTES}-byte limit."
        ));
    }
    Ok(())
}

fn literal_matcher(query: &str, case_sensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(&regex::escape(query))
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| format!("Failed to build search query: {error}"))
}

fn read_searchable_text(path: &Path) -> Option<(Vec<u8>, String)> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_FILE_BYTES {
        return None;
    }

    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES || bytes.contains(&0) {
        return None;
    }
    let content = String::from_utf8(bytes.clone()).ok()?;
    Some((bytes, content))
}

fn resolve_existing_file(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    if relative_path.is_empty() {
        return Err("File path is required.".to_string());
    }
    let path = Path::new(relative_path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("Invalid replacement file path '{relative_path}'."));
    }
    let resolved = root
        .join(path)
        .canonicalize()
        .map_err(|error| format!("Replacement file '{relative_path}' does not exist: {error}"))?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "Replacement file path '{relative_path}' escapes the workspace root."
        ));
    }
    if !resolved.is_file() {
        return Err(format!("Replacement path '{relative_path}' is not a file."));
    }
    Ok(resolved)
}

fn capped_content_preview(content: &str, match_byte: usize) -> String {
    let content_chars = content.chars().count();
    if content_chars <= MAX_PREVIEW_CHARS {
        return content.to_string();
    }

    let match_char = content[..match_byte].chars().count();
    let start = match_char.saturating_sub((MAX_PREVIEW_CHARS - 6) / 2);
    let has_prefix = start > 0;
    let content_limit = MAX_PREVIEW_CHARS - usize::from(has_prefix) * 3;
    let has_suffix = start + content_limit < content_chars;
    let content_limit = content_limit - usize::from(has_suffix) * 3;
    let preview = content
        .chars()
        .skip(start)
        .take(content_limit)
        .collect::<String>();
    match (has_prefix, has_suffix) {
        (true, true) => format!("...{preview}..."),
        (true, false) => format!("...{preview}"),
        (false, true) => format!("{preview}..."),
        (false, false) => preview,
    }
}

fn append_matches(
    results: &mut Vec<WorkspaceSearchResult>,
    matcher: &Regex,
    file_path: &str,
    content: &str,
    max_results: usize,
) -> bool {
    for (line_number, raw_line) in content.lines().enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        for found in matcher.find_iter(line) {
            if results.len() == max_results {
                return true;
            }
            results.push(WorkspaceSearchResult {
                file_path: file_path.to_string(),
                line: line_number,
                character: line[..found.start()].encode_utf16().count(),
                preview: capped_preview(line, found.start()),
            });
        }
    }
    false
}

fn capped_preview(line: &str, match_byte: usize) -> String {
    let line_chars = line.chars().count();
    if line_chars <= MAX_PREVIEW_CHARS {
        return line.to_string();
    }

    let match_char = line[..match_byte].chars().count();
    let start = match_char.saturating_sub((MAX_PREVIEW_CHARS - 6) / 2);
    let has_prefix = start > 0;
    let content_limit = MAX_PREVIEW_CHARS - usize::from(has_prefix) * 3;
    let has_suffix = start + content_limit < line_chars;
    let content_limit = content_limit - usize::from(has_suffix) * 3;
    let preview = line
        .chars()
        .skip(start)
        .take(content_limit)
        .collect::<String>();
    match (has_prefix, has_suffix) {
        (true, true) => format!("...{preview}..."),
        (true, false) => format!("...{preview}"),
        (false, true) => format!("{preview}..."),
        (false, false) => preview,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_matches, apply_workspace_replace, capped_preview, literal_matcher,
        preview_workspace_replace, search_workspace, validate_query, WorkspaceReplaceApplyRequest,
        WorkspaceReplacePlanFile, MAX_PREVIEW_CHARS, MAX_RESULTS,
    };
    use std::fs;

    #[test]
    fn literal_matching_respects_case_sensitivity() {
        let sensitive = literal_matcher("Needle.", true).unwrap();
        let insensitive = literal_matcher("Needle.", false).unwrap();

        assert!(sensitive.is_match("Needle."));
        assert!(!sensitive.is_match("needle."));
        assert!(insensitive.is_match("needle."));
        assert!(!insensitive.is_match("NeedleX"));
    }

    #[test]
    fn character_offsets_are_zero_based_utf16() {
        let matcher = literal_matcher("needle", true).unwrap();
        let mut results = Vec::new();

        assert!(!append_matches(
            &mut results,
            &matcher,
            "src/main.rs",
            "first\n😀needle",
            10,
        ));
        assert_eq!(results[0].line, 1);
        assert_eq!(results[0].character, 2);
    }

    #[test]
    fn query_and_result_bounds_are_enforced() {
        assert!(validate_query(" \n\t").is_err());
        assert!(validate_query(&"x".repeat(513)).is_err());

        let matcher = literal_matcher("x", true).unwrap();
        let mut results = Vec::new();
        let content = "x\n".repeat(MAX_RESULTS + 1);
        assert!(append_matches(
            &mut results,
            &matcher,
            "many.txt",
            &content,
            10_000usize.clamp(1, MAX_RESULTS),
        ));
        assert_eq!(results.len(), MAX_RESULTS);

        let long_line = format!("{}needle{}", "a".repeat(600), "b".repeat(600));
        assert!(capped_preview(&long_line, 600).chars().count() <= MAX_PREVIEW_CHARS);
    }

    #[test]
    fn ignored_and_hidden_files_are_excluded() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join(".git")).unwrap();
        fs::write(directory.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(directory.path().join("visible.txt"), "needle").unwrap();
        fs::write(directory.path().join("ignored.txt"), "needle").unwrap();
        fs::write(directory.path().join(".hidden.txt"), "needle").unwrap();

        let response = search_workspace(directory.path(), "needle", true, 10).unwrap();
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].file_path, "visible.txt");
        assert_eq!(response.files_searched, 1);
        assert!(!response.truncated);
    }

    #[test]
    fn replace_preview_is_literal_versioned_and_non_mutating() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("visible.txt");
        fs::write(&path, "Needle.$ needle.$ untouched").unwrap();

        let response =
            preview_workspace_replace(directory.path(), "needle.$", "$value", false).unwrap();

        assert_eq!(response.files.len(), 1);
        assert_eq!(response.total_replacements, 2);
        assert!(!response.truncated);
        assert_eq!(response.files[0].file_path, "visible.txt");
        assert_eq!(response.files[0].replacement_count, 2);
        assert!(!response.files[0].version.is_empty());
        assert_eq!(
            response.files[0].before_preview,
            "Needle.$ needle.$ untouched"
        );
        assert_eq!(response.files[0].after_preview, "$value $value untouched");
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "Needle.$ needle.$ untouched"
        );
    }

    #[test]
    fn applies_an_unchanged_preview_plan() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.txt");
        let second = directory.path().join("second.txt");
        fs::write(&first, "needle and needle").unwrap();
        fs::write(&second, "one needle").unwrap();
        let preview =
            preview_workspace_replace(directory.path(), "needle", "thread", true).unwrap();
        let files = preview
            .files
            .iter()
            .map(|file| WorkspaceReplacePlanFile {
                file_path: file.file_path.clone(),
                version: file.version.clone(),
                replacement_count: file.replacement_count,
            })
            .collect();

        let response = apply_workspace_replace(
            directory.path(),
            WorkspaceReplaceApplyRequest {
                workspace_id: "workspace-test".to_string(),
                query: "needle".to_string(),
                replacement: "thread".to_string(),
                case_sensitive: true,
                files,
                truncated: preview.truncated,
            },
        )
        .unwrap();

        assert_eq!(response.files.len(), 2);
        assert_eq!(response.total_replacements, 3);
        assert_eq!(fs::read_to_string(first).unwrap(), "thread and thread");
        assert_eq!(fs::read_to_string(second).unwrap(), "one thread");
    }

    #[test]
    fn changed_file_rejects_the_whole_plan_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.txt");
        let second = directory.path().join("second.txt");
        fs::write(&first, "needle first").unwrap();
        fs::write(&second, "needle second").unwrap();
        let preview =
            preview_workspace_replace(directory.path(), "needle", "thread", true).unwrap();
        let files = preview
            .files
            .iter()
            .map(|file| WorkspaceReplacePlanFile {
                file_path: file.file_path.clone(),
                version: file.version.clone(),
                replacement_count: file.replacement_count,
            })
            .collect();
        fs::write(&second, "needle changed externally").unwrap();

        let error = apply_workspace_replace(
            directory.path(),
            WorkspaceReplaceApplyRequest {
                workspace_id: "workspace-test".to_string(),
                query: "needle".to_string(),
                replacement: "thread".to_string(),
                case_sensitive: true,
                files,
                truncated: false,
            },
        )
        .unwrap_err();

        assert!(error.contains("changed on disk"));
        assert_eq!(fs::read_to_string(first).unwrap(), "needle first");
        assert_eq!(
            fs::read_to_string(second).unwrap(),
            "needle changed externally"
        );
    }

    #[test]
    fn truncated_or_tampered_plans_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("file.txt"), "needle").unwrap();
        let preview =
            preview_workspace_replace(directory.path(), "needle", "thread", true).unwrap();
        let mut file = WorkspaceReplacePlanFile {
            file_path: preview.files[0].file_path.clone(),
            version: preview.files[0].version.clone(),
            replacement_count: preview.files[0].replacement_count,
        };

        let request = |file, truncated| WorkspaceReplaceApplyRequest {
            workspace_id: "workspace-test".to_string(),
            query: "needle".to_string(),
            replacement: "thread".to_string(),
            case_sensitive: true,
            files: vec![file],
            truncated,
        };
        assert!(apply_workspace_replace(directory.path(), request(file.clone(), true)).is_err());

        file.replacement_count += 1;
        let error = apply_workspace_replace(directory.path(), request(file, false)).unwrap_err();
        assert!(error.contains("Replacement count"));
        assert_eq!(
            fs::read_to_string(directory.path().join("file.txt")).unwrap(),
            "needle"
        );
    }
}
