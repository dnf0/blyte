//! Model-assisted check of prose claims in a research report against their cited source lines.

use blyte_provider::{Message, Provider, ProviderError, Request, Usage};

use crate::research_check::{Claim, Evidence};

/// Maximum number of claims checked per report; any beyond this are counted as `unchecked`.
pub const MAX_CLAIMS: usize = 64;

/// Number of claims sent per checker model call.
pub const BATCH_SIZE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VerdictItem {
    pub id: usize,
    pub verdict: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaimFlag {
    pub line: usize,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaimCheck {
    pub checked: usize,
    pub unchecked: usize,
    pub contradicted: Vec<ClaimFlag>,
    pub not_enough_evidence: usize,
    pub parse_errors: usize,
    pub calls: u32,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

pub fn format_evidence(evidence: &[Evidence]) -> String {
    let mut out = String::new();
    for ev in evidence {
        out.push_str(&format!("--- {}:{}-{} ---\n", ev.path, ev.start, ev.end));
        out.push_str(&ev.lines);
        out.push('\n');
    }
    out
}

pub fn build_check_request(batch: &[(usize, &Claim)]) -> Request {
    let mut prompt = String::new();
    prompt.push_str(
        "For each claim below, using ONLY the evidence shown for that claim, determine whether \
         the claim is `supported`, `contradicted`, or `not_enough_evidence`, with a one-sentence \
         reason quoting the line number it relies on.\n\
         Reply with ONLY a JSON array of the form:\n\
         [{\"id\": 1, \"verdict\": \"supported\" | \"contradicted\" | \"not_enough_evidence\", \"reason\": \"...\"}]\n\n",
    );
    for &(id, c) in batch {
        prompt.push_str(&format!("### Claim {id}\n{}\n", c.text));
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
        timeout: None,
    }
}

pub fn extract_json_array(text: &str) -> Option<&str> {
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
pub fn parse_verdict_reply(text: &str) -> Result<Vec<VerdictItem>, String> {
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

/// Check up to [`MAX_CLAIMS`] claims in batches of [`BATCH_SIZE`] using `provider`.
pub fn check_claims(
    provider: &dyn Provider,
    claims: &[Claim],
    deadline: Option<std::time::Instant>,
) -> ClaimCheck {
    let capped = claims.len().min(MAX_CLAIMS);
    let to_check = &claims[..capped];
    let batches: Vec<&[Claim]> = to_check.chunks(BATCH_SIZE).collect();
    let total_batches = batches.len();

    let mut out = ClaimCheck {
        checked: 0,
        unchecked: claims.len(),
        ..Default::default()
    };

    for (batch_idx, batch) in batches.iter().enumerate() {
        let pairs: Vec<(usize, &Claim)> =
            batch.iter().enumerate().map(|(i, c)| (i + 1, c)).collect();
        let mut req = build_check_request(&pairs);

        let mut parsed: Option<Vec<VerdictItem>> = None;
        let mut out_of_time = false;
        for _attempt in 0..2 {
            if let Some(d) = deadline {
                let time_left = d.saturating_duration_since(std::time::Instant::now());
                if time_left.is_zero() {
                    out_of_time = true;
                    break;
                }
                req.timeout = Some(time_left);
            }
            out.calls += 1;
            match provider.complete(&req) {
                Ok(resp) => {
                    add_usage(&mut out.usage, &resp.usage);
                    if let Ok(items) = parse_verdict_reply(&resp.text) {
                        parsed = Some(items);
                        break;
                    }
                }
                Err(e) => {
                    if let ProviderError::NoAnswer { usage, .. } = &e {
                        add_usage(&mut out.usage, usage);
                    }
                    if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                        out_of_time = true;
                        break;
                    }
                }
            }
        }

        if out_of_time && parsed.is_none() {
            out.skipped = Some(format!(
                "out of time after {batch_idx} of {total_batches} batches"
            ));
            break;
        }

        out.checked += batch.len();
        out.unchecked = claims.len().saturating_sub(out.checked);

        match parsed {
            Some(items) => {
                for (idx, c) in batch.iter().enumerate() {
                    let id = idx + 1;
                    match items.iter().find(|it| it.id == id) {
                        Some(found) => match found.verdict.as_str() {
                            "contradicted" => out.contradicted.push(ClaimFlag {
                                line: c.line,
                                text: c.text.clone(),
                                reason: found.reason.clone(),
                            }),
                            "not_enough_evidence" => out.not_enough_evidence += 1,
                            _ => {}
                        },
                        None => out.parse_errors += 1,
                    }
                }
            }
            None => {
                out.parse_errors += batch.len();
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use blyte_provider::Response;

    use super::*;

    struct ScriptedProvider {
        replies: Mutex<VecDeque<Result<Response, ProviderError>>>,
        batch_sizes: Mutex<Vec<usize>>,
    }

    impl ScriptedProvider {
        fn new(replies: Vec<Result<Response, ProviderError>>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                batch_sizes: Mutex::new(Vec::new()),
            }
        }
    }

    impl Provider for ScriptedProvider {
        fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
            let prompt = match req.messages.first() {
                Some(Message::User(s)) => s.as_str(),
                _ => "",
            };
            let count = prompt.matches("### Claim ").count();
            self.batch_sizes.lock().unwrap().push(count);

            if let Some(r) = self.replies.lock().unwrap().pop_front() {
                return r;
            }
            let items: Vec<serde_json::Value> = (1..=count)
                .map(|id| {
                    serde_json::json!({
                        "id": id,
                        "verdict": "supported",
                        "reason": "matches line 1"
                    })
                })
                .collect();
            Ok(Response {
                text: serde_json::to_string(&items).unwrap(),
                usage: Usage {
                    input: 10,
                    cached: 0,
                    output: 5,
                    thinking: 0,
                },
                ..Default::default()
            })
        }

        fn model_id(&self) -> String {
            "scripted".into()
        }
    }

    fn make_claims(n: usize) -> Vec<Claim> {
        (1..=n)
            .map(|i| Claim {
                id: i,
                line: i * 10,
                text: format!("claim {i}"),
                quoted: None,
                evidence: vec![Evidence {
                    path: "src/lib.rs".into(),
                    start: 1,
                    end: 3,
                    lines: "1\tfn a() {}".into(),
                }],
            })
            .collect()
    }

    #[test]
    fn seventeen_claims_batched_into_8_8_1() {
        let provider = ScriptedProvider::new(Vec::new());
        let claims = make_claims(17);
        let res = check_claims(&provider, &claims, None);
        assert_eq!(res.checked, 17);
        assert_eq!(res.unchecked, 0);
        assert_eq!(res.calls, 3);
        assert_eq!(res.parse_errors, 0);
        assert_eq!(*provider.batch_sizes.lock().unwrap(), vec![8, 8, 1]);
    }

    #[test]
    fn seventy_claims_caps_at_64_checked_and_6_unchecked() {
        let provider = ScriptedProvider::new(Vec::new());
        let claims = make_claims(70);
        let res = check_claims(&provider, &claims, None);
        assert_eq!(res.checked, 64);
        assert_eq!(res.unchecked, 6);
        assert_eq!(res.calls, 8);
        assert_eq!(res.parse_errors, 0);
    }

    #[test]
    fn unparseable_reply_then_valid_retries_once_without_parse_error() {
        let provider = ScriptedProvider::new(vec![
            Ok(Response {
                text: "not valid json".into(),
                usage: Usage {
                    input: 100,
                    cached: 20,
                    output: 10,
                    thinking: 5,
                },
                ..Default::default()
            }),
            Ok(Response {
                text: r#"[{"id": 1, "verdict": "supported", "reason": "line 1"}, {"id": 2, "verdict": "not_enough_evidence", "reason": "line 2"}]"#.into(),
                usage: Usage {
                    input: 100,
                    cached: 80,
                    output: 30,
                    thinking: 10,
                },
                ..Default::default()
            }),
        ]);
        let claims = make_claims(2);
        let res = check_claims(&provider, &claims, None);
        assert_eq!(res.calls, 2);
        assert_eq!(res.parse_errors, 0);
        assert_eq!(res.not_enough_evidence, 1);
        assert_eq!(
            res.usage,
            Usage {
                input: 200,
                cached: 100,
                output: 40,
                thinking: 15,
            }
        );
    }

    #[test]
    fn two_unparseable_replies_count_batch_claims_as_parse_errors() {
        let provider = ScriptedProvider::new(vec![
            Ok(Response {
                text: "garbage 1".into(),
                usage: Usage {
                    input: 50,
                    cached: 0,
                    output: 5,
                    thinking: 0,
                },
                ..Default::default()
            }),
            Err(ProviderError::NoAnswer {
                block_reason: None,
                block_reason_message: None,
                usage: Usage {
                    input: 50,
                    cached: 10,
                    output: 0,
                    thinking: 3,
                },
            }),
        ]);
        let claims = make_claims(3);
        let res = check_claims(&provider, &claims, None);
        assert_eq!(res.calls, 2);
        assert_eq!(res.parse_errors, 3);
        assert_eq!(
            res.usage,
            Usage {
                input: 100,
                cached: 10,
                output: 5,
                thinking: 3,
            }
        );
    }

    #[test]
    fn contradicted_verdict_produces_flag_with_line_and_reason() {
        let provider = ScriptedProvider::new(vec![Ok(Response {
            text: r#"[
                {"id": 1, "verdict": "supported", "reason": "line 1 matches"},
                {"id": 2, "verdict": "contradicted", "reason": "line 2 says 5, not 10"}
            ]"#
            .into(),
            ..Default::default()
        })]);
        let claims = make_claims(2);
        let res = check_claims(&provider, &claims, None);
        assert_eq!(res.calls, 1);
        assert_eq!(res.parse_errors, 0);
        assert_eq!(
            res.contradicted,
            vec![ClaimFlag {
                line: 20,
                text: "claim 2".into(),
                reason: "line 2 says 5, not 10".into(),
            }]
        );
    }

    #[test]
    fn stops_between_batches_when_deadline_passes() {
        use std::time::{Duration, Instant};

        struct SlowBatchProvider {
            calls: Mutex<u32>,
            timeouts: Mutex<Vec<Option<Duration>>>,
        }
        impl Provider for SlowBatchProvider {
            fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
                self.timeouts.lock().unwrap().push(req.timeout);
                let mut c = self.calls.lock().unwrap();
                *c += 1;
                if *c == 1 {
                    std::thread::sleep(Duration::from_millis(250));
                }
                let items: Vec<serde_json::Value> = (1..=BATCH_SIZE)
                    .map(|id| {
                        serde_json::json!({
                            "id": id,
                            "verdict": "supported",
                            "reason": "ok"
                        })
                    })
                    .collect();
                Ok(Response {
                    text: serde_json::to_string(&items).unwrap(),
                    ..Default::default()
                })
            }
            fn model_id(&self) -> String {
                "slow-batch".into()
            }
        }

        let provider = SlowBatchProvider {
            calls: Mutex::new(0),
            timeouts: Mutex::new(Vec::new()),
        };
        let claims = make_claims(17); // 3 batches (8, 8, 1)
        let deadline = Some(Instant::now() + Duration::from_millis(200));
        let res = check_claims(&provider, &claims, deadline);
        assert_eq!(res.calls, 1);
        assert_eq!(res.checked, 8);
        assert_eq!(res.unchecked, 9);
        assert_eq!(
            res.skipped.as_deref(),
            Some("out of time after 1 of 3 batches")
        );
        let seen = provider.timeouts.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].is_some_and(|t| t <= Duration::from_millis(200)));
    }
}
