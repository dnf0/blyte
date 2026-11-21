use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Request {
    pub fn query_value(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

impl Response {
    pub fn json(status: u16, value: &serde_json::Value) -> Response {
        Response {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(value).expect("a JSON value serialises"),
        }
    }

    pub fn text(status: u16, body: &str) -> Response {
        Response {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn html(body: Vec<u8>) -> Response {
        Response {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body,
        }
    }
}

pub fn serve<F>(listener: TcpListener, handler: F) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let port = listener.local_addr()?.port();
    let handler = Arc::new(handler);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let handler = Arc::clone(&handler);
                std::thread::spawn(move || {
                    handle_connection(stream, port, handler.as_ref());
                });
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

fn handle_connection<F>(mut stream: TcpStream, port: u16, handler: &F)
where
    F: Fn(&Request) -> Response,
{
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let response = match read_request(&mut stream, port) {
        Ok(Some(request)) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&request))) {
                Ok(resp) => resp,
                Err(_) => Response::text(500, "internal error"),
            }
        }
        Ok(None) => return,
        Err(err_response) => err_response,
    };
    write_response(&mut stream, response);
}

struct RawHead {
    head: Vec<u8>,
    leftover: Vec<u8>,
}

fn read_request(stream: &mut TcpStream, port: u16) -> Result<Option<Request>, Response> {
    let Some(RawHead {
        head: head_bytes,
        leftover,
    }) = read_head(stream)?
    else {
        return Ok(None);
    };

    let head = std::str::from_utf8(&head_bytes)
        .map_err(|_| Response::text(400, "request headers are not valid UTF-8"))?;

    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| Response::text(400, "missing request line"))?;

    let (method, target) = parse_request_line(request_line)?;
    let (path, query) = parse_target(target)?;

    let mut host_values: Vec<&str> = Vec::new();
    let mut origin_values: Vec<&str> = Vec::new();
    let mut content_length_values: Vec<&str> = Vec::new();

    for line in lines {
        if line.contains('\r') || line.contains('\n') {
            return Err(Response::text(400, "malformed header line"));
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| Response::text(400, "malformed header line"))?;
        if name.is_empty() || !name.bytes().all(is_header_name_char) {
            return Err(Response::text(400, "malformed header name"));
        }
        let value = value.trim_matches(|c| c == ' ' || c == '\t');
        if name.eq_ignore_ascii_case("host") {
            host_values.push(value);
        } else if name.eq_ignore_ascii_case("origin") {
            origin_values.push(value);
        } else if name.eq_ignore_ascii_case("content-length") {
            content_length_values.push(value);
        }
    }

    if method != "GET" && method != "POST" {
        return Err(Response::text(405, "method not allowed"));
    }

    match host_values.as_slice() {
        [] => return Err(Response::text(403, "missing Host header")),
        [host] => {
            if !is_allowed_host(host) {
                return Err(Response::text(403, "forbidden Host header"));
            }
        }
        _ => return Err(Response::text(400, "duplicate Host header")),
    }

    if method == "POST" {
        for origin in &origin_values {
            if !is_allowed_origin(origin, port) {
                return Err(Response::text(403, "forbidden Origin header"));
            }
        }
    }

    let content_length = match content_length_values.as_slice() {
        [] => {
            if method == "POST" {
                return Err(Response::text(411, "missing Content-Length header"));
            }
            0
        }
        [raw_len] => parse_content_length(raw_len)?,
        _ => return Err(Response::text(400, "duplicate Content-Length header")),
    };

    let body = read_body(stream, &leftover, content_length)?;

    Ok(Some(Request {
        method: method.to_string(),
        path,
        query,
        body,
    }))
}

fn read_head(stream: &mut TcpStream) -> Result<Option<RawHead>, Response> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            if pos > MAX_HEADER_BYTES {
                return Err(Response::text(400, "request headers exceed 16 KiB"));
            }
            break pos;
        }
        if buf.len() >= MAX_HEADER_BYTES + 4 {
            return Err(Response::text(400, "request headers exceed 16 KiB"));
        }
        match stream.read(&mut tmp) {
            Ok(0) => {
                if buf.is_empty() {
                    return Ok(None);
                }
                return Err(Response::text(400, "incomplete HTTP request"));
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err)
                if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock =>
            {
                if buf.is_empty() {
                    return Ok(None);
                }
                return Err(Response::text(400, "timed out reading request headers"));
            }
            Err(_) => return Ok(None),
        }
    };

    let leftover = buf[head_end + 4..].to_vec();
    buf.truncate(head_end);
    Ok(Some(RawHead {
        head: buf,
        leftover,
    }))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_request_line(line: &str) -> Result<(&str, &str), Response> {
    if line.is_empty() || line.contains('\r') || line.contains('\n') {
        return Err(Response::text(400, "malformed request line"));
    }
    let mut parts = line.split(' ');
    let method = parts
        .next()
        .filter(|s| !s.is_empty() && s.bytes().all(is_header_name_char))
        .ok_or_else(|| Response::text(400, "malformed request line"))?;
    let target = parts
        .next()
        .filter(|s| s.starts_with('/') && !s.bytes().any(|b| b < 0x21 || b == 0x7f))
        .ok_or_else(|| Response::text(400, "malformed request line"))?;
    let version = parts
        .next()
        .filter(|s| *s == "HTTP/1.1" || *s == "HTTP/1.0")
        .ok_or_else(|| Response::text(400, "malformed request line"))?;
    if parts.next().is_some() {
        return Err(Response::text(400, "malformed request line"));
    }
    let _ = version;
    Ok((method, target))
}

