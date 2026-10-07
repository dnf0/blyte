//! Combined orchestrator and implementer cost reporting.
//!
//! Aggregates costs from the machine-wide usage log across interactive REPL sessions
//! and dispatched worker runs.

use crate::usage_log::Entry;
use serde::{Deserialize, Serialize};

/// Aggregated spend and run count.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Totals {
    pub orchestrator_usd: f64,
    pub implementers_usd: f64,
    pub runs: usize,
}

impl Default for Totals {
    fn default() -> Self {
        Self {
            orchestrator_usd: 0.0,
            implementers_usd: 0.0,
            runs: 0,
        }
    }
}

/// Aggregates usage log entries matching all given filters.
///
/// Filters:
/// - `session`: entries with matching session ID
/// - `since`: entries with timestamp >= `since`
/// - `repo`: entries with matching repository path
///
/// Entries with `dispatch: Some(_)` count toward `implementers_usd` and `runs`.
/// Entries without dispatch count toward `orchestrator_usd`.
/// An entry with `cost_usd: None` adds 0 to cost totals.
pub fn totals(
    entries: &[Entry],
    session: Option<&str>,
    since: Option<u64>,
    repo: Option<&str>,
) -> Totals {
    let mut orchestrator_usd = 0.0;
    let mut implementers_usd = 0.0;
    let mut runs = 0;

    for entry in entries {
        if let Some(s) = session {
            if entry.session.as_deref() != Some(s) {
                continue;
            }
        }
        if let Some(since_ts) = since {
            if entry.ts < since_ts {
                continue;
            }
        }
        if let Some(r) = repo {
            if entry.repo != r {
                continue;
            }
        }

        let cost = entry.cost_usd.unwrap_or(0.0);
        if entry.dispatch.is_some() {
            implementers_usd += cost;
            runs += 1;
        } else {
            orchestrator_usd += cost;
        }
    }

    Totals {
        orchestrator_usd,
        implementers_usd,
        runs,
    }
}

/// Formats the totals into the human-readable summary line:
/// `blyte $<o+i> (orchestrator $<o>, implementers $<i> in <runs> runs)`
pub fn format_totals(totals: &Totals) -> String {
    let total = totals.orchestrator_usd + totals.implementers_usd;
    format!(
        "blyte ${:.2} (orchestrator ${:.2}, implementers ${:.2} in {} runs)",
        total, totals.orchestrator_usd, totals.implementers_usd, totals.runs
    )
}

impl std::fmt::Display for Totals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format_totals(self))
    }
}

