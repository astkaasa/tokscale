use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use anyhow::Result;

#[derive(Clone, Debug)]
pub(crate) struct StaticSite {
    pub html: String,
    pub json: String,
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

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
    let body = response.body.as_bytes();
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:\r\nConnection: close\r\n\r\n",
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
