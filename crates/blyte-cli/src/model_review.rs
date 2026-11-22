use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Concern {
    pub file: String,
    pub line: Option<u64>,
    pub severity: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FallbackVerdict {
    pub file: String,
    pub line: usize,
    pub verdict: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReview {
    pub concerns: Vec<Concern>,
    pub fallbacks: Vec<FallbackVerdict>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedModelReview {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub concerns: Vec<Concern>,
    #[serde(default)]
    pub fallbacks: Vec<FallbackVerdict>,
    #[serde(default)]
    pub ts: u64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelReviewResult {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub concerns: Vec<Concern>,
    #[serde(default)]
    pub fallbacks: Vec<FallbackVerdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ModelReviewResult {
    pub fn ok(model: String, cost_usd: Option<f64>, concerns: Vec<Concern>) -> Self {
        Self {
            status: "ok".to_string(),
            model: Some(model),
            cost_usd,
            concerns,
            fallbacks: Vec::new(),
            error: None,
        }
    }

    pub fn with_fallbacks(mut self, fallbacks: Vec<FallbackVerdict>) -> Self {
        self.fallbacks = fallbacks;
        self
    }

    pub fn failed(model: Option<String>, cost_usd: Option<f64>, error: impl Into<String>) -> Self {
        Self {
            status: "failed".to_string(),
            model,
            cost_usd,
            concerns: Vec::new(),
            fallbacks: Vec::new(),
            error: Some(error.into()),
        }
    }

    pub fn has_findings(&self) -> bool {
        self.status == "ok" && !self.concerns.is_empty()
    }

    pub fn is_failed(&self) -> bool {
        self.status != "ok" || self.error.is_some()
    }

    pub fn waits_in_queue(&self) -> bool {
        self.has_findings() || self.is_failed()
    }

    pub fn flag(&self) -> Option<String> {
        if self.is_failed() {
            let reason = self.error.as_deref().unwrap_or("unknown error");
            return Some(sanitize_flag(format!("model review failed: {reason}")));
        }
        if self.has_findings() {
            let first = format_concern(&self.concerns[0]);
            let msg = if self.concerns.len() == 1 {
                format!("model review (1 concern): {first}")
            } else {
                format!(
                    "model review ({} concerns): {first} (and {} more)",
                    self.concerns.len(),
                    self.concerns.len() - 1
                )
            };
            return Some(sanitize_flag(msg));
        }
        None
    }
}

fn sanitize_flag(flag: String) -> String {
    let single_line = flag.lines().next().unwrap_or("").to_string();
    if single_line.chars().count() > 200 {
        single_line.chars().take(200).collect()
    } else {
        single_line
    }
}

pub const SYSTEM_PROMPT: &str = "\
You are a senior software engineer performing a first review of a code diff against a task specification.
Review the diff against the task. Raise only concrete problems a reviewer must look at:
- the task's requirements not met
- a behaviour or edge case the task names but the code or tests miss
- changes outside what the task asks for
- tests that could pass with a wrong implementation
- risky commands or deleted tests

Do not make style comments.
Raise at most 10 concerns.
Answer with only a JSON object with the following schema:
{\"concerns\": [{\"file\": \"<path>\", \"line\": <line number or null>, \"severity\": \"high\" | \"medium\" | \"low\", \"text\": \"<description>\"}], \"fallbacks\": [{\"file\": \"<path>\", \"line\": <line number>, \"verdict\": \"fallback\" | \"not_fallback\", \"reason\": \"<one sentence>\"}]}
Return {\"concerns\": [], \"fallbacks\": []} when there is nothing to raise and no fallback candidates were listed. Do not include any text or markdown outside the JSON object.";

/// System prompt + user message. The user message holds, in this order: the task text
/// (under "## Task"), `git diff --stat`, the full diff (under "## Diff"), and when
/// fallback candidates are present, a section listing each candidate and the fallback definition.
pub fn build_request(
    task: &str,
    stat: &str,
    diff: &str,
    candidates: &[crate::fallback_check::Finding],
) -> blyte_provider::Request {
    let diff_text = if diff.chars().count() > 200_000 {
        let cutoff = diff
            .char_indices()
            .nth(200_000)
            .map(|(i, _)| i)
            .unwrap_or(diff.len());
        let mut s = diff[..cutoff].to_string();
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("[diff truncated]\n");
        s
    } else {
        diff.to_string()
    };

    let mut user_msg = String::new();
    user_msg.push_str("## Task\n");
    user_msg.push_str(task);
    if !user_msg.ends_with('\n') {
        user_msg.push('\n');
    }
    user_msg.push('\n');

    let stat_trimmed = stat.trim();
    if !stat_trimmed.is_empty() {
        user_msg.push_str(stat_trimmed);
        user_msg.push_str("\n\n");
    }

    user_msg.push_str("## Diff\n");
    user_msg.push_str(&diff_text);
    if !user_msg.ends_with('\n') {
        user_msg.push('\n');
    }

    if !candidates.is_empty() {
        user_msg.push_str("\n## Fallback candidates\n");
        user_msg.push_str(
            "Judge each candidate below as \"fallback\" or \"not_fallback\" and return one entry in \"fallbacks\" for every candidate listed.\n\
A fallback is code that substitutes a default, a guess, an empty result or a silent skip when something is missing, unknown or unreadable, instead of failing with an error that names it; a documented, intended default is not a fallback; neither is string or path handling that cannot hide a missing input.\n\n",
        );
        for c in candidates {
            user_msg.push_str(&format!(
                "- {}:{} [{}] {}\n",
                c.file, c.line, c.pattern, c.text
            ));
        }
    }

    blyte_provider::Request {
        system: SYSTEM_PROMPT.to_string(),
        messages: vec![blyte_provider::Message::User(user_msg)],
        tools: Vec::new(),
        max_output_tokens: Some(32_000),
        timeout: None,
    }
}

/// Parse the model's JSON object reply `{"concerns": [...], "fallbacks": [...]}`.
/// A bare array is a parse error. An unknown fallback verdict is a parse error.
/// When `expect_fallbacks` is true, a missing `"fallbacks"` key is a parse error.
/// Empty or whitespace-only text → Err("empty response").
pub fn parse(text: &str, expect_fallbacks: bool) -> Result<ParsedReview, String> {
    if text.trim().is_empty() {
        return Err("empty response".to_string());
    }
    let candidate = extract_json_candidate(text);
    let value: serde_json::Value = match candidate {
        Some(s) => match serde_json::from_str(s) {
            Ok(v) => v,
            Err(_) => {
                if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
                    if end >= start {
                        match serde_json::from_str(&text[start..=end]) {
                            Ok(v) => v,
                            Err(_) => return Err(first_200(text)),
                        }
                    } else {
                        return Err(first_200(text));
                    }
                } else {
                    return Err(first_200(text));
                }
            }
        },
        None => return Err(first_200(text)),
    };

    let mut map = match value {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Array(_) => {
            return Err(format!(
                "expected JSON object, got bare array: {}",
                first_200(text)
            ));
        }
        _ => return Err(first_200(text)),
    };

    let concerns_arr = match map.remove("concerns") {
        Some(serde_json::Value::Array(arr)) => arr,
        _ => {
            return Err(format!(
                "missing or invalid \"concerns\" array: {}",
                first_200(text)
            ));
        }
    };

    let fallbacks_arr = match map.remove("fallbacks") {
        Some(serde_json::Value::Array(arr)) => arr,
        Some(_) => {
            return Err(format!(
                "invalid \"fallbacks\" field (expected array): {}",
                first_200(text)
            ));
        }
        None => {
            if expect_fallbacks {
                return Err("missing \"fallbacks\" key in model review response".to_string());
            }
            Vec::new()
        }
    };

    let mut concerns = Vec::new();
    for item in concerns_arr {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let text_val = match obj.get("text").and_then(|t| t.as_str()) {
            Some(t) => t.to_string(),
            None => continue,
        };
        let file = obj
            .get("file")
            .and_then(|f| f.as_str())
            .unwrap_or("")
            .to_string();
        let line = obj.get("line").and_then(|l| {
            l.as_u64()
                .or_else(|| l.as_str().and_then(|s| s.parse::<u64>().ok()))
        });
        let severity = match obj.get("severity").and_then(|s| s.as_str()) {
            Some(s) if s.eq_ignore_ascii_case("high") => "high".to_string(),
            Some(s) if s.eq_ignore_ascii_case("medium") => "medium".to_string(),
            Some(s) if s.eq_ignore_ascii_case("low") => "low".to_string(),
            _ => "low".to_string(),
        };
        concerns.push(Concern {
            file,
            line,
            severity,
            text: text_val,
        });
    }

    let mut fallbacks = Vec::with_capacity(fallbacks_arr.len());
    for item in fallbacks_arr {
        let Some(obj) = item.as_object() else {
            return Err(format!(
                "invalid fallback entry (expected object): {}",
                first_200(text)
            ));
        };
        let Some(file) = obj.get("file").and_then(|v| v.as_str()) else {
            return Err(format!(
                "fallback entry missing \"file\": {}",
                first_200(text)
            ));
        };
        let Some(line) = obj
            .get("line")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            })
            .and_then(|n| usize::try_from(n).ok())
        else {
            return Err(format!(
                "fallback entry missing valid \"line\": {}",
                first_200(text)
            ));
        };
        let Some(verdict) = obj.get("verdict").and_then(|v| v.as_str()) else {
            return Err(format!(
                "fallback entry missing \"verdict\": {}",
                first_200(text)
            ));
        };
        if verdict != "fallback" && verdict != "not_fallback" {
            return Err(format!(
                "unknown fallback verdict {verdict:?} (expected \"fallback\" or \"not_fallback\")"
            ));
        }
        let Some(reason) = obj.get("reason").and_then(|v| v.as_str()) else {
            return Err(format!(
                "fallback entry missing \"reason\": {}",
                first_200(text)
            ));
        };
        fallbacks.push(FallbackVerdict {
            file: file.to_string(),
            line,
            verdict: verdict.to_string(),
            reason: reason.to_string(),
        });
    }

    Ok(ParsedReview {
        concerns,
        fallbacks,
    })
}

fn first_200(text: &str) -> String {
    text.chars().take(200).collect()
}

fn extract_json_candidate(text: &str) -> Option<&str> {
    if let Some(start) = text.find("```json") {
        let content_start = start + 7;
        if let Some(end) = text[content_start..].find("```") {
            return Some(text[content_start..content_start + end].trim());
        }
    }
    if let Some(start) = text.find("```") {
        let after_fence = &text[start + 3..];
        let content_start = if let Some(nl) = after_fence.find('\n') {
            nl + 1
        } else {
            0
        };
        if let Some(end) = after_fence[content_start..].find("```") {
            return Some(after_fence[content_start..content_start + end].trim());
        }
    }
    let trimmed = text.trim();
    if trimmed.starts_with('[') {
        return Some(trimmed);
    }
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end >= start {
                return Some(&text[start..=end]);
            }
        }
    }
    None
}

fn severity_rank(sev: &str) -> u8 {
    match sev.to_ascii_lowercase().as_str() {
        "high" => 0,
        "medium" => 1,
        "low" => 2,
        _ => 3,
    }
}

pub fn parse_error_message(err: &str, finish: Option<&str>) -> String {
    match finish {
        Some(reason) if !reason.is_empty() => format!("{err} (finish reason: {reason})"),
        _ => err.to_string(),
    }
}

pub fn sort_concerns(concerns: &mut [Concern]) {
    concerns.sort_by_key(|c| severity_rank(&c.severity));
}

pub fn format_concern(c: &Concern) -> String {
    if let Some(line) = c.line {
        format!("[{}] {}:{} {}", c.severity, c.file, line, c.text)
    } else {
        format!("[{}] {} {}", c.severity, c.file, c.text)
    }
}

pub fn run(root: &Path, id: &str, base: &str, wt: &Path) -> ModelReviewResult {
    let start_path = root.join(".wt").join(format!("{id}.start.json"));
    let task_text = match read_task_text(root, &start_path) {
        Some(t) => t,
        None => {
            let result = ModelReviewResult::failed(None, None, "task file not found");
            save_review(
                root,
                id,
                None,
                None,
                Vec::new(),
                Vec::new(),
                result.error.clone(),
            );
            return result;
        }
    };

    if let Some(env_path) = crate::env_file::default_path() {
        let _ = crate::env_file::load(&env_path);
    }

    let repo_config = match crate::config::load_with_fallback(root) {
        Ok(Some((cfg, _))) => Some(cfg),
        Ok(None) => None,
        Err(e) => {
            let result = ModelReviewResult::failed(None, None, e);
            save_review(
                root,
                id,
                None,
                None,
                Vec::new(),
                Vec::new(),
                result.error.clone(),
            );
            return result;
        }
    };
    let dispatch_cfg = repo_config.map(|c| c.dispatch).unwrap_or_default();
    let model = dispatch_cfg.model.as_deref();

    // One call over a whole diff at high effort can take minutes; allow 10 unless the user set a limit.
    if std::env::var_os("BLYTE_MODEL_TIMEOUT_SECS").is_none() {
        std::env::set_var("BLYTE_MODEL_TIMEOUT_SECS", "600");
    }
    let provider = match blyte_provider::resolve_provider(model) {
        Ok(p) => p,
        Err(blyte_provider::ProviderError::MissingKey(key)) => {
            let result = ModelReviewResult::failed(None, None, format!("no API key (set {key})"));
            save_review(
                root,
                id,
                None,
                None,
                Vec::new(),
                Vec::new(),
                result.error.clone(),
            );
            return result;
        }
        Err(e) => {
            let result = ModelReviewResult::failed(None, None, e.to_string());
            save_review(
                root,
                id,
                None,
                None,
                Vec::new(),
                Vec::new(),
                result.error.clone(),
            );
            return result;
        }
    };

    run_with_task_and_provider(root, id, base, wt, &task_text, provider.as_ref())
}

pub fn run_with_provider(
    root: &Path,
    id: &str,
    base: &str,
    wt: &Path,
    provider: &dyn blyte_provider::Provider,
) -> ModelReviewResult {
    let start_path = root.join(".wt").join(format!("{id}.start.json"));
    let task_text = match read_task_text(root, &start_path) {
        Some(t) => t,
        None => {
            let result =
                ModelReviewResult::failed(Some(provider.model_id()), None, "task file not found");
            save_review(
                root,
                id,
                result.model.as_deref(),
                None,
                Vec::new(),
                Vec::new(),
                result.error.clone(),
            );
            return result;
        }
    };
    run_with_task_and_provider(root, id, base, wt, &task_text, provider)
}

fn is_max_tokens(finish_reason: Option<&str>) -> bool {
    finish_reason.is_some_and(|r| r.eq_ignore_ascii_case("MAX_TOKENS"))
}

fn sum_costs(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn run_with_task_and_provider(
    root: &Path,
    id: &str,
    base: &str,
    wt: &Path,
    task_text: &str,
    provider: &dyn blyte_provider::Provider,
) -> ModelReviewResult {
    let model_id = provider.model_id();

    let stat_out = match Command::new("git")
        .args(["diff", "--stat", &format!("{base}..HEAD")])
        .current_dir(wt)
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) => {
            let err = format!(
                "git diff --stat failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            save_review(
                root,
                id,
                Some(&model_id),
                None,
                Vec::new(),
                Vec::new(),
                Some(err.clone()),
            );
            return ModelReviewResult::failed(Some(model_id), None, err);
        }
        Err(e) => {
            let err = format!("git diff --stat failed: {e}");
            save_review(
                root,
                id,
                Some(&model_id),
                None,
                Vec::new(),
                Vec::new(),
                Some(err.clone()),
            );
            return ModelReviewResult::failed(Some(model_id), None, err);
        }
    };

    let diff_out = match Command::new("git")
        .args(["diff", &format!("{base}..HEAD")])
        .current_dir(wt)
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => {
            let err = format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            save_review(
                root,
                id,
                Some(&model_id),
                None,
                Vec::new(),
                Vec::new(),
                Some(err.clone()),
            );
            return ModelReviewResult::failed(Some(model_id), None, err);
        }
        Err(e) => {
            let err = format!("git diff failed: {e}");
            save_review(
                root,
                id,
                Some(&model_id),
                None,
                Vec::new(),
                Vec::new(),
                Some(err.clone()),
            );
            return ModelReviewResult::failed(Some(model_id), None, err);
        }
    };

    let candidates = crate::fallback_check::added_fallbacks(&diff_out);
    let expect_fallbacks = !candidates.is_empty();
    let req = build_request(task_text, &stat_out, &diff_out, &candidates);
    let resp = match provider.complete(&req) {
        Ok(r) => r,
        Err(e) => {
            let err = e.to_string();
            save_review(
                root,
                id,
                Some(&model_id),
                None,
                Vec::new(),
                Vec::new(),
                Some(err.clone()),
            );
            return ModelReviewResult::failed(Some(model_id), None, err);
        }
    };

    let cost1 = blyte_provider::pricing(&model_id).map(|p| p.cost(&resp.usage));

    let (parsed, total_cost) = match parse(&resp.text, expect_fallbacks) {
        Ok(p) => (p, cost1),
        Err(_) if is_max_tokens(resp.finish_reason.as_deref()) => {
            let mut retry_req = req.clone();
            retry_req.max_output_tokens = retry_req.max_output_tokens.map(|n| n.saturating_mul(2));
            let resp2 = match provider.complete(&retry_req) {
                Ok(r) => r,
                Err(e2) => {
                    let err_msg = format!("{e2} (after one retry)");
                    save_review(
                        root,
                        id,
                        Some(&model_id),
                        cost1,
                        Vec::new(),
                        Vec::new(),
                        Some(err_msg.clone()),
                    );
                    return ModelReviewResult::failed(Some(model_id), cost1, err_msg);
                }
            };
            let cost2 = blyte_provider::pricing(&model_id).map(|p| p.cost(&resp2.usage));
            let total_cost = sum_costs(cost1, cost2);
            match parse(&resp2.text, expect_fallbacks) {
                Ok(p) => (p, total_cost),
                Err(e2) => {
                    let err_msg = format!(
                        "{} (after one retry)",
                        parse_error_message(&e2, resp2.finish_reason.as_deref())
                    );
                    save_review(
                        root,
                        id,
                        Some(&model_id),
                        total_cost,
                        Vec::new(),
                        Vec::new(),
                        Some(err_msg.clone()),
                    );
                    return ModelReviewResult::failed(Some(model_id), total_cost, err_msg);
                }
            }
        }
        Err(e) => {
            let err_msg = parse_error_message(&e, resp.finish_reason.as_deref());
            save_review(
                root,
                id,
                Some(&model_id),
                cost1,
                Vec::new(),
                Vec::new(),
                Some(err_msg.clone()),
            );
            return ModelReviewResult::failed(Some(model_id), cost1, err_msg);
        }
    };

    let mut concerns = parsed.concerns;
    sort_concerns(&mut concerns);
    let fallbacks = parsed.fallbacks;

    save_review(
        root,
        id,
        Some(&model_id),
        total_cost,
        concerns.clone(),
        fallbacks.clone(),
        None,
    );
    ModelReviewResult::ok(model_id, total_cost, concerns).with_fallbacks(fallbacks)
}

