use std::io::{BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use blyte_provider::gemini::Gemini;
use blyte_provider::ProviderError;

#[test]
fn default_timeouts() {
    let (whole, read) = blyte_provider::model_timeouts_from(None);
    assert_eq!(whole, Duration::from_secs(180));
    assert_eq!(read, Duration::from_secs(150));

    // At least 30s read timeout
    let (whole40, read40) = blyte_provider::model_timeouts_from(Some("40"));
    assert_eq!(whole40, Duration::from_secs(40));
    assert_eq!(read40, Duration::from_secs(30));

    let client = blyte_provider::build_http_client(whole, read);
    drop(client);
}

#[test]
fn streaming_timeout_helpers_defaults_and_parsing() {
    assert_eq!(blyte_provider::model_idle_timeout_secs_from(None), 90);
    assert_eq!(blyte_provider::model_idle_timeout_secs_from(Some("")), 90);
    assert_eq!(blyte_provider::model_idle_timeout_secs_from(Some("0")), 90);
    assert_eq!(
        blyte_provider::model_idle_timeout_secs_from(Some("not-a-number")),
        90
    );
    assert_eq!(
        blyte_provider::model_idle_timeout_secs_from(Some(" 45 ")),
        45
    );

    assert_eq!(blyte_provider::model_stream_cap_secs_from(None), 1800);
    assert_eq!(blyte_provider::model_stream_cap_secs_from(Some("")), 1800);
    assert_eq!(blyte_provider::model_stream_cap_secs_from(Some("0")), 1800);
    assert_eq!(
        blyte_provider::model_stream_cap_secs_from(Some("bad")),
        1800
    );
    assert_eq!(blyte_provider::model_stream_cap_secs_from(Some("600")), 600);

    let a = blyte_provider::streaming_http_client() as *const ureq::Agent;
    let b = blyte_provider::streaming_http_client() as *const ureq::Agent;
    assert_eq!(a, b);
}

#[test]
fn env_override_gives_60_and_30() {
    unsafe {
        std::env::set_var("BLYTE_MODEL_TIMEOUT_SECS", "60");
    }
    let (whole, read) = blyte_provider::model_timeouts();
    assert_eq!(whole, Duration::from_secs(60));
    assert_eq!(read, Duration::from_secs(30));
    assert_eq!(blyte_provider::model_stream_cap_secs(), 60);
    let client = blyte_provider::http_client();
    let _ = client;
}

fn read_http_request(stream: &mut std::net::TcpStream) {
    let mut buf = [0u8; 1024];
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
}

#[test]
fn streaming_idle_timeout_fires_when_server_stalls_after_one_chunk() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"first\"}]}}]}\n\n",
            );
            let _ = stream.flush();
            // Stall past the 1 s idle timeout.
            std::thread::sleep(Duration::from_millis(2500));
        }
    });

    let client = blyte_provider::build_streaming_http_client(Duration::from_secs(1));
    let started = Instant::now();
    let resp = client
        .get(&format!("http://{addr}/stream"))
        .set("Accept", "text/event-stream")
        .call()
        .unwrap();
    let reader = BufReader::new(resp.into_reader());
    let err = Gemini::parse_stream(reader, Duration::from_secs(60), started).unwrap_err();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "expected failure within a few seconds, took {:?}",
        started.elapsed()
    );
    assert!(
        err.is_timeout(),
        "expected timeout Transport error, got {err:?}"
    );
    assert!(matches!(err, ProviderError::Transport(_)), "{err:?}");

    let _ = server.join();
}

#[test]
fn streaming_succeeds_past_idle_timeout_when_chunks_keep_arriving() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            for i in 0..6 {
                std::thread::sleep(Duration::from_millis(500));
                let chunk = if i == 5 {
                    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"!\"}]},\"finishReason\":\"STOP\"}]}\n\n"
                } else {
                    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"x\"}]}}]}\n\n"
                };
                if stream.write_all(chunk.as_bytes()).is_err() {
                    break;
                }
                let _ = stream.flush();
            }
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let mut drain = [0u8; 64];
            let _ = stream.read(&mut drain);
        }
    });

    let client = blyte_provider::build_streaming_http_client(Duration::from_secs(1));
    let started = Instant::now();
    let resp = client
        .get(&format!("http://{addr}/stream"))
        .set("Accept", "text/event-stream")
        .call()
        .unwrap();
    let reader = BufReader::new(resp.into_reader());
    let r = Gemini::parse_stream(reader, Duration::from_secs(60), started).unwrap();

    assert_eq!(r.text, "xxxxx!");
    assert_eq!(r.finish_reason.as_deref(), Some("STOP"));
    assert!(started.elapsed() >= Duration::from_millis(2500));

    let _ = server.join();
}
