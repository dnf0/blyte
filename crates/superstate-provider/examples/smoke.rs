//! One live call: does the model come back with a tool call in the shape we parse?
use serde_json::json;
use superstate_provider::{gemini::Gemini, Message, Provider, Request, ToolSpec};

fn main() {
    let g = match Gemini::from_env() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("model: {}", g.model_id());
    let req = Request {
        system: "You are a coding agent. Use tools rather than guessing.".into(),
        messages: vec![Message::User(
            "Read the file src/main.rs and tell me what it does.".into(),
        )],
        tools: vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a file from the repository".into(),
            parameters: json!({"type":"object",
                "properties":{"path":{"type":"string","description":"path to read"}},
                "required":["path"]}),
        }],
        max_output_tokens: Some(512),
    };
    match g.complete(&req) {
        Ok(r) => {
            println!("text        : {:?}", r.text);
            println!("tool_calls  : {:?}", r.tool_calls);
            println!("finish      : {:?}", r.finish_reason);
            println!("usage       : {:?}", r.usage);
        }
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}