pub fn print_result(result: &ModelReviewResult) {
    if result.is_failed() {
        let err = result.error.as_deref().unwrap_or("unknown error");
        println!("== model review");
        if err == "task file not found" || err.starts_with("no API key (set ") {
            println!("skipped: {err}");
        } else {
            println!("error: {err}");
        }
        return;
    }

    let model_id = result.model.as_deref().unwrap_or("?");
    let cost_str = match result.cost_usd {
        Some(c) => format!("${c:.4}"),
        None => "?".to_string(),
    };
    println!("== model review (trial: {model_id}, {cost_str})");
    if result.concerns.is_empty() {
        println!("nothing raised");
    } else {
        for c in &result.concerns {
            println!("{}", format_concern(c));
        }
    }
    if !result.fallbacks.is_empty() {
        let confirmed: Vec<&FallbackVerdict> = result
            .fallbacks
            .iter()
            .filter(|f| f.verdict == "fallback")
            .collect();
        let dismissed = result
            .fallbacks
            .iter()
            .filter(|f| f.verdict == "not_fallback")
            .count();
        if !confirmed.is_empty() {
            println!("confirmed fallbacks ({}):", confirmed.len());
            for f in confirmed {
                println!("  {}:{} {}", f.file, f.line, f.reason);
            }
        }
        println!("fallback candidates dismissed: {dismissed}");
    }
}

