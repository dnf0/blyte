//! Tests for Gemini on Vertex AI with Application Default Credentials.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use blyte_agent::shell::credential_like;
use blyte_provider::gemini::{vertex_url, Gemini};
use blyte_provider::vertex_auth::VertexAuth;
use blyte_provider::{
    detect_provider_kind, resolve_provider, Message, Provider, ProviderError, ProviderKind, Request,
};
use serde_json::json;

#[derive(Clone, Debug)]
struct RecordedRequest {
    #[allow(dead_code)]
    method: String,
    #[allow(dead_code)]
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl RecordedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug)]
struct StubResponse {
    status: u16,
    status_text: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
}

impl StubResponse {
    fn json(status: u16, status_text: &'static str, body: serde_json::Value) -> Self {
        let body_str = body.to_string();
        Self {
            status,
            status_text,
            headers: vec![
                ("Content-Type", "application/json".to_string()),
                ("Connection", "close".to_string()),
            ],
            body: body_str,
        }
    }
}

struct StubServer {
    url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    #[allow(dead_code)]
    responses: Arc<Mutex<Vec<StubResponse>>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StubServer {
    fn new(responses: Vec<StubResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
        listener.set_nonblocking(true).expect("set nonblocking");
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");

        let requests = Arc::new(Mutex::new(Vec::new()));
        let responses = Arc::new(Mutex::new(responses));
        let shutdown = Arc::new(AtomicBool::new(false));

        let req_clone = Arc::clone(&requests);
        let resp_clone = Arc::clone(&responses);
        let shut_clone = Arc::clone(&shutdown);

        let handle = thread::spawn(move || {
            while !shut_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let reqs = Arc::clone(&req_clone);
                        let resps = Arc::clone(&resp_clone);
                        thread::spawn(move || {
                            handle_connection(stream, &reqs, &resps);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            url,
            requests,
            responses,
            shutdown,
            handle: Some(handle),
        }
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    requests: &Arc<Mutex<Vec<RecordedRequest>>>,
    responses: &Arc<Mutex<Vec<StubResponse>>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
        return;
    }
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    let mut headers = Vec::new();
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = val.parse().unwrap_or(0);
            }
            headers.push((key, val));
        }
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body_bytes);
    }
    let body = String::from_utf8_lossy(&body_bytes).to_string();

    requests.lock().unwrap().push(RecordedRequest {
        method,
        path,
        headers,
        body,
    });

    let resp = {
        let mut resps = responses.lock().unwrap();
        if !resps.is_empty() {
            resps.remove(0)
        } else {
            StubResponse::json(
                500,
                "Internal Server Error",
                json!({"error": "no response queued"}),
            )
        }
    };

