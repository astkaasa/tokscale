use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::Result;

const DEFAULT_SURFACE_COLS: u16 = 220;
const DEFAULT_SURFACE_ROWS: u16 = 69;
const MIN_SURFACE_COLS: u16 = 40;
const MAX_SURFACE_COLS: u16 = 320;
const MIN_SURFACE_ROWS: u16 = 16;
const MAX_SURFACE_ROWS: u16 = 120;
const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const REQUEST_HEADER_READ_DEADLINE: Duration = Duration::from_secs(5);
const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const PERMISSIONS_POLICY: &str = "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()";

pub(crate) struct StaticSite {
    pub html: String,
    pub json: String,
    pub review_html: Option<String>,
    pub pulse_json: Option<String>,
    pub pulse_markdown: Option<String>,
    pub surface: Option<Box<dyn Fn(SurfaceSize) -> Result<String> + Send + Sync>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceSize {
    pub cols: u16,
    pub rows: u16,
}

impl Default for SurfaceSize {
    fn default() -> Self {
        Self {
            cols: DEFAULT_SURFACE_COLS,
            rows: DEFAULT_SURFACE_ROWS,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct HttpResponse {
    status: &'static str,
    content_type: &'static str,
    body: String,
    send_body: bool,
}

#[derive(Debug)]
struct RequestHead {
    request_line: String,
    host: Option<String>,
}

#[derive(Debug)]
enum RequestReadError {
    BadRequest,
    HeaderTooLarge,
    TimedOut,
    Io(io::Error),
}

trait RequestHeaderReader: Read {
    fn set_next_read_timeout(&mut self, timeout: Duration) -> io::Result<()>;
}

impl RequestHeaderReader for TcpStream {
    fn set_next_read_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(timeout))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedRequestLine<'a> {
    method: &'a str,
    target: &'a str,
}

pub(crate) fn serve_static_overview(port: u16, site: StaticSite) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let addr = listener.local_addr()?;

    println!();
    println!("  Tokscale Overview: http://{}:{}/", addr.ip(), addr.port());
    println!(
        "  Data JSON:          http://{}:{}/data.json",
        addr.ip(),
        addr.port()
    );
    if site.review_html.is_some() {
        println!(
            "  Weekly Review:      http://{}:{}/review",
            addr.ip(),
            addr.port()
        );
    }
    if site.pulse_json.is_some() {
        println!(
            "  Pulse JSON:         http://{}:{}/api/v1/pulse",
            addr.ip(),
            addr.port()
        );
    }
    if site.pulse_markdown.is_some() {
        println!(
            "  Pulse Markdown:     http://{}:{}/exports/pulse.md",
            addr.ip(),
            addr.port()
        );
    }
    println!("  Press Ctrl-C to stop.");
    println!();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(err) = handle_stream(stream, &site) {
                    eprintln!("  serve error: {err}");
                }
            }
            Err(err) => eprintln!("  connection error: {err}"),
        }
    }

    Ok(())
}

fn handle_stream(mut stream: TcpStream, site: &StaticSite) -> Result<()> {
    let response = match read_request_head(&mut stream, REQUEST_HEADER_READ_DEADLINE) {
        Ok(request) => {
            response_for_request_line(&request.request_line, request.host.as_deref(), site)
        }
        Err(RequestReadError::BadRequest) => {
            plain_text_response("400 Bad Request", "Malformed HTTP request\n")
        }
        Err(RequestReadError::HeaderTooLarge) => plain_text_response(
            "431 Request Header Fields Too Large",
            "Request headers too large\n",
        ),
        Err(RequestReadError::TimedOut) => {
            plain_text_response("408 Request Timeout", "Request header timed out\n")
        }
        Err(RequestReadError::Io(err)) => return Err(err.into()),
    };

    write_response(&mut stream, response)?;
    Ok(())
}

