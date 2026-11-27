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
/// and sleeps for 60s without sending a reply.
fn spawn_stalling_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            std::thread::sleep(Duration::from_secs(60));
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

fn req_with_timeout(timeout: Duration) -> Request {
    Request {
        system: String::new(),
        messages: vec![Message::User("hi".into())],
        tools: Vec::new(),
        max_output_tokens: None,
        timeout: Some(timeout),
    }
}

fn assert_timeout_err(err: ProviderError, elapsed: Duration, limit: Duration) {
    assert!(
        elapsed < Duration::from_secs(30),
        "expected call to fail within 30s (well before 60s), took {elapsed:?}"
    );
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?}"
    );
    let msg = err.to_string();
    let expected_limit = format!("{limit:?}");
    assert!(
        msg.contains("time limit") && msg.contains(&expected_limit),
        "expected message to mention the request's time limit of {expected_limit}, got: {msg}"
    );
}

#[test]
fn gemini_request_timeout_and_quick_reply() {
    let limit = Duration::from_millis(200);
    let stall_url = spawn_stalling_server();
    let p_stall = Gemini::new_api_key("test-key").with_api_root(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(limit)).unwrap_err();
    assert_timeout_err(err, start.elapsed(), limit);

    let sse_body = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hello from gemini\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":3}}\n\n";
    let quick_url = spawn_quick_server("text/event-stream", sse_body);
    let p_quick = Gemini::new_api_key("test-key").with_api_root(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(Duration::from_secs(30)))
        .expect("quick reply with 30s timeout should succeed");
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
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(40));
                if stream.write_all(b": keepalive\n\n").is_err() {
                    break;
                }
                let _ = stream.flush();
            }
        }
    });
    let limit = Duration::from_millis(250);
    let p = Gemini::new_api_key("test-key").with_api_root(format!("http://{addr}"));
    let start = Instant::now();
    let err = p.complete(&req_with_timeout(limit)).unwrap_err();
    assert_timeout_err(err, start.elapsed(), limit);
}

#[test]
fn anthropic_request_timeout_and_quick_reply() {
    let limit = Duration::from_millis(200);
    let stall_url = spawn_stalling_server();
    let p_stall = Anthropic::new("test-key").with_api_root(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(limit)).unwrap_err();
    assert_timeout_err(err, start.elapsed(), limit);

    let json_body = r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"hello from anthropic"}],"stop_reason":"end_turn","usage":{"input_tokens":4,"output_tokens":3}}"#;
    let quick_url = spawn_quick_server("application/json", json_body);
    let p_quick = Anthropic::new("test-key").with_api_root(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(Duration::from_secs(30)))
        .expect("quick reply with 30s timeout should succeed");
    assert_eq!(resp.text, "hello from anthropic");
}

#[test]
fn openai_request_timeout_and_quick_reply() {
    let limit = Duration::from_millis(200);
    let stall_url = spawn_stalling_server();
    let p_stall = OpenAi::new("test-key").with_base_url(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(limit)).unwrap_err();
    assert_timeout_err(err, start.elapsed(), limit);

    let json_body = r#"{"choices":[{"message":{"role":"assistant","content":"hello from openai"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":3}}"#;
    let quick_url = spawn_quick_server("application/json", json_body);
    let p_quick = OpenAi::new("test-key").with_base_url(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(Duration::from_secs(30)))
        .expect("quick reply with 30s timeout should succeed");
    assert_eq!(resp.text, "hello from openai");
}

#[test]
fn ollama_request_timeout_and_quick_reply() {
    let limit = Duration::from_millis(200);
    let stall_url = spawn_stalling_server();
    let p_stall = Ollama::from_env_or_default()
        .with_model("qwen2.5-coder:32b")
        .with_api_root(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(limit)).unwrap_err();
    assert_timeout_err(err, start.elapsed(), limit);

    let json_body = r#"{"model":"qwen2.5-coder:32b","message":{"role":"assistant","content":"hello from ollama"},"done":true,"done_reason":"stop","prompt_eval_count":4,"eval_count":3}"#;
    let quick_url = spawn_quick_server("application/json", json_body);
    let p_quick = Ollama::from_env_or_default()
        .with_model("qwen2.5-coder:32b")
        .with_api_root(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(Duration::from_secs(30)))
        .expect("quick reply with 30s timeout should succeed");
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
    let limit = Duration::from_millis(200);
    let stall_url = spawn_stalling_server();
    let (_d1, p_stall) = chatgpt_for_test(stall_url);
    let start = Instant::now();
    let err = p_stall.complete(&req_with_timeout(limit)).unwrap_err();
    assert_timeout_err(err, start.elapsed(), limit);

    let sse_body = concat!(
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello from chatgpt\"}]}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":3}}}\n\n",
    );
    let quick_url = spawn_quick_server("text/event-stream", sse_body);
    let (_d2, p_quick) = chatgpt_for_test(quick_url);
    let resp = p_quick
        .complete(&req_with_timeout(Duration::from_secs(30)))
        .expect("quick reply with 30s timeout should succeed");
    assert_eq!(resp.text, "hello from chatgpt");
}

