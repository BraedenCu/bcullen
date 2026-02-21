use std::io::Write;
use std::net::TcpStream;

use crate::cgi::send_cgi_response;
use crate::request::{parse_request, Method, ParseError};
use crate::response::HttpResponse;
use crate::router::{route_request, RouteResult};
use crate::server::ServerState;

pub fn handle_connection(mut stream: TcpStream, state: &ServerState) {
    let peer_addr = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let (peer_ip, peer_port) = parse_peer_addr(&peer_addr);

    loop {
        let request = match parse_request(&mut stream) {
            Ok(req) => req,
            Err(ParseError::ConnectionClosed) => break,
            Err(ParseError::Timeout) => {
                let resp = HttpResponse::bad_request("Request timeout");
                let _ = stream.write_all(&resp.serialize());
                break;
            }
            Err(_) => {
                let resp = HttpResponse::bad_request("Malformed request");
                let _ = stream.write_all(&resp.serialize());
                break;
            }
        };

        let connection_header = request
            .header("connection")
            .unwrap_or("keep-alive")
            .to_lowercase();
        let keep_alive = connection_header != "close";

        if request.path() == "/load" && request.method == Method::Get {
            let resp = if state.accepting.load(std::sync::atomic::Ordering::Relaxed)
                && !state.is_overloaded()
            {
                let mut r = HttpResponse::ok();
                r.set_body(b"OK".to_vec(), "text/plain");
                r
            } else {
                HttpResponse::service_unavailable()
            };

            let mut resp = resp;
            resp.set_header(
                "Connection",
                if keep_alive { "keep-alive" } else { "close" },
            );
            let _ = stream.write_all(&resp.serialize());
            let _ = stream.flush();

            if !keep_alive {
                break;
            }
            continue;
        }

        let hostname = request
            .header("host")
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("");
        let vhost = state.config.find_host(hostname);

        match route_request(request, vhost) {
            RouteResult::Response(mut resp) => {
                resp.set_header(
                    "Connection",
                    if keep_alive { "keep-alive" } else { "close" },
                );
                let _ = stream.write_all(&resp.serialize());
                let _ = stream.flush();
            }
            RouteResult::Cgi {
                script_path,
                request,
            } => {
                let _ = send_cgi_response(
                    &mut stream,
                    &script_path,
                    &request,
                    vhost,
                    &peer_ip,
                    peer_port,
                    state.config.listen_port,
                );
            }
        }

        if !keep_alive {
            break;
        }
    }
}

fn parse_peer_addr(addr: &str) -> (String, u16) {
    if let Some(bracket_end) = addr.rfind(']') {
        let ip = &addr[..=bracket_end];
        let port = addr[bracket_end + 1..]
            .trim_start_matches(':')
            .parse()
            .unwrap_or(0);
        (ip.to_string(), port)
    } else if let Some(colon) = addr.rfind(':') {
        let ip = &addr[..colon];
        let port = addr[colon + 1..].parse().unwrap_or(0);
        (ip.to_string(), port)
    } else {
        (addr.to_string(), 0)
    }
}
