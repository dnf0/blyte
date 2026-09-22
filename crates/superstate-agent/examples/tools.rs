fn main() {
    for t in superstate_agent::tools::specs() {
        println!("  {:<6} {}", t.name, t.description.lines().next().unwrap_or(""));
    }
    println!("\n  environment: {}", superstate_agent::tools::available_binaries().join(", "));
}
