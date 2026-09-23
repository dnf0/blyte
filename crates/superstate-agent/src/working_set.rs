//! Working set data structures and LRU cache management.
//!
//! This module maintains a bounded working set of actively viewed and edited file
//! snippets in external working memory. It lives strictly within the harness `Runtime`
//! (Invariant I), ensuring the agent cannot mutate or clear observed facts.

use serde::{Deserialize, Serialize};

/// Maximum active files kept in context memory before LRU eviction triggers.
pub const MAX_ACTIVE_FILES: usize = 3;

/// Maximum lines kept per individual file snippet before head/tail truncation.
pub const MAX_FILE_LINES: usize = 100;

/// Maximum aggregate lines allowed across all file snippets in the working set.
pub const MAX_TOTAL_LINES: usize = 250;

/// A cached file snippet actively inspected or modified by the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSnippet {
    /// Relative or canonical path to the file.
    pub path: String,
    /// Cached text content, potentially distilled to stay within line bounds.
    pub content: String,
    /// 1-indexed starting line number of the snippet, if partial read.
    pub start_line: Option<usize>,
    /// 1-indexed ending line number of the snippet, if partial read.
    pub end_line: Option<usize>,
    /// Session turn number when this file was last read or edited.
    pub last_accessed_turn: u32,
}

/// Harness-managed working set representing active external working memory.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkingSet {
    /// Active file snippets. Insertion order is preserved; LRU eviction is ordered
    /// strictly by `last_accessed_turn`.
    pub files: Vec<FileSnippet>,
    /// Distilled output of the most recent failed gate or command execution.
    pub last_failure: Option<String>,
}

/// Distills snippet text if line count exceeds `MAX_FILE_LINES`.
fn distill_snippet(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() > MAX_FILE_LINES {
        let half = MAX_FILE_LINES / 2;
        let head = &lines[..half];
        let tail = &lines[lines.len() - half..];
        let omitted = lines.len() - MAX_FILE_LINES;
        format!(
            "{}\n\n[... {} lines omitted ...]\n\n{}",
            head.join("\n"),
            omitted,
            tail.join("\n")
        )
    } else {
        content.to_string()
    }
}

impl WorkingSet {
    /// Add or update a file snippet in the working set, updating its access turn.
    ///
    /// Distills the snippet content if it exceeds `MAX_FILE_LINES`, updates an
    /// existing entry if present, or pushes a new entry and triggers LRU pruning.
    ///
    /// # Parameters
    /// - `path`: File path identifier.
    /// - `content`: Raw file or slice content.
    /// - `start_line`: Optional starting line index.
    /// - `end_line`: Optional ending line index.
    /// - `turn`: Current session turn number.
    pub fn add_or_update(
        &mut self,
        path: &str,
        content: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
        turn: u32,
    ) {
        // Enforce per-file line limits via distillation
        let distilled = distill_snippet(content);

        // Update existing entry or append a new snippet
        if let Some(existing) = self.files.iter_mut().find(|f| f.path == path) {
            existing.content = distilled;
            existing.start_line = start_line;
            existing.end_line = end_line;
            existing.last_accessed_turn = turn;
        } else {
            self.files.push(FileSnippet {
                path: path.to_string(),
                content: distilled,
                start_line,
                end_line,
                last_accessed_turn: turn,
            });
        }

        // Enforce capacity bounds across file count and aggregate lines
        self.prune_lru(MAX_ACTIVE_FILES, MAX_TOTAL_LINES);
    }

    /// Apply an exact string replacement chunk to cached content in memory.
    ///
    /// # Parameters
    /// - `path`: Target file path.
    /// - `old`: Exact string slice to replace.
    /// - `new`: Replacement string slice.
    /// - `turn`: Current turn number for LRU updating.
    ///
    /// # Returns
    /// `true` if the file was found and updated; `false` otherwise.
    pub fn apply_edit(&mut self, path: &str, old: &str, new: &str, turn: u32) -> bool {
        // Reject empty search string to prevent unintentional prepending
        if old.is_empty() {
            return false;
        }

        // Find cached file matching path and containing target substring
        let mut edited = false;
        if let Some(existing) = self.files.iter_mut().find(|f| f.path == path) {
            let replaced = if existing.content.contains(old) {
                Some(existing.content.replacen(old, new, 1))
            } else if let Some(trimmed_old) = old.strip_suffix('\n') {
                if !trimmed_old.is_empty()
                    && existing.end_line.is_none()
                    && existing.content.ends_with(trimmed_old)
                {
                    let prefix = &existing.content[..existing.content.len() - trimmed_old.len()];
                    Some(format!("{prefix}{new}"))
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(replaced) = replaced {
                existing.content = distill_snippet(&replaced);
                existing.last_accessed_turn = turn;

                // Adjust end_line if snippet bounds were tracked
                if let Some(end) = existing.end_line {
                    let old_lines = old.lines().count();
                    let new_lines = new.lines().count();
                    if new_lines >= old_lines {
                        existing.end_line = Some(end + (new_lines - old_lines));
                    } else {
                        existing.end_line = Some(end.saturating_sub(old_lines - new_lines));
                    }
                }
                edited = true;
            }
        }

        // Enforce aggregate line budget and capacity bounds after edit expansion
        if edited {
            self.prune_lru(MAX_ACTIVE_FILES, MAX_TOTAL_LINES);
        }
        edited
    }

    /// Record a distilled failure output from non-zero test or gate execution.
    ///
    /// Keeps head and tail lines if output exceeds 20 lines to prevent context bloat.
    ///
    /// # Parameters
    /// - `output`: Raw stderr/stdout output from command execution.
    pub fn record_failure(&mut self, output: &str) {
        // Filter out blank lines to focus on diagnostic messages
        let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();

        // Truncate middle lines if failure report is excessively verbose
        let distilled = if lines.len() > 20 {
            let head = &lines[..10];
            let tail = &lines[lines.len() - 10..];
            format!("{}\n...\n{}", head.join("\n"), tail.join("\n"))
        } else {
            lines.join("\n")
        };
        self.last_failure = Some(distilled);
    }

    /// Clear test failure output on passing execution.
    pub fn clear_failure(&mut self) {
        self.last_failure = None;
    }

    /// Evict least-recently-accessed files when exceeding file count or line budget.
    ///
    /// # Parameters
    /// - `max_files`: Maximum number of files permitted.
    /// - `max_lines_total`: Maximum total lines across all retained files.
    pub fn prune_lru(&mut self, max_files: usize, max_lines_total: usize) {
        // Evict oldest files when exceeding maximum file count
        while self.files.len() > max_files {
            if let Some(min_idx) = self
                .files
                .iter()
                .enumerate()
                .min_by_key(|(_, f)| f.last_accessed_turn)
                .map(|(i, _)| i)
            {
                self.files.remove(min_idx);
            } else {
                break;
            }
        }

        // Evict oldest files when aggregate line budget is exceeded, keeping at least 1 file
        let mut total_lines: usize = self.files.iter().map(|f| f.content.lines().count()).sum();
        while total_lines > max_lines_total && self.files.len() > 1 {
            if let Some(min_idx) = self
                .files
                .iter()
                .enumerate()
                .min_by_key(|(_, f)| f.last_accessed_turn)
                .map(|(i, _)| i)
            {
                self.files.remove(min_idx);
                total_lines = self.files.iter().map(|f| f.content.lines().count()).sum();
            } else {
                break;
            }
        }
    }
}
