use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use anyhow::Result;

const DEFAULT_SURFACE_COLS: u16 = 220;
const DEFAULT_SURFACE_ROWS: u16 = 69;
const MIN_SURFACE_COLS: u16 = 40;
const MAX_SURFACE_COLS: u16 = 320;
const MIN_SURFACE_ROWS: u16 = 16;
const MAX_SURFACE_ROWS: u16 = 120;

pub(crate) struct StaticSite {
    pub html: String,
    pub json: String,
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
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request_line = String::new();
    {
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut request_line)?;
    }

    let response = response_for_request_line(&request_line, site);
    write_response(&mut stream, response)?;
    Ok(())
}

fn response_for_request_line(request_line: &str, site: &StaticSite) -> HttpResponse {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or("/");

    if method != "GET" && method != "HEAD" {
        return HttpResponse {
            status: "405 Method Not Allowed",
            content_type: "text/plain; charset=utf-8",
            body: "Method not allowed\n".to_string(),
            send_body: true,
        };
    }

    let mut response = response_for_target(target, site);
    response.send_body = method == "GET";
    response
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
        _ => HttpResponse {
            status: "404 Not Found",
            content_type: "text/plain; charset=utf-8",
            body: "Not found\n".to_string(),
            send_body: true,
        },
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

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
    let body = response.body.as_bytes();
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        body.len()
    )?;
    if response.send_body {
        stream.write_all(body)?;
    }
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> StaticSite {
        StaticSite {
            html: "<html>overview</html>".to_string(),
            json: "{\"ok\":true}".to_string(),
            surface: None,
        }
    }

    fn dynamic_site() -> StaticSite {
        StaticSite {
            html: "<html>overview</html>".to_string(),
            json: "{\"ok\":true}".to_string(),
            surface: Some(Box::new(|size| Ok(format!("{}x{}", size.cols, size.rows)))),
        }
    }

    #[test]
    fn routes_overview_to_html() {
        let response = response_for_request_line("GET /overview HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert_eq!(response.body, "<html>overview</html>");
    }

    #[test]
    fn routes_data_json_to_snapshot() {
        let response = response_for_request_line("GET /data.json HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "application/json; charset=utf-8");
        assert_eq!(response.body, "{\"ok\":true}");
    }

    #[test]
    fn routes_surface_to_dynamic_renderer() {
        let response =
            response_for_request_line("GET /surface?cols=96&rows=32 HTTP/1.1", &dynamic_site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert_eq!(response.body, "96x32");
    }

    #[test]
    fn clamps_surface_size() {
        let response =
            response_for_request_line("GET /surface?cols=999&rows=2 HTTP/1.1", &dynamic_site());

        assert_eq!(response.body, "320x16");
    }

    #[test]
    fn rejects_non_get_methods() {
        let response = response_for_request_line("POST / HTTP/1.1", &site());

        assert_eq!(response.status, "405 Method Not Allowed");
    }

    #[test]
    fn head_keeps_body_for_content_length_but_marks_it_unsent() {
        let response = response_for_request_line("HEAD /overview HTTP/1.1", &site());

        assert_eq!(response.status, "200 OK");
        assert_eq!(response.body, "<html>overview</html>");
        assert!(!response.send_body);
    }
}