    let mut response_bytes = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n",
        resp.status,
        resp.status_text,
        resp.body.len()
    );
    for (k, v) in resp.headers {
        response_bytes.push_str(&format!("{k}: {v}\r\n"));
    }
    response_bytes.push_str("\r\n");
    response_bytes.push_str(&resp.body);

    let _ = stream.write_all(response_bytes.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

#[test]
fn test_vertex_url_global_and_europe_west1() {
    assert_eq!(
        vertex_url("my-project", "global", "gemini-3.8-flash"),
        "https://aiplatform.googleapis.com/v1/projects/my-project/locations/global/publishers/google/models/gemini-3.8-flash:generateContent"
    );
    assert_eq!(
        vertex_url("my-project", "europe-west1", "gemini-3.8-flash"),
        "https://europe-west1-aiplatform.googleapis.com/v1/projects/my-project/locations/europe-west1/publishers/google/models/gemini-3.8-flash:generateContent"
    );
}

#[test]
fn test_detect_and_resolve_vertex_provider() {
    assert_eq!(
        detect_provider_kind(Some("vertex/gemini-3.8-flash-high")),
        ProviderKind::Gemini
    );

    std::env::set_var("GOOGLE_CLOUD_PROJECT", "test-project-123");
    std::env::set_var("GOOGLE_OAUTH_ACCESS_TOKEN", "fake-token");

    let p =
        resolve_provider(Some("vertex/gemini-3.8-flash-high")).expect("resolve vertex provider");
    assert_eq!(p.model_id(), "vertex/gemini-3.8-flash");
    assert_eq!(p.thinking_level(), Some("high"));
}

#[test]
fn test_authorized_user_adc_file_and_caching() {
    let dir = tempfile::tempdir().unwrap();
    let adc_path = dir.path().join("application_default_credentials.json");
    let adc_json = json!({
        "type": "authorized_user",
        "client_id": "test-client-id",
        "client_secret": "test-client-secret",
        "refresh_token": "test-refresh-token",
        "quota_project_id": "test-quota-project"
    });
    std::fs::write(&adc_path, adc_json.to_string()).unwrap();

    let stub = StubServer::new(vec![StubResponse::json(
        200,
        "OK",
        json!({
            "access_token": "token-1",
            "expires_in": 3600
        }),
    )]);

    let auth = VertexAuth::from_adc_file(&adc_path, Some(stub.url())).unwrap();

    // First call: contacts stub, returns token-1
    let token1 = auth.token().unwrap();
    assert_eq!(token1, "token-1");

    let reqs = stub.requests();
    assert_eq!(reqs.len(), 1);
    let form_body = &reqs[0].body;
    assert!(
        form_body.contains("grant_type=refresh_token"),
        "{form_body}"
    );
    assert!(
        form_body.contains("client_id=test-client-id"),
        "{form_body}"
    );
    assert!(
        form_body.contains("client_secret=test-client-secret"),
        "{form_body}"
    );
    assert!(
        form_body.contains("refresh_token=test-refresh-token"),
        "{form_body}"
    );

    // Second call within lifetime: does not contact stub again
    let token2 = auth.token().unwrap();
    assert_eq!(token2, "token-1");
    assert_eq!(stub.requests().len(), 1);

    // Now test token with expires_in: 10 (< 300 s) refreshes on the next call
    let stub2 = StubServer::new(vec![
        StubResponse::json(
            200,
            "OK",
            json!({
                "access_token": "short-1",
                "expires_in": 10
            }),
        ),
        StubResponse::json(
            200,
            "OK",
            json!({
                "access_token": "short-2",
                "expires_in": 3600
            }),
        ),
    ]);

    let auth2 = VertexAuth::from_adc_file(&adc_path, Some(stub2.url())).unwrap();
    let tok_a = auth2.token().unwrap();
    assert_eq!(tok_a, "short-1");
    assert_eq!(stub2.requests().len(), 1);

    // Next call: since fewer than 300s remain, it refreshes
    let tok_b = auth2.token().unwrap();
    assert_eq!(tok_b, "short-2");
    assert_eq!(stub2.requests().len(), 2);
}

#[test]
fn test_service_account_error_and_missing_adc_file() {
    let dir = tempfile::tempdir().unwrap();
    let sa_path = dir.path().join("sa.json");
    std::fs::write(&sa_path, json!({"type": "service_account"}).to_string()).unwrap();

    let err = VertexAuth::from_adc_file(&sa_path, None).expect_err("expected error");
    match err {
        ProviderError::Auth(msg) => {
            assert_eq!(
                msg,
                "service-account credentials are not supported yet; set GOOGLE_OAUTH_ACCESS_TOKEN, for example from gcloud auth print-access-token"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let non_existent = dir.path().join("does_not_exist.json");
    let err2 = VertexAuth::from_adc_file(&non_existent, None).expect_err("expected error");
    match err2 {
        ProviderError::MissingKey(key) => {
            assert_eq!(key, "GOOGLE_APPLICATION_CREDENTIALS");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn test_complete_against_vertex_stub() {
    let response_body = json!({
        "candidates": [{
            "content": {
                "parts": [{"text": "Hello from Vertex stub!"}]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": {
            "promptTokenCount": 150,
            "cachedContentTokenCount": 50,
            "candidatesTokenCount": 25,
            "thoughtsTokenCount": 10
        }
    });

    let stub = StubServer::new(vec![StubResponse::json(200, "OK", response_body)]);

    let auth = VertexAuth::from_token("stub-vertex-access-token");
    let client = Gemini::vertex("test-proj", "global", auth)
        .with_vertex_root(stub.url())
        .with_model("gemini-3.8-flash");

    let req = Request {
        messages: vec![Message::User("hi vertex".into())],
        ..Default::default()
    };

    let resp = client.complete(&req).expect("complete call failed");
    assert_eq!(resp.text, "Hello from Vertex stub!");
    assert_eq!(resp.usage.input, 100); // 150 - 50 cached
    assert_eq!(resp.usage.cached, 50);
    assert_eq!(resp.usage.output, 25);
    assert_eq!(resp.usage.thinking, 10);
    assert_eq!(resp.finish_reason.as_deref(), Some("STOP"));

    let reqs = stub.requests();
    assert_eq!(reqs.len(), 1);
    let captured = &reqs[0];
    assert_eq!(
        captured.header("authorization"),
        Some("Bearer stub-vertex-access-token")
    );
    assert_eq!(captured.header("x-goog-user-project"), Some("test-proj"));
    let content_type = captured.header("content-type").unwrap_or_default();
    assert!(content_type.contains("application/json"), "{content_type}");

    let parsed_body: serde_json::Value = serde_json::from_str(&captured.body).unwrap();
    assert_eq!(parsed_body, client.body(&req));
}

#[test]
fn test_401_refresh_and_retry_success() {
    let dir = tempfile::tempdir().unwrap();
    let adc_path = dir.path().join("application_default_credentials.json");
    let adc_json = json!({
        "type": "authorized_user",
        "client_id": "c1",
        "client_secret": "s1",
        "refresh_token": "r1",
        "quota_project_id": "test-proj"
    });
    std::fs::write(&adc_path, adc_json.to_string()).unwrap();

    let token_stub = StubServer::new(vec![
        StubResponse::json(
            200,
            "OK",
            json!({
                "access_token": "tok-1",
                "expires_in": 3600
            }),
        ),
        StubResponse::json(
            200,
            "OK",
            json!({
                "access_token": "tok-2",
                "expires_in": 3600
            }),
        ),
    ]);

    let vertex_stub = StubServer::new(vec![
        StubResponse::json(
            401,
            "Unauthorized",
            json!({
                "error": "unauthorized token"
            }),
        ),
        StubResponse::json(
            200,
            "OK",
            json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "Success on retry"}]
                    },
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 20,
                    "cachedContentTokenCount": 0,
                    "candidatesTokenCount": 10,
                    "thoughtsTokenCount": 0
                }
            }),
        ),
    ]);

    let auth = VertexAuth::from_adc_file(&adc_path, Some(token_stub.url())).unwrap();
    let client = Gemini::vertex("test-proj", "global", auth)
        .with_vertex_root(vertex_stub.url())
        .with_model("gemini-3.8-flash");

    let req = Request {
        messages: vec![Message::User("retry me".into())],
        ..Default::default()
    };

    let resp = client.complete(&req).expect("complete after retry");
    assert_eq!(resp.text, "Success on retry");

    // Vertex stub got 2 calls: first with tok-1, second with tok-2
    let v_reqs = vertex_stub.requests();
    assert_eq!(v_reqs.len(), 2);
    assert_eq!(v_reqs[0].header("authorization"), Some("Bearer tok-1"));
    assert_eq!(v_reqs[1].header("authorization"), Some("Bearer tok-2"));

    // Token stub got 2 calls: one initial, one refresh after 401
    assert_eq!(token_stub.requests().len(), 2);
}

#[test]
fn test_google_oauth_access_token_is_credential_like() {
    assert!(credential_like("GOOGLE_OAUTH_ACCESS_TOKEN", "x"));
}