pub fn load_saved_review(path: &Path) -> Result<SavedModelReview, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

fn read_task_text(root: &Path, start_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(start_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let task_file_str = val.get("task_file")?.as_str()?;
    let task_path = Path::new(task_file_str);
    if task_path.is_absolute() {
        std::fs::read_to_string(task_path).ok()
    } else {
        std::fs::read_to_string(root.join(task_path))
            .or_else(|_| std::fs::read_to_string(task_path))
            .ok()
    }
}

fn save_review(
    root: &Path,
    id: &str,
    model: Option<&str>,
    cost_usd: Option<f64>,
    concerns: Vec<Concern>,
    fallbacks: Vec<FallbackVerdict>,
    error: Option<String>,
) {
    let wt_dir = root.join(".wt");
    let _ = std::fs::create_dir_all(&wt_dir);
    let review_path = wt_dir.join(format!("{id}.model-review.json"));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = SavedModelReview {
        model: model.map(str::to_string),
        cost_usd,
        concerns,
        fallbacks,
        ts,
        error,
    };
    if let Ok(json_str) = serde_json::to_string_pretty(&record) {
        let _ = std::fs::write(&review_path, json_str + "\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback_check::Finding;

    #[test]
    fn test_parse_empty_object() {
        let res =
            parse(r#"{"concerns": [], "fallbacks": []}"#, false).expect("parse should succeed");
        assert!(res.concerns.is_empty());
        assert!(res.fallbacks.is_empty());
    }

    #[test]
    fn test_parse_bare_array_is_error() {
        assert!(parse("[]", false).is_err());
        assert!(parse(
            r#"[{"file":"src/a.rs","line":1,"severity":"high","text":"bad"}]"#,
            false
        )
        .is_err());
    }

    #[test]
    fn test_parse_missing_fallbacks_when_expected_is_error() {
        let err = parse(r#"{"concerns": []}"#, true).expect_err("expected error");
        assert!(err.contains("fallbacks"), "{err}");

        let ok = parse(r#"{"concerns": []}"#, false).expect("allowed when no candidates sent");
        assert!(ok.concerns.is_empty());
        assert!(ok.fallbacks.is_empty());
    }

    #[test]
    fn test_parse_unknown_verdict_is_error() {
        let input = r#"{
            "concerns": [],
            "fallbacks": [
                {"file": "src/a.rs", "line": 4, "verdict": "maybe", "reason": "unclear"}
            ]
        }"#;
        let err = parse(input, true).expect_err("expected error for unknown verdict");
        assert!(err.contains("maybe"), "{err}");
    }

    #[test]
    fn test_parse_both_arrays() {
        let input = r#"{
            "concerns": [
                {"file": "src/a.rs", "line": 10, "severity": "high", "text": "missing check"}
            ],
            "fallbacks": [
                {"file": "src/a.rs", "line": 4, "verdict": "fallback", "reason": "hides missing env var"},
                {"file": "src/b.rs", "line": 9, "verdict": "not_fallback", "reason": "documented default port"}
            ]
        }"#;
        let parsed = parse(input, true).expect("parse should succeed");
        assert_eq!(parsed.concerns.len(), 1);
        assert_eq!(parsed.fallbacks.len(), 2);
        assert_eq!(parsed.fallbacks[0].verdict, "fallback");
        assert_eq!(parsed.fallbacks[0].line, 4);
        assert_eq!(parsed.fallbacks[1].verdict, "not_fallback");
        assert_eq!(parsed.fallbacks[1].reason, "documented default port");
    }

    #[test]
    fn test_parse_empty_or_whitespace() {
        assert_eq!(parse("", false), Err("empty response".to_string()));
        assert_eq!(parse("  \n", false), Err("empty response".to_string()));
    }

    #[test]
    fn test_parse_error_message() {
        assert_eq!(
            parse_error_message("empty response", Some("MAX_TOKENS")),
            "empty response (finish reason: MAX_TOKENS)"
        );
        assert_eq!(
            parse_error_message("empty response", None),
            "empty response"
        );
    }

    #[test]
    fn test_parse_fenced_with_prose() {
        let input = r#"
Here is the review of the diff against the task:
```json
{
  "concerns": [
    {
      "file": "crates/blyte-cli/src/foo.rs",
      "line": 42,
      "severity": "high",
      "text": "Task requires error handling here"
    },
    {
      "file": "crates/blyte-cli/src/bar.rs",
      "line": 100,
      "severity": "medium",
      "text": "Missing edge case check"
    }
  ],
  "fallbacks": []
}
```
Overall looks okay otherwise.
"#;
        let parsed = parse(input, false).expect("parse should succeed");
        let concerns = parsed.concerns;
        assert_eq!(concerns.len(), 2);
        assert_eq!(concerns[0].file, "crates/blyte-cli/src/foo.rs");
        assert_eq!(concerns[0].line, Some(42));
        assert_eq!(concerns[0].severity, "high");
        assert_eq!(concerns[0].text, "Task requires error handling here");
        assert_eq!(concerns[1].file, "crates/blyte-cli/src/bar.rs");
        assert_eq!(concerns[1].line, Some(100));
        assert_eq!(concerns[1].severity, "medium");
        assert_eq!(concerns[1].text, "Missing edge case check");
    }

    #[test]
    fn test_parse_normalization_and_filtering() {
        let input = r#"{
            "concerns": [
                {
                    "file": "crates/a.rs",
                    "line": null,
                    "severity": "critical",
                    "text": "A severe issue"
                },
                {
                    "file": "crates/b.rs",
                    "line": 10,
                    "severity": "high"
                }
            ],
            "fallbacks": []
        }"#;
        let parsed = parse(input, false).expect("parse should succeed");
        let concerns = parsed.concerns;
        assert_eq!(concerns.len(), 1);
        assert_eq!(concerns[0].file, "crates/a.rs");
        assert_eq!(concerns[0].line, None);
        assert_eq!(concerns[0].severity, "low");
        assert_eq!(concerns[0].text, "A severe issue");
    }

    #[test]
    fn test_parse_not_json() {
        let err = parse("not json", false).expect_err("should return Err");
        assert!(err.contains("not json"));
    }

    #[test]
    fn test_parse_long_unparseable_truncated_to_200() {
        let input = "not json ".repeat(30);
        assert!(input.chars().count() > 200);
        let err = parse(&input, false).expect_err("should return Err");
        assert_eq!(err.chars().count(), 200);
        assert!(input.starts_with(&err));
    }

    #[test]
    fn test_build_request_diff_truncation() {
        let task = "Implement feature X with full test coverage.";
        let stat = " src/main.rs | 100 ++\n 1 file changed";
        let diff = "a".repeat(300_000);

        let req = build_request(task, stat, &diff, &[]);
        assert_eq!(req.max_output_tokens, Some(32_000));
        assert!(req.tools.is_empty(), "tools should be empty");
        assert_eq!(req.messages.len(), 1);

        let blyte_provider::Message::User(ref msg) = req.messages[0] else {
            panic!("expected user message");
        };

        assert!(
            msg.contains("[diff truncated]"),
            "must contain [diff truncated]"
        );
        assert!(msg.len() < 210_000, "user message length was {}", msg.len());

        let task_pos = msg.find(task).expect("task text missing");
        let diff_pos = msg.find("## Diff").expect("diff header missing");
        assert!(task_pos < diff_pos, "task text must appear before diff");
        assert!(!msg.contains("## Fallback candidates"), "{msg}");
    }

    #[test]
    fn test_build_request_lists_candidates_only_when_present() {
        let req_empty = build_request("task", "stat", "diff", &[]);
        let blyte_provider::Message::User(ref empty_msg) = req_empty.messages[0] else {
            panic!("expected user message");
        };
        assert!(!empty_msg.contains("## Fallback candidates"));

        let candidates = vec![
            Finding {
                file: "src/cfg.rs".to_string(),
                line: 14,
                pattern: "unwrap_or",
                text: "let port = env_port.unwrap_or(8080);".to_string(),
            },
            Finding {
                file: "src/io.rs".to_string(),
                line: 42,
                pattern: "unwrap_or_default",
                text: "let body = read().unwrap_or_default();".to_string(),
            },
        ];
        let req = build_request("task", "stat", "diff", &candidates);
        let blyte_provider::Message::User(ref msg) = req.messages[0] else {
            panic!("expected user message");
        };
        assert!(msg.contains("## Fallback candidates"), "{msg}");
        assert!(
            msg.contains("src/cfg.rs:14 [unwrap_or] let port = env_port.unwrap_or(8080);"),
            "{msg}"
        );
        assert!(
            msg.contains("src/io.rs:42 [unwrap_or_default] let body = read().unwrap_or_default();"),
            "{msg}"
        );
        assert!(
            msg.contains("a documented, intended default is not a fallback"),
            "{msg}"
        );
    }

    #[test]
    fn test_build_request_max_output_tokens() {
        let req = build_request("task", "stat", "diff", &[]);
        assert_eq!(req.max_output_tokens, Some(32_000));
    }

    #[test]
    fn test_concerns_sort_and_format() {
        let mut concerns = vec![
            Concern {
                file: "low.rs".to_string(),
                line: Some(1),
                severity: "low".to_string(),
                text: "low issue".to_string(),
            },
            Concern {
                file: "high.rs".to_string(),
                line: None,
                severity: "high".to_string(),
                text: "high issue".to_string(),
            },
            Concern {
                file: "med.rs".to_string(),
                line: Some(2),
                severity: "medium".to_string(),
                text: "medium issue".to_string(),
            },
        ];

        sort_concerns(&mut concerns);

        assert_eq!(concerns[0].severity, "high");
        assert_eq!(concerns[1].severity, "medium");
        assert_eq!(concerns[2].severity, "low");

        assert_eq!(format_concern(&concerns[0]), "[high] high.rs high issue");
        assert_eq!(
            format_concern(&concerns[1]),
            "[medium] med.rs:2 medium issue"
        );
        assert_eq!(format_concern(&concerns[2]), "[low] low.rs:1 low issue");
    }

    #[test]
    fn test_save_review_record_and_load_old_format() {
        let dir = tempfile::tempdir().unwrap();
        let concerns = vec![Concern {
            file: "foo.rs".to_string(),
            line: Some(5),
            severity: "high".to_string(),
            text: "problem".to_string(),
        }];
        let fallbacks = vec![FallbackVerdict {
            file: "foo.rs".to_string(),
            line: 7,
            verdict: "not_fallback".to_string(),
            reason: "documented default".to_string(),
        }];
        save_review(
            dir.path(),
            "T1",
            Some("gemini/gemini-3.8-flash"),
            Some(0.0012),
            concerns,
            fallbacks,
            None,
        );
        let path = dir.path().join(".wt/T1.model-review.json");
        assert!(path.is_file());
        let content = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(val["model"], "gemini/gemini-3.8-flash");
        assert_eq!(val["cost_usd"], 0.0012);
        assert_eq!(val["concerns"].as_array().unwrap().len(), 1);
        assert_eq!(val["fallbacks"].as_array().unwrap().len(), 1);
        assert_eq!(val["fallbacks"][0]["verdict"], "not_fallback");
        assert!(val["error"].is_null());
        assert!(val["ts"].as_u64().unwrap() > 0);

        let loaded = load_saved_review(&path).unwrap();
        assert_eq!(loaded.fallbacks.len(), 1);

        let old_path = dir.path().join(".wt/OLD.model-review.json");
        std::fs::write(
            &old_path,
            r#"{
  "model": "gemini/gemini-3.8-flash",
  "cost_usd": 0.001,
  "concerns": [],
  "ts": 1700000000,
  "error": null
}"#,
        )
        .unwrap();
        let old_loaded = load_saved_review(&old_path).unwrap();
        assert_eq!(old_loaded.model.as_deref(), Some("gemini/gemini-3.8-flash"));
        assert!(old_loaded.fallbacks.is_empty());
    }
}