fn read_request_head<R: RequestHeaderReader>(
    reader: &mut R,
    total_timeout: Duration,
) -> Result<RequestHead, RequestReadError> {
    let deadline = Instant::now()
        .checked_add(total_timeout)
        .ok_or(RequestReadError::TimedOut)?;
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(RequestReadError::TimedOut)?;
        if remaining.is_zero() {
            return Err(RequestReadError::TimedOut);
        }
        reader
            .set_next_read_timeout(remaining)
            .map_err(RequestReadError::Io)?;

        let available = MAX_REQUEST_HEADER_BYTES
            .saturating_add(1)
            .saturating_sub(bytes.len());
        let read_len = available.min(chunk.len());
        if read_len == 0 {
            return Err(RequestReadError::HeaderTooLarge);
        }

        let bytes_read = match reader.read(&mut chunk[..read_len]) {
            Ok(0) => return Err(RequestReadError::BadRequest),
            Ok(bytes_read) => bytes_read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(RequestReadError::TimedOut);
            }
            Err(err) => return Err(RequestReadError::Io(err)),
        };
        if Instant::now() >= deadline {
            return Err(RequestReadError::TimedOut);
        }

        let search_from = bytes.len().saturating_sub(3);
        bytes.extend_from_slice(&chunk[..bytes_read]);
        if let Some(header_end) = find_header_end(&bytes, search_from) {
            if header_end + 4 > MAX_REQUEST_HEADER_BYTES {
                return Err(RequestReadError::HeaderTooLarge);
            }
            return parse_request_head(&bytes[..header_end]);
        }
        if bytes.len() > MAX_REQUEST_HEADER_BYTES {
            return Err(RequestReadError::HeaderTooLarge);
        }
    }
}

fn find_header_end(bytes: &[u8], search_from: usize) -> Option<usize> {
    bytes[search_from..]
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| search_from + position)
}

fn parse_request_head(bytes: &[u8]) -> Result<RequestHead, RequestReadError> {
    let head = std::str::from_utf8(bytes).map_err(|_| RequestReadError::BadRequest)?;
    if !head.is_ascii() {
        return Err(RequestReadError::BadRequest);
    }

    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(RequestReadError::BadRequest)?;
    parse_request_line(request_line).map_err(|_| RequestReadError::BadRequest)?;

    let mut host = None;
    let mut duplicate = false;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(RequestReadError::BadRequest)?;
        if !is_http_token(name)
            || value
                .bytes()
                .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err(RequestReadError::BadRequest);
        }
        if name.eq_ignore_ascii_case("host") && host.replace(value.trim().to_string()).is_some() {
            duplicate = true;
        }
    }

    Ok(RequestHead {
        request_line: request_line.to_string(),
        host: if duplicate { None } else { host },
    })
}

fn response_for_request_line(
    request_line: &str,
    host: Option<&str>,
    site: &StaticSite,
) -> HttpResponse {
    let Ok(request) = parse_request_line(request_line) else {
        return plain_text_response("400 Bad Request", "Malformed request line\n");
    };

    let mut response = if !is_allowed_host(host) {
        plain_text_response("403 Forbidden", "Host not allowed\n")
    } else if request.method != "GET" && request.method != "HEAD" {
        plain_text_response("405 Method Not Allowed", "Method not allowed\n")
    } else {
        response_for_target(request.target, site)
    };

    response.send_body = request.method != "HEAD";
    response
}

fn parse_request_line(request_line: &str) -> Result<ParsedRequestLine<'_>, ()> {
    let mut parts = request_line.split(' ');
    let method = parts.next().ok_or(())?;
    let target = parts.next().ok_or(())?;
    let version = parts.next().ok_or(())?;

    if parts.next().is_some()
        || !is_http_token(method)
        || target.is_empty()
        || !target.bytes().all(|byte| byte.is_ascii_graphic())
        || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
    {
        return Err(());
    }

    Ok(ParsedRequestLine { method, target })
}

fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
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
        })
}

fn plain_text_response(status: &'static str, body: &'static str) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "text/plain; charset=utf-8",
        body: body.to_string(),
        send_body: true,
    }
}

fn is_allowed_host(host: Option<&str>) -> bool {
    let Some(authority) = host else {
        return false;
    };
    let authority = authority.trim();
    let (hostname, port) = match authority.split_once(':') {
        Some((hostname, port)) => (hostname, Some(port)),
        None => (authority, None),
    };

    if let Some(port) = port {
        if port.is_empty()
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port.parse::<u16>().is_err()
        {
            return false;
        }
    }

    hostname.eq_ignore_ascii_case("localhost") || hostname == "127.0.0.1"
}

