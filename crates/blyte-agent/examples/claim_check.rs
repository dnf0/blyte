//! Measure a model claim checker on saved research reports.
//!
//! `cargo run -p blyte-agent --example claim_check -- --manifest <m.json> --out <dir> [--model <id>] [--seed-per-report 5] [--max-cost <usd>]`

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use blyte_agent::fs::{Fs, LocalFs};
use blyte_agent::research_check::{self, Claim, Evidence, Sources};
use blyte_agent::shell::{LocalShell, Shell};
use blyte_provider::{pricing, resolve_provider, Message, Provider, Request, Usage};

#[derive(Debug, Clone, serde::Deserialize)]
struct ManifestEntry {
    id: String,
    report: PathBuf,
    roots: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct PoolClaim {
    id: usize,
    seeded: bool,
    original_id: Option<usize>,
    text: String,
    quoted: Option<String>,
    evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct VerdictItem {
    id: usize,
    verdict: String,
    reason: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ClaimOutputRow {
    report_id: String,
    claim_id: usize,
    seeded: bool,
    original_id: Option<usize>,
    text: String,
    verdict: String,
    reason: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct SummaryRow {
    id: String,
    claims: usize,
    seeded: usize,
    seeded_contradicted: usize,
    seeded_flagged_or_not_enough_evidence: usize,
    originals_contradicted: usize,
    originals_not_enough_evidence: usize,
    parse_errors: usize,
    model_calls: usize,
    input_tokens: u64,
    cached_tokens: u64,
    output_tokens: u64,
    thinking_tokens: u64,
    cost_usd: f64,
    seconds: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ExperimentSummary {
    model: String,
    max_cost_usd: f64,
    stopped_by_max_cost: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    reports: Vec<SummaryRow>,
    total: SummaryRow,
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    for (i, a) in args.iter().enumerate() {
        if a == flag {
            return args.get(i + 1).cloned();
        }
        if let Some(val) = a.strip_prefix(&prefix) {
            return Some(val.to_string());
        }
    }
    None
}

/// Deterministically select up to `max_seeds` claims (every k-th claim).
fn select_for_seeding(claims: &[Claim], max_seeds: usize) -> Vec<&Claim> {
    if max_seeds == 0 || claims.is_empty() {
        return Vec::new();
    }
    if claims.len() <= max_seeds {
        return claims.iter().collect();
    }
    let step = claims.len() / max_seeds;
    (0..max_seeds).map(|i| &claims[i * step]).collect()
}

/// Deterministic Fisher-Yates shuffle with a fixed seed, then renumber `id` 1..=len.
fn shuffle_and_renumber(items: &mut [PoolClaim], seed: u64) {
    let mut state = if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    };
    let len = items.len();
    for i in (1..len).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        items.swap(i, j);
    }
    for (idx, item) in items.iter_mut().enumerate() {
        item.id = idx + 1;
    }
}

/// Split items into batches of at most `batch_size`.
fn batch_slice<T>(items: &[T], batch_size: usize) -> Vec<&[T]> {
    let size = batch_size.max(1);
    items.chunks(size).collect()
}

fn format_evidence(evidence: &[Evidence]) -> String {
    let mut out = String::new();
    for ev in evidence {
        out.push_str(&format!("--- {}:{}-{} ---\n", ev.path, ev.start, ev.end));
        out.push_str(&ev.lines);
        out.push('\n');
    }
    out
}

fn build_seed_request(claim: &Claim) -> Request {
    let mut prompt = String::new();
    prompt.push_str(
        "Rewrite the following claim text so that it is false given the evidence below, \
         changing one specific fact (a number, a name, a condition, or a yes/no), while \
         keeping the same length and style. Reply with ONLY the rewritten claim text.\n\n",
    );
    prompt.push_str("## Claim\n");
    prompt.push_str(&claim.text);
    prompt.push('\n');
    if let Some(q) = &claim.quoted {
        prompt.push_str("\n## Quoted code\n```\n");
        prompt.push_str(q);
        prompt.push_str("\n```\n");
    }
    prompt.push_str("\n## Evidence\n");
    prompt.push_str(&format_evidence(&claim.evidence));

    Request {
        system: String::new(),
        messages: vec![Message::User(prompt)],
        tools: Vec::new(),
        max_output_tokens: None,
    }
}

fn build_check_request(batch: &[PoolClaim]) -> Request {
    let mut prompt = String::new();
    prompt.push_str(
        "For each claim below, using ONLY the evidence shown for that claim, determine whether \
         the claim is `supported`, `contradicted`, or `not_enough_evidence`, with a one-sentence \
         reason quoting the line number it relies on.\n\
         Reply with ONLY a JSON array of the form:\n\
         [{\"id\": 1, \"verdict\": \"supported\" | \"contradicted\" | \"not_enough_evidence\", \"reason\": \"...\"}]\n\n",
    );
    for c in batch {
        prompt.push_str(&format!("### Claim {}\n{}\n", c.id, c.text));
        if let Some(q) = &c.quoted {
            prompt.push_str("Quoted block:\n```\n");
            prompt.push_str(q);
            prompt.push_str("\n```\n");
        }
        prompt.push_str("Evidence:\n");
        prompt.push_str(&format_evidence(&c.evidence));
        prompt.push('\n');
    }

    Request {
        system: String::new(),
        messages: vec![Message::User(prompt)],
        tools: Vec::new(),
        max_output_tokens: None,
    }
}

fn extract_json_array(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if let Some(start) = trimmed.find("```json") {
        let rest = &trimmed[start + 7..];
        if let Some(end) = rest.find("```") {
            return Some(rest[..end].trim());
        }
    }
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        if let Some(end) = after[body_start..].find("```") {
            return Some(after[body_start..body_start + end].trim());
        }
    }
    let start = trimmed.find('[')?;
    let end = trimmed.rfind(']')?;
    (end >= start).then_some(&trimmed[start..=end])
}

/// Tolerantly parse a model's JSON array of `{"id", "verdict", "reason"}`.
fn parse_verdict_reply(text: &str) -> Result<Vec<VerdictItem>, String> {
    let candidate = extract_json_array(text).ok_or_else(|| "no JSON array found".to_string())?;
    let val: serde_json::Value = match serde_json::from_str(candidate) {
        Ok(v) => v,
        Err(e) => {
            if let (Some(s), Some(end)) = (text.find('['), text.rfind(']')) {
                if end >= s {
                    serde_json::from_str(&text[s..=end]).map_err(|e2| e2.to_string())?
                } else {
                    return Err(e.to_string());
                }
            } else {
                return Err(e.to_string());
            }
        }
    };
    let arr = val
        .as_array()
        .ok_or_else(|| "expected JSON array".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| "array item is not an object".to_string())?;
        let id = obj
            .get("id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| "missing numeric id".to_string())? as usize;
        let raw_verdict = obj
            .get("verdict")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing verdict".to_string())?
            .trim()
            .to_ascii_lowercase();
        if !matches!(
            raw_verdict.as_str(),
            "supported" | "contradicted" | "not_enough_evidence"
        ) {
            return Err(format!("invalid verdict: {raw_verdict}"));
        }
        let reason = obj
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(VerdictItem {
            id,
            verdict: raw_verdict,
            reason,
        });
    }
    Ok(out)
}

fn add_usage(dst: &mut Usage, src: &Usage) {
    dst.input += src.input;
    dst.cached += src.cached;
    dst.output += src.output;
    dst.thinking += src.thinking;
}

fn cost_of(model_id: &str, u: &Usage) -> f64 {
    pricing(model_id).map(|p| p.cost(u)).unwrap_or(0.0)
}

fn extract_report_claims(entry: &ManifestEntry, sh: &dyn Shell, fs: &dyn Fs) -> Vec<Claim> {
    let Ok(report) = fs.read(&entry.report) else {
        eprintln!(
            "warning: cannot read report {} for {}",
            entry.report.display(),
            entry.id
        );
        return Vec::new();
    };
    let default_root = entry
        .report
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let workspace = entry.roots.first().unwrap_or(&default_root);
    let read_roots: &[PathBuf] = if entry.roots.len() > 1 {
        &entry.roots[1..]
    } else {
        &[]
    };
    let mut listing = Vec::new();
    for dir in std::iter::once(workspace).chain(read_roots.iter()) {
        if let Ok(o) = sh.exec(
            "git -c core.quotePath=off -c core.fsmonitor=false ls-files -co --exclude-standard",
            dir,
            Duration::from_secs(60),
        ) {
            if o.exit_code == 0 {
                listing.extend(o.stdout.lines().map(|l| dir.join(l)));
            }
        }
    }
    let read = |p: &Path| fs.read(p);
    research_check::claims(
        &report,
        &Sources {
            workspace,
            read_roots,
            listing: &listing,
            read: &read,
        },
    )
}

fn print_table(rows: &[SummaryRow], total: &SummaryRow, stopped_by_max_cost: bool, max_cost: f64) {
    println!(
        "{:<16} {:>6} {:>6} {:>11} {:>12} {:>11} {:>8} {:>9} {:>5} {:>9} {:>9} {:>8} {:>7}",
        "report",
        "claims",
        "seeded",
        "seed_contra",
        "seed_flg/nee",
        "orig_contra",
        "orig_nee",
        "parse_err",
        "calls",
        "in_tok",
        "out_tok",
        "cost",
        "seconds"
    );
    println!("{}", "-".repeat(130));
    for r in rows.iter().chain(std::iter::once(total)) {
        println!(
            "{:<16} {:>6} {:>6} {:>11} {:>12} {:>11} {:>8} {:>9} {:>5} {:>9} {:>9} {:>8.4} {:>7.1}",
            r.id,
            r.claims,
            r.seeded,
            r.seeded_contradicted,
            r.seeded_flagged_or_not_enough_evidence,
            r.originals_contradicted,
            r.originals_not_enough_evidence,
            r.parse_errors,
            r.model_calls,
            r.input_tokens + r.cached_tokens,
            r.output_tokens + r.thinking_tokens,
            r.cost_usd,
            r.seconds
        );
    }
    if stopped_by_max_cost {
        println!("stopped early: --max-cost ${max_cost:.2} reached");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(manifest_path) = arg(&args, "--manifest") else {
        eprintln!(
            "usage: claim_check --manifest <m.json> --out <dir> [--model <id>] [--seed-per-report 5] [--max-cost <usd>]"
        );
        std::process::exit(2);
    };
    let Some(out_dir) = arg(&args, "--out").map(PathBuf::from) else {
        eprintln!(
            "usage: claim_check --manifest <m.json> --out <dir> [--model <id>] [--seed-per-report 5] [--max-cost <usd>]"
        );
        std::process::exit(2);
    };
    let model_arg = arg(&args, "--model");
    let seed_per_report: usize = arg(&args, "--seed-per-report")
        .map(|s| s.parse().expect("--seed-per-report N"))
        .unwrap_or(5);
    let max_cost: f64 = arg(&args, "--max-cost")
        .map(|s| s.parse().expect("--max-cost USD"))
        .unwrap_or(10.0);

    let manifest_text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("cannot read manifest {manifest_path}: {e}"));
    let entries: Vec<ManifestEntry> = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|e| panic!("invalid manifest {manifest_path}: {e}"));

    let provider: Box<dyn Provider> = match resolve_provider(model_arg.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let model_id = provider.model_id();
    std::fs::create_dir_all(&out_dir).expect("create --out dir");

    let sh = LocalShell;
    let fs = LocalFs;
    let mut all_rows: Vec<ClaimOutputRow> = Vec::new();
    let mut report_summaries: Vec<SummaryRow> = Vec::new();
    let mut total_usage = Usage::default();
    let mut total_calls = 0usize;
    let mut stopped_by_max_cost = false;
    let run_started = Instant::now();

    for entry in &entries {
        let rep_started = Instant::now();
        let extracted = extract_report_claims(entry, &sh, &fs);
        let mut rep_usage = Usage::default();
        let mut rep_calls = 0usize;

        let to_seed = select_for_seeding(&extracted, seed_per_report);
        let mut seeded_claims: Vec<PoolClaim> = Vec::new();

        for orig in to_seed {
            if cost_of(&model_id, &total_usage) >= max_cost {
                stopped_by_max_cost = true;
                break;
            }
            let req = build_seed_request(orig);
            rep_calls += 1;
            total_calls += 1;
            if let Ok(resp) = provider.complete(&req) {
                add_usage(&mut rep_usage, &resp.usage);
                add_usage(&mut total_usage, &resp.usage);
                let rewritten = resp.text.trim().to_string();
                if !rewritten.is_empty() {
                    seeded_claims.push(PoolClaim {
                        id: 0,
                        seeded: true,
                        original_id: Some(orig.id),
                        text: rewritten,
                        quoted: orig.quoted.clone(),
                        evidence: orig.evidence.clone(),
                    });
                }
            }
        }

        let mut pool: Vec<PoolClaim> = extracted
            .iter()
            .map(|c| PoolClaim {
                id: c.id,
                seeded: false,
                original_id: None,
                text: c.text.clone(),
                quoted: c.quoted.clone(),
                evidence: c.evidence.clone(),
            })
            .chain(seeded_claims)
            .collect();

        let seeded_count = pool.iter().filter(|c| c.seeded).count();
        shuffle_and_renumber(&mut pool, 0x517c_c1b7_2722_0a95);

        let mut rep_rows: Vec<ClaimOutputRow> = Vec::new();
        for batch in batch_slice(&pool, 15) {
            if cost_of(&model_id, &total_usage) >= max_cost {
                stopped_by_max_cost = true;
                break;
            }
            let req = build_check_request(batch);
            rep_calls += 1;
            total_calls += 1;
            let parsed = match provider.complete(&req) {
                Ok(resp) => {
                    add_usage(&mut rep_usage, &resp.usage);
                    add_usage(&mut total_usage, &resp.usage);
                    parse_verdict_reply(&resp.text).ok()
                }
                Err(_) => None,
            };

            match parsed {
                Some(items) => {
                    for c in batch {
                        if let Some(found) = items.iter().find(|it| it.id == c.id) {
                            rep_rows.push(ClaimOutputRow {
                                report_id: entry.id.clone(),
                                claim_id: c.id,
                                seeded: c.seeded,
                                original_id: c.original_id,
                                text: c.text.clone(),
                                verdict: found.verdict.clone(),
                                reason: found.reason.clone(),
                            });
                        } else {
                            rep_rows.push(ClaimOutputRow {
                                report_id: entry.id.clone(),
                                claim_id: c.id,
                                seeded: c.seeded,
                                original_id: c.original_id,
                                text: c.text.clone(),
                                verdict: "parse_error".to_string(),
                                reason: "missing claim id in batch response".to_string(),
                            });
                        }
                    }
                }
                None => {
                    for c in batch {
                        rep_rows.push(ClaimOutputRow {
                            report_id: entry.id.clone(),
                            claim_id: c.id,
                            seeded: c.seeded,
                            original_id: c.original_id,
                            text: c.text.clone(),
                            verdict: "parse_error".to_string(),
                            reason: "batch failed to parse".to_string(),
                        });
                    }
                }
            }
        }

        let summary = summarize_rows(
            &entry.id,
            extracted.len(),
            seeded_count,
            &rep_rows,
            rep_calls,
            &rep_usage,
            cost_of(&model_id, &rep_usage),
            rep_started.elapsed().as_secs_f64(),
        );
        report_summaries.push(summary);
        all_rows.extend(rep_rows);

        if stopped_by_max_cost {
            break;
        }
    }

    let total_claims: usize = report_summaries.iter().map(|r| r.claims).sum();
    let total_seeded: usize = report_summaries.iter().map(|r| r.seeded).sum();
    let total_summary = summarize_rows(
        "total",
        total_claims,
        total_seeded,
        &all_rows,
        total_calls,
        &total_usage,
        cost_of(&model_id, &total_usage),
        run_started.elapsed().as_secs_f64(),
    );

    let mut jsonl = String::new();
    for row in &all_rows {
        jsonl.push_str(&serde_json::to_string(row).unwrap());
        jsonl.push('\n');
    }
    std::fs::write(out_dir.join("claims.jsonl"), jsonl).expect("write claims.jsonl");

    let exp_summary = ExperimentSummary {
        model: model_id,
        max_cost_usd: max_cost,
        stopped_by_max_cost,
        note: stopped_by_max_cost
            .then(|| format!("stopped early: max cost ${max_cost:.2} reached")),
        reports: report_summaries.clone(),
        total: total_summary.clone(),
    };
    std::fs::write(
        out_dir.join("summary.json"),
        serde_json::to_string_pretty(&exp_summary).unwrap() + "\n",
    )
    .expect("write summary.json");

    print_table(
        &report_summaries,
        &total_summary,
        stopped_by_max_cost,
        max_cost,
    );
}

#[allow(clippy::too_many_arguments)]
fn summarize_rows(
    id: &str,
    claims: usize,
    seeded: usize,
    rows: &[ClaimOutputRow],
    model_calls: usize,
    usage: &Usage,
    cost_usd: f64,
    seconds: f64,
) -> SummaryRow {
    let seeded_contradicted = rows
        .iter()
        .filter(|r| r.seeded && r.verdict == "contradicted")
        .count();
    let seeded_flagged_or_not_enough_evidence = rows
        .iter()
        .filter(|r| {
            r.seeded && matches!(r.verdict.as_str(), "contradicted" | "not_enough_evidence")
        })
        .count();
    let originals_contradicted = rows
        .iter()
        .filter(|r| !r.seeded && r.verdict == "contradicted")
        .count();
    let originals_not_enough_evidence = rows
        .iter()
        .filter(|r| !r.seeded && r.verdict == "not_enough_evidence")
        .count();
    let parse_errors = rows.iter().filter(|r| r.verdict == "parse_error").count();

    SummaryRow {
        id: id.to_string(),
        claims,
        seeded,
        seeded_contradicted,
        seeded_flagged_or_not_enough_evidence,
        originals_contradicted,
        originals_not_enough_evidence,
        parse_errors,
        model_calls,
        input_tokens: usage.input,
        cached_tokens: usage.cached,
        output_tokens: usage.output,
        thinking_tokens: usage.thinking,
        cost_usd,
        seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_slice_caps_at_15() {
        let items: Vec<usize> = (1..=35).collect();
        let batches = batch_slice(&items, 15);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 15);
        assert_eq!(batches[1].len(), 15);
        assert_eq!(batches[2].len(), 5);
    }

    #[test]
    fn test_parse_verdict_reply_with_fences() {
        let reply = r#"Here are the verdicts:
```json
[
  {"id": 1, "verdict": "supported", "reason": "Line 12 returns 0 on empty input."},
  {"id": 2, "verdict": "contradicted", "reason": "Line 18 uses 5, not 10."},
  {"id": 3, "verdict": "not_enough_evidence", "reason": "Line 4 only declares the struct."}
]
```
"#;
        let parsed = parse_verdict_reply(reply).expect("should parse");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].id, 1);
        assert_eq!(parsed[0].verdict, "supported");
        assert_eq!(parsed[1].id, 2);
        assert_eq!(parsed[1].verdict, "contradicted");
        assert_eq!(parsed[2].id, 3);
        assert_eq!(parsed[2].verdict, "not_enough_evidence");
    }

    #[test]
    fn test_parse_verdict_reply_rejects_bad_json() {
        assert!(parse_verdict_reply("not json at all").is_err());
        assert!(parse_verdict_reply(r#"[{"id": 1, "verdict": "maybe", "reason": "x"}]"#).is_err());
    }

    #[test]
    fn test_select_for_seeding_deterministic() {
        let claims: Vec<Claim> = (1..=12)
            .map(|id| Claim {
                id,
                text: format!("claim {id}"),
                quoted: None,
                evidence: Vec::new(),
            })
            .collect();
        let picked: Vec<usize> = select_for_seeding(&claims, 5)
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(picked, vec![1, 3, 5, 7, 9]);
    }
}
