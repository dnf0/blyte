//! What the model is offered, and where its commands land.
use std::path::Path;
use superstate_agent::shell::{LocalShell, Shell};

fn main() {
    for t in superstate_agent::tools::specs() {
        println!("  {:<6} {}", t.name, t.description.lines().next().unwrap_or(""));
    }
    let sh = LocalShell;
    println!("\n  execution : {}", sh.describe());
    println!("  reachable : {}",
        superstate_agent::tools::available_binaries(&sh, Path::new(".")).join(", "));
}