fn response_for_target(target: &str, site: &StaticSite) -> HttpResponse {
    let path = target.split('?').next().unwrap_or(target);
    match path {
        "/" | "/overview" => HttpResponse {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            body: site.html.clone(),
            send_body: true,
        },
        "/data.json" => HttpResponse {
            status: "200 OK",
            content_type: "application/json; charset=utf-8",
            body: site.json.clone(),
            send_body: true,
        },
        "/review" => optional_response(&site.review_html, "text/html; charset=utf-8"),
        "/api/v1/pulse" => optional_response(&site.pulse_json, "application/json; charset=utf-8"),
        "/exports/pulse.md" => {
            optional_response(&site.pulse_markdown, "text/markdown; charset=utf-8")
        }
        "/surface" => match &site.surface {
            Some(render) => match render(surface_size_from_target(target)) {
                Ok(body) => HttpResponse {
                    status: "200 OK",
                    content_type: "text/html; charset=utf-8",
                    body,
                    send_body: true,
                },
                Err(err) => HttpResponse {
                    status: "500 Internal Server Error",
                    content_type: "text/plain; charset=utf-8",
                    body: format!("Render failed: {err}\n"),
                    send_body: true,
                },
            },
            None => HttpResponse {
                status: "404 Not Found",
                content_type: "text/plain; charset=utf-8",
                body: "Surface renderer not configured\n".to_string(),
                send_body: true,
            },
        },
        "/healthz" => HttpResponse {
            status: "200 OK",
            content_type: "text/plain; charset=utf-8",
            body: "ok\n".to_string(),
            send_body: true,
        },
        "/favicon.ico" => HttpResponse {
            status: "204 No Content",
            content_type: "image/x-icon",
            body: String::new(),
            send_body: true,
        },
        _ => not_found_response(),
    }
}

fn optional_response(body: &Option<String>, content_type: &'static str) -> HttpResponse {
    match body {
        Some(body) => HttpResponse {
            status: "200 OK",
            content_type,
            body: body.clone(),
            send_body: true,
        },
        None => not_found_response(),
    }
}

fn not_found_response() -> HttpResponse {
    HttpResponse {
        status: "404 Not Found",
        content_type: "text/plain; charset=utf-8",
        body: "Not found\n".to_string(),
        send_body: true,
    }
}

fn surface_size_from_target(target: &str) -> SurfaceSize {
    let mut size = SurfaceSize::default();
    let Some((_, query)) = target.split_once('?') else {
        return size;
    };

    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let Ok(value) = value.parse::<u16>() else {
            continue;
        };
        match key {
            "cols" => size.cols = value.clamp(MIN_SURFACE_COLS, MAX_SURFACE_COLS),
            "rows" => size.rows = value.clamp(MIN_SURFACE_ROWS, MAX_SURFACE_ROWS),
            _ => {}
        }
    }

    size
}

