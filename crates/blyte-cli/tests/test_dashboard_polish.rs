//! Polish from the dashboard's first live run: the task section of the plan, headline-free
//! summaries, and polls that leave a selection alone (2026-09-30).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{collect, Sources};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn job<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["key"] == key)
        .unwrap_or_else(|| panic!("no job {key} in {v}"))
}

#[test]
fn the_task_is_its_section_of_the_plan_and_now_skips_headings() {
    let tmp = tempfile::tempdir().unwrap();
    let plan = tmp.path().join("plan.md");
    write(
        &plan,
        "# The plan\n\n### Task 6: other\n\nsix\n\n### Task 7: the thing\n\nDo the thing.\n\n```bash\n# run it\n```\n\n#### Step 1\n\nstep\n\n### Task 8: next\n\neight\n",
    );
    let runs = tmp.path().join("runs/repo-e");
    let prompt = format!(
        "You are implementing exactly ONE task.\n\nPlan: {}\nTask: \"Task 7: the thing\" ONLY. Read the plan first.\n",
        plan.display()
    );
    write(&runs.join("P1.prompt"), &prompt);
    write(&runs.join("P1.log"), "[Turn 3] \n");
    write(
        &runs.join("P1.report.json"),
        r####"{"outcome":"verified","final_message":"### Summary of Changes\n\nAdded the thing.\nMore."}"####,
    );
    // A second job on the same plan, whose title isn't in it, keeps its prompt.
    write(
        &runs.join("P2.prompt"),
        &format!(
            "Plan: {}\nTask: \"Task 9: missing\" ONLY.\n",
            plan.display()
        ),
    );
    write(&runs.join("P2.log"), "[Turn 1] \n");
    write(
        &runs.join("P2.report.json"),
        r####"{"outcome":"verified","final_message":"## Done\n\n"}"####,
    );

    let src = Sources {
        roots: vec![],
        runs_dirs: vec![tmp.path().join("runs")],
        hours: 24,
    };
    let v = collect(&src, now());

    let p1 = job(&v, "repo-e/P1");
    let task = p1["task"].as_str().unwrap();
    assert!(task.starts_with("### Task 7: the thing"), "{task}");
    assert!(
        task.contains("Do the thing.") && task.contains("#### Step 1"),
        "{task}"
    );
    assert!(!task.contains("Task 8") && !task.contains("ONLY"), "{task}");
    assert_eq!(p1["now"], "Added the thing.");

    let p2 = job(&v, "repo-e/P2");
    assert!(p2["task"].as_str().unwrap().contains("Task 9: missing"));
    assert_eq!(p2["now"], "Done");
}

#[test]
fn the_page_skips_rerendering_the_list_while_text_is_selected() {
    let page = include_str!("../assets/dashboard/index.html");
    assert!(page.contains("getSelection"));
}
