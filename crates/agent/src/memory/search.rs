//! Advanced memory file search, aligned with Codex ext/memories search.
//!
//! Supports multiple match modes (Any, AllOnSameLine, AllWithinLines),
//! case sensitivity, normalization, context lines, and cursor-based pagination.

use crate::memory::types::{SearchMatchMode, SearchQuery, SearchResponse, SearchResult};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Maximum number of search results per query.
#[allow(dead_code)]
const MAX_SEARCH_RESULTS: usize = 200;

/// Perform an advanced search across memory files under `root`.
#[allow(dead_code)]
pub(crate) fn search_memory_files_advanced(
    root: &Path,
    query: &SearchQuery,
) -> Result<SearchResponse, String> {
    let queries: Vec<String> = query.queries.iter().map(|q| q.trim().to_string()).collect();
    if queries.is_empty() || queries.iter().any(|q| q.is_empty()) {
        return Err("queries must not be empty or contain empty strings".to_string());
    }
    if matches!(
        query.match_mode,
        SearchMatchMode::AllWithinLines { line_count: 0 }
    ) {
        return Err("all_within_lines.line_count must be a positive integer".to_string());
    }

    let max_results = query.max_results.min(MAX_SEARCH_RESULTS);
    let start_path = resolve_search_path(root, query.path.as_deref())?;
    let start_index = match query.cursor.as_deref() {
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| format!("invalid cursor: {cursor}"))?,
        None => 0,
    };

    let matcher = SearchMatcher::new(
        queries.clone(),
        query.match_mode.clone(),
        query.case_sensitive,
        query.normalize,
    );

    let mut matches = Vec::new();
    search_entries(
        root,
        &start_path,
        &matcher,
        query.context_lines,
        &mut matches,
    )?;

    // Sort by path then line number for deterministic output.
    matches.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then(a.line_number.cmp(&b.line_number))
    });

    // Apply cursor-based pagination.
    if start_index > matches.len() {
        return Err(format!("cursor {start_index} exceeds result count"));
    }
    let end_index = start_index.saturating_add(max_results).min(matches.len());
    let next_cursor = if end_index < matches.len() {
        Some(end_index.to_string())
    } else {
        None
    };
    let truncated = next_cursor.is_some();

    debug!(
        total_matches = matches.len(),
        returned = end_index - start_index,
        "memory search completed"
    );

    Ok(SearchResponse {
        queries,
        match_mode: query.match_mode.clone(),
        path: query.path.clone(),
        matches: matches[start_index..end_index].to_vec(),
        next_cursor,
        truncated,
    })
}

// ---------------------------------------------------------------------------
// Internal search logic
// ---------------------------------------------------------------------------

fn resolve_search_path(root: &Path, relative: Option<&str>) -> Result<PathBuf, String> {
    match relative {
        None => Ok(root.to_path_buf()),
        Some(rel) => {
            let path = root.join(rel);
            // Ensure no path traversal outside root.
            let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !canonical_path.starts_with(&canonical_root) {
                return Err(format!("path '{}' escapes the memory root", rel));
            }
            Ok(path)
        }
    }
}

