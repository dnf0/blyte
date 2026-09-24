//! What the model is offered, and where its commands land.
use blyte_agent::shell::{LocalShell, Shell};
use std::path::Path;

fn main() {
    for t in blyte_agent::tools::specs() {
        println!(
            "  {:<6} {}",
            t.name,
            t.description.lines().next().unwrap_or("")
        );
    }
    let sh = LocalShell;
    println!("\n  execution : {}", sh.describe());
    println!(
        "  reachable : {}",
        blyte_agent::tools::available_binaries(&sh, Path::new(".")).join(", ")
    );
}
