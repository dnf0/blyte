use blyte_cli::http::{serve, Request, Response};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

fn start_server<F>(handler: F) -> u16
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    std::thread::spawn(move || {
        let _ = serve(listener, handler);
    });
    port
}

fn send_raw(port: u16, raw: &[u8]) -> RawResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set_read_timeout");
    stream.write_all(raw).expect("write_all");
    stream.flush().expect("flush");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("read_to_end");
    parse_response(&buf)
}

fn send_raw_and_shutdown_write(port: u16, raw: &[u8]) -> RawResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set_read_timeout");
    stream.write_all(raw).expect("write_all");
    stream.flush().expect("flush");
    stream.shutdown(Shutdown::Write).expect("shutdown write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).expect("read_to_end");
    parse_response(&buf)
}

struct RawResponse {
    status_line: String,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl RawResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn body_str(&self) -> &str {
        std::str::from_utf8(&self.body).expect("utf-8 body")
    }
}

fn parse_response(raw: &[u8]) -> RawResponse {
    let pos = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response header terminator");
    let head = std::str::from_utf8(&raw[..pos]).expect("utf-8 headers");
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("status line").to_string();
    let status = status_line
        .split(' ')
        .nth(1)
        .expect("status code")
        .parse::<u16>()
        .expect("numeric status");
    let mut headers = Vec::new();
    for line in lines {
        let (k, v) = line.split_once(':').expect("header colon");
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    RawResponse {
        status_line,
        status,
        headers,
        body: raw[pos + 4..].to_vec(),
    }
}

#[derive(Clone)]
struct CapturedQueryRequest {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    x: Option<String>,
    y: Option<String>,
    z: Option<String>,
    missing: Option<String>,
}

#[test]
fn get_with_query() {
    let captured: Arc<Mutex<Option<CapturedQueryRequest>>> = Arc::new(Mutex::new(None));
    let captured_clone = Arc::clone(&captured);

    let port = start_server(move |req| {
        *captured_clone.lock().unwrap() = Some(CapturedQueryRequest {
            method: req.method.clone(),
            path: req.path.clone(),
            query: req.query.clone(),
            x: req.query_value("x").map(str::to_string),
            y: req.query_value("y").map(str::to_string),
            z: req.query_value("z").map(str::to_string),
            missing: req.query_value("missing").map(str::to_string),
        });
        Response::text(200, "ok")
    });

    let req = format!("GET /a%20b?x=1&y=two+words&z HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status_line, "HTTP/1.1 200 OK");
    assert_eq!(resp.body_str(), "ok");

    let got = captured.lock().unwrap().clone().expect("handler called");
    assert_eq!(got.method, "GET");
    assert_eq!(got.path, "/a b");
    assert_eq!(
        got.query,
        vec![
            ("x".to_string(), "1".to_string()),
            ("y".to_string(), "two words".to_string()),
            ("z".to_string(), "".to_string()),
        ]
    );
    assert_eq!(got.x.as_deref(), Some("1"));
    assert_eq!(got.y.as_deref(), Some("two words"));
    assert_eq!(got.z.as_deref(), Some(""));
    assert_eq!(got.missing, None);
}

#[test]
fn post_with_body() {
    let seen_body: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen_body);

    let port = start_server(move |req| {
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/submit");
        *seen_clone.lock().unwrap() = Some(req.body.clone());
        Response::json(202, &serde_json::json!({"accepted": true}))
    });

    let payload = br#"{"msg":"hello \u0000 world","n":42}"#;
    let mut raw = format!(
        "POST /submit HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        payload.len()
    )
    .into_bytes();
    raw.extend_from_slice(payload);

    let resp = send_raw(port, &raw);
    assert_eq!(resp.status_line, "HTTP/1.1 202 Accepted");
    assert_eq!(resp.header("content-type"), Some("application/json"));
    let parsed: serde_json::Value = serde_json::from_slice(&resp.body).expect("json response");
    assert_eq!(parsed, serde_json::json!({"accepted": true}));

    let got = seen_body.lock().unwrap().clone().expect("body captured");
    assert_eq!(got, payload);
}

#[test]
fn post_without_length_is_411() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let req = format!("POST /item HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 411);
    assert_eq!(resp.status_line, "HTTP/1.1 411 Length Required");
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn oversized_body_is_413() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let start = Instant::now();
    let req = format!(
        "POST /upload HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 2000000\r\n\r\n"
    );
    let resp = send_raw(port, req.as_bytes());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(resp.status, 413);
    assert_eq!(resp.status_line, "HTTP/1.1 413 Payload Too Large");
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn short_body_is_400() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let req =
        format!("POST /upload HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 10\r\n\r\nabc");
    let resp = send_raw_and_shutdown_write(port, req.as_bytes());
    assert_eq!(resp.status, 400);
    assert_eq!(resp.status_line, "HTTP/1.1 400 Bad Request");
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn put_is_405() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let req = format!("PUT /item HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 405);
    assert_eq!(resp.status_line, "HTTP/1.1 405 Method Not Allowed");
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn bad_escape_is_400() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let req = format!("GET /bad%ZZpath HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 400);
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));

    let req_truncated = format!("GET /bad%2 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp2 = send_raw(port, req_truncated.as_bytes());
    assert_eq!(resp2.status, 400);
    assert!(!called.load(Ordering::SeqCst));

    let req_query = format!("GET /ok?a=%GG HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp3 = send_raw(port, req_query.as_bytes());
    assert_eq!(resp3.status, 400);
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn foreign_host_is_403() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let req = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    let resp = send_raw(port, req);
    assert_eq!(resp.status, 403);
    assert_eq!(resp.status_line, "HTTP/1.1 403 Forbidden");
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn missing_host_is_403() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let req = b"GET / HTTP/1.1\r\n\r\n";
    let resp = send_raw(port, req);
    assert_eq!(resp.status, 403);
    assert_eq!(resp.status_line, "HTTP/1.1 403 Forbidden");
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn foreign_origin_post_is_403() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let req = format!(
        "POST /action HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://example.com\r\nContent-Length: 0\r\n\r\n"
    );
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 403);
    assert_eq!(resp.status_line, "HTTP/1.1 403 Forbidden");
    assert!(!resp.body_str().is_empty());
    assert!(!called.load(Ordering::SeqCst));

    let wrong_port = if port == 65535 { port - 1 } else { port + 1 };
    let req_wrong_port = format!(
        "POST /action HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{wrong_port}\r\nContent-Length: 0\r\n\r\n"
    );
    let resp2 = send_raw(port, req_wrong_port.as_bytes());
    assert_eq!(resp2.status, 403);
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn own_origin_post_is_accepted() {
    let port = start_server(|_| Response::text(200, "accepted"));

    for origin in [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ] {
        let req = format!(
            "POST /action HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nContent-Length: 2\r\n\r\nhi"
        );
        let resp = send_raw(port, req.as_bytes());
        assert_eq!(resp.status, 200, "origin {origin} should be accepted");
        assert_eq!(resp.body_str(), "accepted");
    }
}

#[test]
fn get_ignores_origin() {
    let port = start_server(|_| Response::text(200, "ok"));

    let req = format!(
        "GET /info HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://example.com\r\n\r\n"
    );
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_str(), "ok");
}

#[test]
fn slow_handler_does_not_block_others() {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);

    let port = start_server(move |req| {
        if req.path == "/slow" {
            let _ = tx.lock().unwrap().send(());
            std::thread::sleep(Duration::from_secs(1));
            Response::text(200, "slow")
        } else {
            Response::text(200, "fast")
        }
    });

    let slow_handle = std::thread::spawn(move || {
        let req = format!("GET /slow HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        send_raw(port, req.as_bytes())
    });

    rx.recv_timeout(Duration::from_secs(2))
        .expect("slow handler entered");

    let start = Instant::now();
    let fast_req = format!("GET /fast HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let fast_resp = send_raw(port, fast_req.as_bytes());
    let elapsed = start.elapsed();

    assert_eq!(fast_resp.status, 200);
    assert_eq!(fast_resp.body_str(), "fast");
    assert!(
        elapsed < Duration::from_millis(500),
        "fast request took {elapsed:?}"
    );

    let slow_resp = slow_handle.join().expect("slow thread");
    assert_eq!(slow_resp.status, 200);
    assert_eq!(slow_resp.body_str(), "slow");
}

#[test]
fn panicking_handler_is_500_and_server_survives() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&count);

    let port = start_server(move |_| {
        let n = count_clone.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            panic!("handler exploded");
        }
        Response::text(200, "alive")
    });

    let req = format!("GET /first HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp1 = send_raw(port, req.as_bytes());
    assert_eq!(resp1.status_line, "HTTP/1.1 500 Internal Server Error");
    assert_eq!(resp1.body_str(), "internal error");

    let req2 = format!("GET /second HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp2 = send_raw(port, req2.as_bytes());
    assert_eq!(resp2.status, 200);
    assert_eq!(resp2.body_str(), "alive");
}

#[test]
fn response_headers() {
    let port = start_server(|req| {
        if req.path == "/html" {
            Response::html(b"<h1>hi</h1>".to_vec())
        } else {
            Response::text(200, "hello")
        }
    });

    let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-length"), Some("5"));
    assert_eq!(resp.header("connection"), Some("close"));
    assert_eq!(resp.header("cache-control"), Some("no-store"));
    assert_eq!(
        resp.header("content-type"),
        Some("text/plain; charset=utf-8")
    );

    let html_req = format!("GET /html HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let html_resp = send_raw(port, html_req.as_bytes());
    assert_eq!(html_resp.status, 200);
    assert_eq!(html_resp.header("content-length"), Some("11"));
    assert_eq!(html_resp.header("connection"), Some("close"));
    assert_eq!(html_resp.header("cache-control"), Some("no-store"));
    assert_eq!(
        html_resp.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(html_resp.body_str(), "<h1>hi</h1>");
}

#[test]
fn headers_over_16_kib_is_400() {
    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);
    let port = start_server(move |_| {
        called_clone.store(true, Ordering::SeqCst);
        Response::text(200, "ok")
    });

    let big = "a".repeat(17 * 1024);
    let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Big: {big}\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 400);
    assert!(!called.load(Ordering::SeqCst));
}

#[test]
fn invalid_content_length_and_malformed_line_are_400() {
    let port = start_server(|_| Response::text(200, "ok"));

    let req = format!("POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: nope\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status, 400);

    let req2 = b"NOT_A_REQUEST_LINE\r\n\r\n";
    let resp2 = send_raw(port, req2);
    assert_eq!(resp2.status, 400);
}

#[test]
fn unknown_status_from_handler_is_500() {
    let port = start_server(|_| Response {
        status: 418,
        content_type: "text/plain; charset=utf-8",
        body: b"teapot".to_vec(),
    });

    let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    let resp = send_raw(port, req.as_bytes());
    assert_eq!(resp.status_line, "HTTP/1.1 500 Internal Server Error");
    assert_eq!(resp.body_str(), "internal error");
}

#[test]
fn case_insensitive_headers_and_allowed_hosts() {
    let port = start_server(|req| Response::text(200, std::str::from_utf8(&req.body).unwrap()));

    for host in [
        "localhost",
        "LOCALHOST:8080",
        "[::1]",
        "[::1]:9000",
        "127.0.0.1",
    ] {
        let req = format!(
            "POST /echo HTTP/1.1\r\nhOsT: {host}\r\ncOnTeNt-LeNgTh: 3\r\noRiGiN: http://localhost:{port}\r\n\r\nabc"
        );
        let resp = send_raw(port, req.as_bytes());
        assert_eq!(resp.status, 200, "host {host} should be allowed");
        assert_eq!(resp.body_str(), "abc");
    }
}