fn parse_target(target: &str) -> Result<(String, Vec<(String, String)>), Response> {
    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let path = percent_decode(raw_path, false)?;
    let mut query = Vec::new();
    if let Some(q) = raw_query {
        if !q.is_empty() {
            for pair in q.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (raw_key, raw_value) = match pair.split_once('=') {
                    Some((k, v)) => (k, v),
                    None => (pair, ""),
                };
                let key = percent_decode(raw_key, true)?;
                let value = percent_decode(raw_value, true)?;
                query.push((key, value));
            }
        }
    }
    Ok((path, query))
}

fn percent_decode(input: &str, plus_as_space: bool) -> Result<String, Response> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return Err(Response::text(400, "invalid percent escape"));
                }
                let hi = hex_value(bytes[i + 1])
                    .ok_or_else(|| Response::text(400, "invalid percent escape"))?;
                let lo = hex_value(bytes[i + 2])
                    .ok_or_else(|| Response::text(400, "invalid percent escape"))?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| Response::text(400, "invalid UTF-8 in percent escape"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_header_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn is_valid_port(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u16>().is_ok()
}

fn is_allowed_host(raw: &str) -> bool {
    let value = raw.trim();
    if let Some(rest) = value.strip_prefix("[::1]") {
        return rest.is_empty() || rest.strip_prefix(':').is_some_and(is_valid_port);
    }
    let (host, port) = match value.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (value, None),
    };
    if let Some(p) = port {
        if !is_valid_port(p) {
            return false;
        }
    }
    host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost")
}

fn is_allowed_origin(raw: &str, port: u16) -> bool {
    let value = raw.trim();
    let Some(rest) = value.strip_prefix("http://") else {
        return false;
    };
    let suffix = format!(":{port}");
    let Some(host) = rest.strip_suffix(&suffix) else {
        return false;
    };
    host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost") || host == "[::1]"
}

fn parse_content_length(raw: &str) -> Result<usize, Response> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Response::text(400, "invalid Content-Length header"));
    }
    let len = match trimmed.parse::<usize>() {
        Ok(n) => n,
        Err(_) => return Err(Response::text(413, "request body exceeds 1 MiB")),
    };
    if len > MAX_BODY_BYTES {
        return Err(Response::text(413, "request body exceeds 1 MiB"));
    }
    Ok(len)
}

fn read_body(
    stream: &mut TcpStream,
    leftover: &[u8],
    content_length: usize,
) -> Result<Vec<u8>, Response> {
    if content_length == 0 {
        return Ok(Vec::new());
    }
    if leftover.len() >= content_length {
        return Ok(leftover[..content_length].to_vec());
    }
    let mut body = Vec::with_capacity(content_length);
    body.extend_from_slice(leftover);
    let mut tmp = [0u8; 8192];
    while body.len() < content_length {
        let want = (content_length - body.len()).min(tmp.len());
        match stream.read(&mut tmp[..want]) {
            Ok(0) => {
                return Err(Response::text(
                    400,
                    "request body shorter than Content-Length",
                ));
            }
            Ok(n) => body.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err)
                if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock =>
            {
                return Err(Response::text(400, "timed out reading request body"));
            }
            Err(_) => {
                return Err(Response::text(400, "failed reading request body"));
            }
        }
    }
    Ok(body)
}

fn reason_phrase(status: u16) -> Option<&'static str> {
    match status {
        200 => Some("OK"),
        201 => Some("Created"),
        202 => Some("Accepted"),
        204 => Some("No Content"),
        400 => Some("Bad Request"),
        403 => Some("Forbidden"),
        404 => Some("Not Found"),
        405 => Some("Method Not Allowed"),
        409 => Some("Conflict"),
        411 => Some("Length Required"),
        413 => Some("Payload Too Large"),
        422 => Some("Unprocessable Entity"),
        500 => Some("Internal Server Error"),
        _ => None,
    }
}

fn write_response(stream: &mut TcpStream, response: Response) {
    let response = match reason_phrase(response.status) {
        Some(_) => response,
        None => Response::text(500, "internal error"),
    };
    let reason = reason_phrase(response.status).unwrap_or("Internal Server Error");
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}