fn read_http_request_opt(stream: &mut std::net::TcpStream) -> bool {
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
                return true;
            }
        }
    }
    false
}

fn req_without_timeout() -> Request {
    Request {
        system: String::new(),
        messages: vec![Message::User("hi".into())],
        tools: Vec::new(),
        max_output_tokens: None,
        timeout: None,
    }
}

fn run_guarded<T: Send + 'static>(
    guard: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> (T, Duration) {
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let res = f();
        let _ = tx.send((res, started.elapsed()));
    });
    match rx.recv_timeout(guard) {
        Ok(pair) => pair,
        Err(_) => panic!(
            "call hung past guard of {guard:?} (elapsed {:?})",
            started.elapsed()
        ),
    }
}

#[test]
fn reused_connection_stalling_before_headers_times_out() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let (reused_tx, reused_rx) = std::sync::mpsc::channel();
    let connections_srv = connections.clone();

    std::thread::spawn(move || {
        let sse_body = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"first reply\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":3}}\n\n";
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else {
                break;
            };
            let idx = connections_srv.fetch_add(1, Ordering::SeqCst);
            let reused_tx = reused_tx.clone();
            std::thread::spawn(move || {
                if idx == 0 {
                    if !read_http_request_opt(&mut stream) {
                        return;
                    }
                    let chunk = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                        sse_body.len(),
                        sse_body
                    );
                    let _ = stream.write_all(chunk.as_bytes());
                    let _ = stream.flush();
                    if read_http_request_opt(&mut stream) {
                        let _ = reused_tx.send(());
                        std::thread::sleep(Duration::from_secs(60));
                    }
                } else {
                    let _ = read_http_request_opt(&mut stream);
                    std::thread::sleep(Duration::from_secs(60));
                }
            });
        }
    });

    let p_first = Gemini::new_api_key("test-key")
        .with_api_root(format!("http://{addr}"))
        .with_stream_timeouts(Duration::from_secs(30), Duration::from_secs(30));
    let first = p_first
        .complete(&req_without_timeout())
        .expect("first call on pooled connection should succeed");
    assert_eq!(first.text, "first reply");

    let p_second = Gemini::new_api_key("test-key")
        .with_api_root(format!("http://{addr}"))
        .with_stream_timeouts(Duration::from_millis(200), Duration::from_secs(30));
    let (second_res, elapsed) = run_guarded(Duration::from_secs(35), move || {
        p_second.complete(&req_without_timeout())
    });

    reused_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("timed out waiting for second request on reused pooled connection");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "client opened a new connection instead of reusing the pooled one"
    );
    let err = second_res.expect_err("second call should time out");
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?} after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "expected second call to time out in under 30s, took {elapsed:?}"
    );
}

#[test]
fn fresh_connection_stalling_before_headers_times_out() {
    let stall_url = spawn_stalling_server();
    let p = Gemini::new_api_key("test-key")
        .with_api_root(stall_url)
        .with_stream_timeouts(Duration::from_millis(200), Duration::from_secs(30));
    let (res, elapsed) = run_guarded(Duration::from_secs(35), move || {
        p.complete(&req_without_timeout())
    });
    let err = res.expect_err("fresh stalling connection should time out");
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?} after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "expected timeout in under 30s, took {elapsed:?}"
    );
}

fn spawn_trickle_without_newline_server(interval: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(interval);
                if stream.write_all(b" ").is_err() {
                    break;
                }
                let _ = stream.flush();
            }
        }
    });
    format!("http://{addr}")
}

#[test]
fn trickle_without_newline_hits_the_cap() {
    let url = spawn_trickle_without_newline_server(Duration::from_millis(20));
    let p = Gemini::new_api_key("test-key")
        .with_api_root(url)
        .with_stream_timeouts(Duration::from_secs(2), Duration::from_millis(2500));
    let (res, elapsed) = run_guarded(Duration::from_secs(35), move || {
        p.complete(&req_without_timeout())
    });
    let err = res.expect_err("trickling stream should hit cap");
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?} after {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_secs(2) && elapsed <= Duration::from_secs(30),
        "expected timeout between 2s and 30s, took {elapsed:?}"
    );
}

#[test]
fn stall_after_first_chunk_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let msg = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"first\"}]}}]}\n\n";
            let _ = stream.write_all(msg.as_bytes());
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(60));
        }
    });
    let p = Gemini::new_api_key("test-key")
        .with_api_root(format!("http://{addr}"))
        .with_stream_timeouts(Duration::from_millis(500), Duration::from_secs(30));
    let (res, elapsed) = run_guarded(Duration::from_secs(35), move || {
        p.complete(&req_without_timeout())
    });
    let err = res.expect_err("stream stalling after first chunk should time out");
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?} after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "expected timeout in under 30s after first chunk, took {elapsed:?}"
    );
}