fn write_response<W: Write>(stream: &mut W, response: HttpResponse) -> Result<()> {
    let body = response.body.as_bytes();
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: {}\r\nReferrer-Policy: no-referrer\r\nPermissions-Policy: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        body.len(),
        CONTENT_SECURITY_POLICY,
        PERMISSIONS_POLICY
    )?;
    if response.send_body {
        stream.write_all(body)?;
    }
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    struct ChunkReader {
        bytes: Vec<u8>,
        position: usize,
        chunk_size: usize,
        delay: Duration,
        timeout_updates: usize,
    }

    impl ChunkReader {
        fn new(bytes: impl Into<Vec<u8>>, chunk_size: usize) -> Self {
            Self {
                bytes: bytes.into(),
                position: 0,
                chunk_size,
                delay: Duration::ZERO,
                timeout_updates: 0,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }
    }

    impl Read for ChunkReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if !self.delay.is_zero() {
                thread::sleep(self.delay);
            }
            if self.position == self.bytes.len() {
                return Ok(0);
            }

            let bytes_read = output
                .len()
                .min(self.chunk_size)
                .min(self.bytes.len() - self.position);
            output[..bytes_read]
                .copy_from_slice(&self.bytes[self.position..self.position + bytes_read]);
            self.position += bytes_read;
            Ok(bytes_read)
        }
    }

    impl RequestHeaderReader for ChunkReader {
        fn set_next_read_timeout(&mut self, _timeout: Duration) -> io::Result<()> {
            self.timeout_updates += 1;
            Ok(())
        }
    }

    fn site() -> StaticSite {
        StaticSite {
            html: "<html>overview</html>".to_string(),
            json: "{\"ok\":true}".to_string(),
            review_html: Some("<html>review</html>".to_string()),
            pulse_json: Some("{\"schemaVersion\":1}".to_string()),
            pulse_markdown: Some("# Pulse\n".to_string()),
            surface: None,
        }
    }

    fn overview_only_site() -> StaticSite {
        let mut site = site();
        site.review_html = None;
        site.pulse_json = None;
        site.pulse_markdown = None;
        site
    }

    fn dynamic_site() -> StaticSite {
        let mut site = site();
        site.surface = Some(Box::new(|size| Ok(format!("{}x{}", size.cols, size.rows))));
        site
    }

    fn local_response(request_line: &str, site: &StaticSite) -> HttpResponse {
        response_for_request_line(request_line, Some("localhost:4317"), site)
    }

    #[test]
    fn reads_complete_request_head_in_bounded_chunks() {
        let mut reader = ChunkReader::new(
            b"GET /review HTTP/1.1\r\nHost: localhost\r\nAccept: text/html\r\n\r\n".to_vec(),
            3,
        );

        let request = read_request_head(&mut reader, Duration::from_secs(1)).unwrap();

        assert_eq!(request.request_line, "GET /review HTTP/1.1");
        assert_eq!(request.host.as_deref(), Some("localhost"));
        assert!(reader.timeout_updates > 1);
    }

    #[test]
    fn rejects_request_head_over_byte_limit() {
        let mut bytes = b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Fill: ".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', MAX_REQUEST_HEADER_BYTES));
        let mut reader = ChunkReader::new(bytes, 1024);

        let error = read_request_head(&mut reader, Duration::from_secs(1)).unwrap_err();

        assert!(matches!(error, RequestReadError::HeaderTooLarge));
        assert!(reader.position <= MAX_REQUEST_HEADER_BYTES + 1);
    }

    #[test]
    fn total_request_head_deadline_does_not_reset_per_read() {
        let bytes = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec();
        let total_bytes = bytes.len();
        let mut reader = ChunkReader::new(bytes, 1).with_delay(Duration::from_millis(8));

        let error = read_request_head(&mut reader, Duration::from_millis(25)).unwrap_err();

        assert!(matches!(error, RequestReadError::TimedOut));
        assert!(reader.position < total_bytes);
    }

    #[test]
    fn malformed_request_lines_return_bad_request() {
        for request_line in [
            "",
            "GET",
            "GET /",
            "GET / HTTP/2",
            "GET / HTTP/1.1 extra",
            "GET  / HTTP/1.1",
            "GET\t/ HTTP/1.1",
            "GE(T / HTTP/1.1",
            "GET / HTTP/1.1\r\n",
        ] {
            let response = local_response(request_line, &site());

            assert_eq!(response.status, "400 Bad Request", "line: {request_line:?}");
        }
    }

    #[test]
    fn accepts_http_1_0_and_1_1_request_lines() {
        for request_line in ["GET /healthz HTTP/1.0", "GET /healthz HTTP/1.1"] {
            let response = local_response(request_line, &site());

            assert_eq!(response.status, "200 OK", "line: {request_line:?}");
        }
    }

    #[test]
    fn malformed_header_line_is_a_bad_request() {
        let error = parse_request_head(b"GET / HTTP/1.1\r\nHost: localhost\r\nBroken").unwrap_err();

        assert!(matches!(error, RequestReadError::BadRequest));
    }

    #[test]
    fn routes_root_and_overview_to_html() {
        for path in ["/", "/overview"] {
            let response = local_response(&format!("GET {path} HTTP/1.1"), &site());

            assert_eq!(response.status, "200 OK");
            assert_eq!(response.content_type, "text/html; charset=utf-8");
            assert_eq!(response.body, "<html>overview</html>");
        }
    }

    #[test]
    fn routes_data_json_to_snapshot() {
        let response = local_response("GET /data.json HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "application/json; charset=utf-8");
        assert_eq!(response.body, "{\"ok\":true}");
    }

    #[test]
    fn routes_review_to_html() {
        let response = local_response("GET /review HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert_eq!(response.body, "<html>review</html>");
    }

    #[test]
    fn routes_pulse_api_to_json() {
        let response = local_response("GET /api/v1/pulse HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "application/json; charset=utf-8");
        assert_eq!(response.body, "{\"schemaVersion\":1}");
    }

    #[test]
    fn routes_pulse_export_to_markdown() {
        let response = local_response("GET /exports/pulse.md HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "text/markdown; charset=utf-8");
        assert_eq!(response.body, "# Pulse\n");
    }

    #[test]
    fn routes_surface_to_dynamic_renderer() {
        let response = local_response("GET /surface?cols=96&rows=32 HTTP/1.1", &dynamic_site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert_eq!(response.body, "96x32");
    }

    #[test]
    fn clamps_surface_size() {
        let response = local_response("GET /surface?cols=999&rows=2 HTTP/1.1", &dynamic_site());

        assert_eq!(response.body, "320x16");
    }

    #[test]
    fn routes_health_check() {
        let response = local_response("GET /healthz HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "text/plain; charset=utf-8");
        assert_eq!(response.body, "ok\n");
    }

    #[test]
    fn missing_optional_routes_return_not_found() {
        let site = overview_only_site();

        for path in ["/review", "/api/v1/pulse", "/exports/pulse.md"] {
            let response = local_response(&format!("GET {path} HTTP/1.1"), &site);

            assert_eq!(response.status, "404 Not Found");
            assert_eq!(response.content_type, "text/plain; charset=utf-8");
            assert_eq!(response.body, "Not found\n");
        }
    }

    #[test]
    fn unknown_route_returns_not_found() {
        let response = local_response("GET /missing HTTP/1.1", &site());

        assert_eq!(response.status, "404 Not Found");
        assert_eq!(response.body, "Not found\n");
    }

    #[test]
    fn rejects_non_get_methods() {
        let response = local_response("POST / HTTP/1.1", &site());

        assert_eq!(response.status, "405 Method Not Allowed");
    }

    #[test]
    fn accepts_local_hosts_with_optional_ports() {
        for host in [
            "localhost",
            "LOCALHOST:8080",
            "127.0.0.1",
            "127.0.0.1:65535",
        ] {
            let response = response_for_request_line("GET /healthz HTTP/1.1", Some(host), &site());

            assert_eq!(response.status, "200 OK", "host: {host}");
        }
    }

    #[test]
    fn rejects_missing_or_unsafe_hosts() {
        for host in [
            None,
            Some("example.com"),
            Some("localhost.example.com"),
            Some("127.0.0.1.example.com"),
            Some("localhost:invalid"),
            Some("localhost:65536"),
            Some("[::1]"),
        ] {
            let response = response_for_request_line("GET / HTTP/1.1", host, &site());

            assert_eq!(response.status, "403 Forbidden", "host: {host:?}");
            assert_eq!(response.body, "Host not allowed\n");
        }
    }

    #[test]
    fn rejects_duplicate_host_headers() {
        let request =
            parse_request_head(b"GET / HTTP/1.1\r\nHost: localhost\r\nHost: 127.0.0.1").unwrap();
        let response =
            response_for_request_line(&request.request_line, request.host.as_deref(), &site());

        assert_eq!(request.host, None);
        assert_eq!(response.status, "403 Forbidden");
    }

    #[test]
    fn head_keeps_content_length_but_sends_no_body() {
        let response = local_response("HEAD /exports/pulse.md HTTP/1.1", &site());
        let content_length = response.body.len();

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "text/markdown; charset=utf-8");
        assert_eq!(response.body, "# Pulse\n");
        assert!(!response.send_body);

        let mut output = Vec::new();
        write_response(&mut output, response).unwrap();
        let output = String::from_utf8(output).unwrap();
        let (_, body) = output.split_once("\r\n\r\n").unwrap();

        assert!(output.contains(&format!("Content-Length: {content_length}\r\n")));
        assert!(body.is_empty());
    }

    #[test]
    fn writes_no_store_and_browser_security_headers() {
        let response = local_response("GET / HTTP/1.1", &site());
        let mut output = Vec::new();

        write_response(&mut output, response).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("Cache-Control: no-store\r\n"));
        assert!(output.contains("X-Content-Type-Options: nosniff\r\n"));
        assert!(output.contains(&format!(
            "Content-Security-Policy: {CONTENT_SECURITY_POLICY}\r\n"
        )));
        assert!(output.contains("frame-ancestors 'none'"));
        assert!(output.contains("Referrer-Policy: no-referrer\r\n"));
        assert!(output.contains(&format!("Permissions-Policy: {PERMISSIONS_POLICY}\r\n")));
    }
}