fn search_entries(
    root: &Path,
    current: &Path,
    matcher: &SearchMatcher,
    context_lines: usize,
    matches: &mut Vec<SearchResult>,
) -> Result<(), String> {
    if !current.exists() {
        return Ok(());
    }
    let metadata =
        std::fs::metadata(current).map_err(|e| format!("metadata {}: {e}", current.display()))?;

    if metadata.is_file() {
        search_file(root, current, matcher, context_lines, matches)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    let mut pending = vec![current.to_path_buf()];
    while let Some(dir_path) = pending.pop() {
        let entries = std::fs::read_dir(&dir_path)
            .map_err(|e| format!("read_dir {}: {e}", dir_path.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden_path(&path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() {
                search_file(root, &path, matcher, context_lines, matches)?;
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn search_file(
    root: &Path,
    path: &Path,
    matcher: &SearchMatcher,
    context_lines: usize,
    matches: &mut Vec<SearchResult>,
) -> Result<(), String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Ok(()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };

    let lines: Vec<&str> = content.lines().collect();
    let line_flags: Vec<Vec<bool>> = lines
        .iter()
        .map(|line| matcher.matched_query_flags(line))
        .collect();

    let relative_path = display_relative_path(root, path);

    match &matcher.match_mode {
        SearchMatchMode::Any => {
            for (idx, flags) in line_flags.iter().enumerate() {
                if flags.iter().any(|m| *m) {
                    matches.push(build_match(
                        &relative_path,
                        &lines,
                        idx,
                        idx,
                        context_lines,
                        matcher.matched_queries(flags),
                    ));
                }
            }
        }
        SearchMatchMode::AllOnSameLine => {
            for (idx, flags) in line_flags.iter().enumerate() {
                if flags.iter().all(|m| *m) {
                    matches.push(build_match(
                        &relative_path,
                        &lines,
                        idx,
                        idx,
                        context_lines,
                        matcher.matched_queries(flags),
                    ));
                }
            }
        }
        SearchMatchMode::AllWithinLines { line_count } => {
            let mut windows: Vec<(usize, usize, Vec<bool>)> = Vec::new();
            for start_idx in 0..lines.len() {
                if !line_flags[start_idx].iter().any(|m| *m) {
                    continue;
                }
                let last_allowed = start_idx
                    .saturating_add(line_count.saturating_sub(1))
                    .min(lines.len().saturating_sub(1));
                let mut combined_flags = vec![false; matcher.queries.len()];
                for (end_idx, flags) in line_flags
                    .iter()
                    .enumerate()
                    .take(last_allowed + 1)
                    .skip(start_idx)
                {
                    for (i, matched) in flags.iter().enumerate() {
                        combined_flags[i] |= matched;
                    }
                    if combined_flags.iter().all(|m| *m) {
                        windows.push((start_idx, end_idx, combined_flags));
                        break;
                    }
                }
            }
            // Remove windows that strictly contain another window.
            for (idx, (start, end, flags)) in windows.iter().enumerate() {
                let is_superset = windows.iter().enumerate().any(|(other_idx, (os, oe, _))| {
                    idx != other_idx && start <= os && end >= oe && (start != os || end != oe)
                });
                if !is_superset {
                    matches.push(build_match(
                        &relative_path,
                        &lines,
                        *start,
                        *end,
                        context_lines,
                        matcher.matched_queries(flags),
                    ));
                }
            }
        }
    }

    Ok(())
}

fn build_match(
    file_path: &str,
    lines: &[&str],
    match_start: usize,
    match_end: usize,
    context_lines: usize,
    matched_queries: Vec<String>,
) -> SearchResult {
    let content_start = match_start.saturating_sub(context_lines);
    let content_end = match_end
        .saturating_add(context_lines)
        .saturating_add(1)
        .min(lines.len());
    SearchResult {
        file_path: file_path.to_string(),
        line_number: match_start + 1,
        content_start_line_number: content_start + 1,
        content: lines[content_start..content_end].join("\n"),
        matched_queries,
    }
}

fn display_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[allow(dead_code)]
fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

// ---------------------------------------------------------------------------
// Search matcher (case sensitivity + normalization)
// ---------------------------------------------------------------------------

struct SearchMatcher {
    queries: Vec<String>,
    prepared_queries: Vec<String>,
    comparison: SearchComparison,
    match_mode: SearchMatchMode,
}

impl SearchMatcher {
    fn new(
        queries: Vec<String>,
        match_mode: SearchMatchMode,
        case_sensitive: bool,
        normalized: bool,
    ) -> Self {
        let comparison = SearchComparison::new(case_sensitive, normalized);
        let prepared_queries: Vec<String> = queries
            .iter()
            .map(|q| comparison.prepare(q).into_owned())
            .collect();
        Self {
            queries,
            prepared_queries,
            comparison,
            match_mode,
        }
    }

    fn matched_query_flags(&self, line: &str) -> Vec<bool> {
        let prepared_line = self.comparison.prepare(line);
        self.prepared_queries
            .iter()
            .map(|q| prepared_line.contains(q.as_str()))
            .collect()
    }

    fn matched_queries(&self, flags: &[bool]) -> Vec<String> {
        self.queries
            .iter()
            .zip(flags)
            .filter_map(|(q, matched)| if *matched { Some(q.clone()) } else { None })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct SearchComparison {
    case_sensitive: bool,
    normalized: bool,
}

#[allow(dead_code)]
impl SearchComparison {
    fn new(case_sensitive: bool, normalized: bool) -> Self {
        Self {
            case_sensitive,
            normalized,
        }
    }

    fn prepare<'a>(&self, value: &'a str) -> Cow<'a, str> {
        if self.case_sensitive && !self.normalized {
            return Cow::Borrowed(value);
        }
        let value = if self.case_sensitive {
            Cow::Borrowed(value)
        } else {
            Cow::Owned(value.to_lowercase())
        };
        if !self.normalized {
            return value;
        }
        Cow::Owned(
            value
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .collect::<String>(),
        )
    }
}
