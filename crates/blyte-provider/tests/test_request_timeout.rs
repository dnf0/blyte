use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use blyte_provider::{
    anthropic::Anthropic, chatgpt::ChatGpt, gemini::Gemini, ollama::Ollama, openai::OpenAi,
    Message, Provider, ProviderError, Request,
};
use serde_json::json;

fn read_http_request(stream: &mut std::net::TcpStream) {
    let mut buf = [0u8; 1024];
    let mut total = Vec::new();
    let mut header_end = None;
    let mut content_len = 0usize;
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        total.extend_from_slice(&buf[..n]);
        if header_end.is_none() {
            if let Some(pos) = total.windows(4).position(|w| w == b"\r\n\r\n") {
                let end = pos + 4;
                header_end = Some(end);
                let head = String::from_utf8_lossy(&total[..end]);
                for line in head.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
            }
        }
        if let Some(end) = header_end {
            if total.len() >= end + content_len {
                break;
            }
        }
    }
}

/// Spawns a local TCP server that accepts one request, reads the request,
/// and sleeps for 30s without sending a reply.
fn spawn_stalling_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            std::thread::sleep(Duration::from_secs(30));
        }
    });
    format!("http://{addr}")
}

/// Spawns a local TCP server that accepts one request and immediately writes `http_response`.
fn spawn_quick_server(content_type: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let mut drain = [0u8; 64];
            let _ = stream.read(&mut drain);
        }
    });
    format!("http://{addr}")
}

fn req_with_timeout(secs: u64) -> Request {
    Request {
        system: String::new(),
        messages: vec![Message::User("hi".into())],
        tools: Vec::new(),
        max_output_tokens: None,
        timeout: Some(Duration::from_secs(secs)),
    }
}

fn assert_timeout_err(err: ProviderError, elapsed: Duration) {
    assert!(
        elapsed < Duration::from_secs(10),
        "expected call to fail within 10s (well before 30s), took {elapsed:?}"
    );
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("time limit") && (msg.contains("1s") || msg.contains("1 s")),
        "expected message to mention the request's time limit of 1s, got: {msg}"
    );
}

#[test]
fn gemini_request_timeout_and_quick_reply() {
    let stall_url = spawn_stalling_server();
    let p_stall = Gemini::new_api_key("test-key").with_api_root(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(1)).unwrap_err();
    assert_timeout_err(err, start.elapsed());

    let sse_body = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hello from gemini\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":3}}\n\n";
    let quick_url = spawn_quick_server("text/event-stream", sse_body);
    let p_quick = Gemini::new_api_key("test-key").with_api_root(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(10))
        .expect("quick reply with 10s timeout should succeed");
    assert_eq!(resp.text, "hello from gemini");
}

#[test]
fn gemini_stream_cap_honors_request_timeout() {
    // Server immediately sends HTTP 200 and one SSE chunk, then trickles comments every 200ms
    // so idle read timeout is never hit, testing the whole-stream cap in parse_stream.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
            for _ in 0..150 {
                std::thread::sleep(Duration::from_millis(200));
                if stream.write_all(b": keepalive\n\n").is_err() {
                    break;
                }
                let _ = stream.flush();
            }
        }
    });
    let p = Gemini::new_api_key("test-key").with_api_root(format!("http://{addr}"));
    let start = Instant::now();
    let err = p.complete(&req_with_timeout(1)).unwrap_err();
    assert_timeout_err(err, start.elapsed());
}

#[test]
fn anthropic_request_timeout_and_quick_reply() {
    let stall_url = spawn_stalling_server();
    let p_stall = Anthropic::new("test-key").with_api_root(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(1)).unwrap_err();
    assert_timeout_err(err, start.elapsed());

    let json_body = r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"hello from anthropic"}],"stop_reason":"end_turn","usage":{"input_tokens":4,"output_tokens":3}}"#;
    let quick_url = spawn_quick_server("application/json", json_body);
    let p_quick = Anthropic::new("test-key").with_api_root(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(10))
        .expect("quick reply with 10s timeout should succeed");
    assert_eq!(resp.text, "hello from anthropic");
}

#[test]
fn openai_request_timeout_and_quick_reply() {
    let stall_url = spawn_stalling_server();
    let p_stall = OpenAi::new("test-key").with_base_url(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(1)).unwrap_err();
    assert_timeout_err(err, start.elapsed());

    let json_body = r#"{"choices":[{"message":{"role":"assistant","content":"hello from openai"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":3}}"#;
    let quick_url = spawn_quick_server("application/json", json_body);
    let p_quick = OpenAi::new("test-key").with_base_url(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(10))
        .expect("quick reply with 10s timeout should succeed");
    assert_eq!(resp.text, "hello from openai");
}

#[test]
fn ollama_request_timeout_and_quick_reply() {
    let stall_url = spawn_stalling_server();
    let p_stall = Ollama::from_env_or_default()
        .with_model("qwen2.5-coder:32b")
        .with_api_root(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(1)).unwrap_err();
    assert_timeout_err(err, start.elapsed());

    let json_body = r#"{"model":"qwen2.5-coder:32b","message":{"role":"assistant","content":"hello from ollama"},"done":true,"done_reason":"stop","prompt_eval_count":4,"eval_count":3}"#;
    let quick_url = spawn_quick_server("application/json", json_body);
    let p_quick = Ollama::from_env_or_default()
        .with_model("qwen2.5-coder:32b")
        .with_api_root(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(10))
        .expect("quick reply with 10s timeout should succeed");
    assert_eq!(resp.text, "hello from ollama");
}

fn b64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(T[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(T[(n & 63) as usize] as char);
        }
    }
    out
}

fn chatgpt_for_test(base_url: String) -> (tempfile::TempDir, ChatGpt) {
    let now: u64 = 1_700_000_000;
    let jwt = format!(
        "{}.{}.sig",
        b64url(b"{\"alg\":\"none\"}"),
        b64url(
            json!({
                "exp": now + 3600,
                "https://api.openai.com/auth": {"chatgpt_account_id": "acc"}
            })
            .to_string()
            .as_bytes()
        )
    );
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("auth.json"),
        json!({"tokens": {"access_token": jwt, "account_id": "acc"}}).to_string(),
    )
    .unwrap();
    let p = ChatGpt::from_codex_home(d.path(), now)
        .unwrap()
        .with_base_url(base_url);
    (d, p)
}

#[test]
fn chatgpt_request_timeout_and_quick_reply() {
    let stall_url = spawn_stalling_server();
    let (_d1, p_stall) = chatgpt_for_test(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(1)).unwrap_err();
    assert_timeout_err(err, start.elapsed());

    let sse_body = concat!(
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello from chatgpt\"}]}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":3}}}\n\n",
    );
    let quick_url = spawn_quick_server("text/event-stream", sse_body);
    let (_d2, p_quick) = chatgpt_for_test(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(10))
        .expect("quick reply with 10s timeout should succeed");
    assert_eq!(resp.text, "hello from chatgpt");
}