#[test]
fn slow_but_steady_stream_succeeds() {
    let chunks = [
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"one \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"two \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"three \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"four \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"five\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5}}\n\n",
    ];
    const COMBINED: &str = concat!(
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"one \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"two \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"three \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"four \"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"five\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5}}\n\n",
    );
    let fast_url = spawn_quick_server("text/event-stream", COMBINED);
    let p_fast = Gemini::new_api_key("test-key")
        .with_api_root(fast_url)
        .with_stream_timeouts(Duration::from_secs(30), Duration::from_secs(30));
    let fast_resp = p_fast
        .complete(&req_without_timeout())
        .expect("single-shot response should succeed");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
            for (i, chunk) in chunks.iter().enumerate() {
                if i > 0 {
                    for _ in 0..30 {
                        std::thread::sleep(Duration::from_millis(20));
                        if stream.write_all(b": keepalive\n\n").is_err() {
                            return;
                        }
                        let _ = stream.flush();
                    }
                }
                if stream.write_all(chunk.as_bytes()).is_err() {
                    break;
                }
                let _ = stream.flush();
            }
        }
    });

    let p_slow = Gemini::new_api_key("test-key")
        .with_api_root(format!("http://{addr}"))
        .with_stream_timeouts(Duration::from_secs(2), Duration::from_secs(30));
    let slow_resp = p_slow
        .complete(&req_without_timeout())
        .expect("slow steady stream should succeed");
    assert_eq!(slow_resp.text, fast_resp.text);
    assert_eq!(slow_resp.text, "one two three four five");
    assert_eq!(slow_resp.finish_reason, fast_resp.finish_reason);
    assert_eq!(slow_resp.usage, fast_resp.usage);
}

#[test]
fn request_timeout_shorter_than_cap_wins() {
    let url = spawn_trickle_without_newline_server(Duration::from_millis(25));
    let p = Gemini::new_api_key("test-key")
        .with_api_root(url)
        .with_stream_timeouts(Duration::from_secs(30), Duration::from_secs(60));
    let (res, elapsed) = run_guarded(Duration::from_secs(35), move || {
        p.complete(&req_with_timeout(Duration::from_millis(250)))
    });
    let err = res.expect_err("request timeout shorter than cap should win");
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?} after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "expected timeout in under 30s, took {elapsed:?}"
    );
}

#[test]
fn abandoned_helper_thread_ends() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(45)));
            let mut buf = [0u8; 64];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => {
                        let _ = closed_tx.send(Ok(Instant::now()));
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionAborted
                        ) =>
                    {
                        let _ = closed_tx.send(Ok(Instant::now()));
                        break;
                    }
                    Err(e) => {
                        let _ = closed_tx.send(Err(e.to_string()));
                        break;
                    }
                }
            }
        }
    });

    let cap = Duration::from_millis(400);
    let p = Gemini::new_api_key("test-key")
        .with_api_root(format!("http://{addr}"))
        .with_stream_timeouts(Duration::from_millis(200), cap)
        .with_helper_timeout_grace(Duration::from_millis(200));
    let (res, elapsed) = run_guarded(Duration::from_secs(35), move || {
        p.complete(&req_without_timeout())
    });
    let returned_at = Instant::now();
    let err = res.expect_err("stalling call should time out");
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?} after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "expected call to time out in under 30s, took {elapsed:?}"
    );

    let max_close_wait = cap + Duration::from_secs(30);
    match closed_rx.recv_timeout(max_close_wait) {
        Ok(Ok(closed_at)) => {
            let since_return = closed_at.saturating_duration_since(returned_at);
            assert!(
                since_return <= max_close_wait,
                "client closed connection after {since_return:?}, exceeding {max_close_wait:?}"
            );
        }
        Ok(Err(e)) => panic!("server error waiting for client close: {e}"),
        Err(_) => panic!(
            "abandoned helper thread did not close connection within {max_close_wait:?} after call returned"
        ),
    }
}

#[test]
fn vertex_fresh_connection_stalling_before_headers_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let addr = listener.local_addr().unwrap();
    let (auth_tx, auth_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let mut total = Vec::new();
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                total.extend_from_slice(&buf[..n]);
                if total.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&total).to_ascii_lowercase();
            let _ = auth_tx.send(head.contains("authorization: bearer vertex-test-token"));
            std::thread::sleep(Duration::from_secs(60));
        }
    });

    let p = Gemini::new_vertex("test-project", "global")
        .with_token_source(|| Ok("vertex-test-token".to_string()))
        .with_api_root(format!("http://{addr}"))
        .with_stream_timeouts(Duration::from_millis(200), Duration::from_secs(30));
    let (res, elapsed) = run_guarded(Duration::from_secs(35), move || {
        p.complete(&req_without_timeout())
    });
    let err = res.expect_err("vertex stalling call should time out");
    assert!(
        err.is_timeout(),
        "expected timeout ProviderError, got: {err:?} after {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "expected timeout in under 30s, took {elapsed:?}"
    );
    let saw_auth = auth_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("timed out waiting for Vertex Authorization Bearer header on request");
    assert!(
        saw_auth,
        "expected Vertex Authorization Bearer header on request"
    );
}
