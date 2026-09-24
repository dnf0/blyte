//! Suppresses a read whose lines the worker already holds.
//!
//! # Why this exists
//!
//! Invariant IV enforces context boundaries at the execution perimeter. Its named mechanisms
//! -- AST slicing for massive reads, output shunting, polling debounce -- were calibrated to a
//! problem that measurement says is small: whole-file reads contributed ~4,740 tokens against
//! a turn admitting 1.9M (FINDINGS 13.16). The guard was at its ceiling because there was
//! almost nothing left to deny.
//!
//! The volume is elsewhere. On two real tasks, 65% (katex) and 39% (scc) of all line-fetches
//! were of lines the worker had already been given, and 133 of 231 reads on katex were
//! *entirely* covered by lines already held (13.21). A tool call costs ~2,500 tokens of fixed
//! preamble before returning a byte, so 231 reads is ~577,500 tokens -- 93% of that task's
//! read cost -- spent on the calls themselves. Eliminating a redundant call is worth far more
//! than tuning how much each one returns: 45-68% against the read budget's 9-17%.
//!
//! # Why it returns a pointer, not the content
//!
//! Serving the cached lines back would save nothing. The lines are already in the transcript;
//! re-sending them pays the same tokens the read would have. The saving comes from answering
//! with a few dozen tokens that say *you already have these*, so neither the preamble nor a
//! duplicate copy enters context -- nor gets re-sent on every subsequent turn.
//!
//! # Why a forgetful worker cannot be trapped
//!
//! Context collapse is real and is the third failure the manifesto names. A worker whose
//! context was compacted may genuinely no longer hold what this cache believes it holds, and
//! a guard that refuses forever would deadlock it.
//!
//! Two mechanical escapes, both deterministic:
//!
//! * **Content change invalidates.** Records are keyed by a hash of the file, so any edit --
//!   the worker's own or a rollback's -- drops every prior record for that path. Re-reading
//!   after a write is never suppressed.
//! * **Insisting wins.** The identical window requested twice in a row is served. The worst
//!   case is one wasted denial, after which the worker gets exactly what it asked for.
//!
//! Neither escape depends on the worker understanding the denial text, which matters because
//! Invariant IV exists precisely because prose instructions do not bind an agent under
//! pressure.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One window of one file, as served to the worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadRecord {
    pub path: String,
    pub start: usize,
    pub end: usize,
    /// Hash of the file's content when this window was served. A different hash means the
    /// file changed and every record carrying the old one is stale.
    pub hash: u64,
    /// Whether this entry records a suppression rather than a served read. The most recent
    /// entry being a suppression of the same window is what makes insisting work.
    #[serde(default)]
    pub suppressed: bool,
}

/// What the cache says about a proposed read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheVerdict {
    /// Not held, or held under a different version of the file. Fall through to the budget.
    Miss,
    /// Every requested line is already in the worker's context and the file has not changed.
    AlreadyHeld {
        /// The windows that cover the request, for a denial message that can be acted on.
        covered_by: Vec<(usize, usize)>,
    },
    /// The identical window was suppressed on the previous request for this file. Serve it.
    Insisted,
}

/// FNV-1a over the file's bytes.
///
/// Deliberately not cryptographic and deliberately not `DefaultHasher`: this only needs to
/// answer "did the file change", and it must give the same answer in every process that reads
/// the same bytes. `DefaultHasher`'s output is explicitly not guaranteed stable across
/// releases, and the cache is read by a hook process separate from the one that wrote it.
pub fn content_hash(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Whether `[start, end]` is wholly contained in the union of `windows`.
///
/// Works on the union rather than any single window: a worker that read 1-100 and then
/// 101-200 holds 50-150 even though no single call fetched it.
fn covered(windows: &[(usize, usize)], start: usize, end: usize) -> bool {
    if start > end {
        return false;
    }
    let mut sorted: Vec<(usize, usize)> = windows.to_vec();
    sorted.sort_unstable();
    let mut reach = start;
    for (a, b) in sorted {
        if a > reach {
            return false; // a gap opens before the request is satisfied
        }
        if b >= reach {
            reach = b + 1;
        }
        if reach > end {
            return true;
        }
    }
    reach > end
}

/// Decide what to do with a proposed read, given the records already written.
pub fn evaluate(
    records: &[ReadRecord],
    target: &Path,
    start: usize,
    end: usize,
    hash: u64,
) -> CacheVerdict {
    let key = target.to_string_lossy();

    // Only records for this file AND this version of it. An edit invalidates everything,
    // which is what makes a read-after-write always go through.
    let live: Vec<&ReadRecord> = records
        .iter()
        .filter(|r| r.path == key && r.hash == hash)
        .collect();

    // Insisting wins: the last thing that happened to this file was us refusing exactly this
    // window. Serve it rather than refuse twice.
    if let Some(last) = live.last() {
        if last.suppressed && last.start == start && last.end == end {
            return CacheVerdict::Insisted;
        }
    }

    let served: Vec<(usize, usize)> = live
        .iter()
        .filter(|r| !r.suppressed)
        .map(|r| (r.start, r.end))
        .collect();

    if !served.is_empty() && covered(&served, start, end) {
        return CacheVerdict::AlreadyHeld { covered_by: served };
    }
    CacheVerdict::Miss
}

/// The denial text. Short by design -- its whole purpose is to cost less than the read.
pub fn suppression_reason(target: &Path, start: usize, end: usize) -> String {
    format!(
        "[blyte-guard] Already in context: {} lines {}-{}.\n\
         You fetched these lines earlier and the file has not changed since, so re-reading \
         would re-send what you already have. Scroll back rather than re-fetching.\n\
         If you truly need them again, issue this identical read once more and it will be \
         served.",
        target.display(),
        start,
        end
    )
}