/// Executes the `blyte cost` subcommand.
pub fn run(args: &crate::cli::CostArgs) -> i32 {
    let entries = crate::usage_log::default_path()
        .as_deref()
        .map(crate::usage_log::read)
        .unwrap_or_default();

    let repo_filter = args.repo.as_ref().map(|p| {
        if p.exists() {
            if let Ok(c) = std::fs::canonicalize(p) {
                return crate::usage_log::repo_of(&c);
            }
        }
        crate::usage_log::repo_of(p)
    });

    let t = totals(
        &entries,
        args.session.as_deref(),
        args.since,
        repo_filter.as_deref(),
    );

    if args.json {
        let json_str = serde_json::to_string_pretty(&t).unwrap();
        println!("{json_str}");
    } else {
        println!("{}", format_totals(&t));
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        ts: u64,
        repo: &str,
        session: Option<&str>,
        dispatch: Option<&str>,
        cost_usd: Option<f64>,
    ) -> Entry {
        Entry {
            ts,
            repo: repo.to_string(),
            session: session.map(String::from),
            dispatch: dispatch.map(String::from),
            research: false,
            model: "test-model".into(),
            outcome: "verified".into(),
            turns: 1,
            input: 100,
            cached: 0,
            output: 50,
            thinking: 0,
            cost_usd,
        }
    }

    #[test]
    fn test_totals_filters_and_aggregation() {
        // Entries mix:
        // Two sessions: "sess-1", "sess-2"
        // Dispatch and not: some Some, some None
        // One with cost_usd: None (e3)
        // Timestamps either side of since (since = 1000): e1 is 500, others >= 1000
        // Two repos: "/repo/alpha", "/repo/beta"
        let entries = vec![
            make_entry(500, "/repo/alpha", Some("sess-1"), None, Some(0.10)),
            make_entry(1500, "/repo/alpha", Some("sess-1"), Some("d1"), Some(0.20)),
            make_entry(1500, "/repo/beta", Some("sess-1"), Some("d2"), None),
            make_entry(2000, "/repo/beta", Some("sess-2"), None, Some(0.50)),
            make_entry(2500, "/repo/beta", Some("sess-2"), Some("d3"), Some(0.40)),
        ];

        // 1. No filter: all entries included
        let t_all = totals(&entries, None, None, None);
        assert!((t_all.orchestrator_usd - 0.60).abs() < 1e-6);
        assert!((t_all.implementers_usd - 0.60).abs() < 1e-6);
        assert_eq!(t_all.runs, 3);

        // 2. Filter session alone: "sess-1"
        let t_s1 = totals(&entries, Some("sess-1"), None, None);
        assert!((t_s1.orchestrator_usd - 0.10).abs() < 1e-6);
        assert!((t_s1.implementers_usd - 0.20).abs() < 1e-6);
        assert_eq!(t_s1.runs, 2);

        // Filter session alone: "sess-2"
        let t_s2 = totals(&entries, Some("sess-2"), None, None);
        assert!((t_s2.orchestrator_usd - 0.50).abs() < 1e-6);
        assert!((t_s2.implementers_usd - 0.40).abs() < 1e-6);
        assert_eq!(t_s2.runs, 1);

        // 3. Filter since alone: 1000 (e1 at 500 excluded)
        let t_since = totals(&entries, None, Some(1000), None);
        assert!((t_since.orchestrator_usd - 0.50).abs() < 1e-6);
        assert!((t_since.implementers_usd - 0.60).abs() < 1e-6);
        assert_eq!(t_since.runs, 3);

        // 4. Filter repo alone: "/repo/alpha" (e1, e2)
        let t_alpha = totals(&entries, None, None, Some("/repo/alpha"));
        assert!((t_alpha.orchestrator_usd - 0.10).abs() < 1e-6);
        assert!((t_alpha.implementers_usd - 0.20).abs() < 1e-6);
        assert_eq!(t_alpha.runs, 1);

        // Filter repo alone: "/repo/beta" (e3, e4, e5)
        let t_beta = totals(&entries, None, None, Some("/repo/beta"));
        assert!((t_beta.orchestrator_usd - 0.50).abs() < 1e-6);
        assert!((t_beta.implementers_usd - 0.40).abs() < 1e-6);
        assert_eq!(t_beta.runs, 2);

        // 5. Combined filters
        // session="sess-1", since=1000, repo="/repo/alpha" -> only e2
        let t_comb1 = totals(&entries, Some("sess-1"), Some(1000), Some("/repo/alpha"));
        assert!((t_comb1.orchestrator_usd - 0.0).abs() < 1e-6);
        assert!((t_comb1.implementers_usd - 0.20).abs() < 1e-6);
        assert_eq!(t_comb1.runs, 1);

        // session="sess-1", since=1000, repo="/repo/beta" -> only e3 (cost_usd: None)
        let t_comb2 = totals(&entries, Some("sess-1"), Some(1000), Some("/repo/beta"));
        assert!((t_comb2.orchestrator_usd - 0.0).abs() < 1e-6);
        assert!((t_comb2.implementers_usd - 0.0).abs() < 1e-6);
        assert_eq!(t_comb2.runs, 1);

        // session="sess-2", since=1000, repo="/repo/beta" -> e4 and e5
        let t_comb3 = totals(&entries, Some("sess-2"), Some(1000), Some("/repo/beta"));
        assert!((t_comb3.orchestrator_usd - 0.50).abs() < 1e-6);
        assert!((t_comb3.implementers_usd - 0.40).abs() < 1e-6);
        assert_eq!(t_comb3.runs, 1);

        // Non-matching session
        let t_none = totals(&entries, Some("sess-nonexistent"), None, None);
        assert_eq!(
            t_none,
            Totals {
                orchestrator_usd: 0.0,
                implementers_usd: 0.0,
                runs: 0,
            }
        );
    }

    #[test]
    fn test_format_totals() {
        let known = Totals {
            orchestrator_usd: 1.25,
            implementers_usd: 2.50,
            runs: 3,
        };
        assert_eq!(
            format_totals(&known),
            "blyte $3.75 (orchestrator $1.25, implementers $2.50 in 3 runs)"
        );
        assert_eq!(
            known.to_string(),
            "blyte $3.75 (orchestrator $1.25, implementers $2.50 in 3 runs)"
        );

        let zero = Totals::default();
        assert_eq!(
            format_totals(&zero),
            "blyte $0.00 (orchestrator $0.00, implementers $0.00 in 0 runs)"
        );
    }

    #[test]
    fn test_totals_json() {
        let t = Totals {
            orchestrator_usd: 1.25,
            implementers_usd: 2.50,
            runs: 3,
        };
        let json = serde_json::to_string(&t).unwrap();
        let parsed: Totals = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, t);
    }
}
