use std::collections::HashMap;
use std::io::Write;
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::VirtualHost;
use crate::request::HttpRequest;
use crate::response::HttpResponse;
use crate::util::format_http_date;

/*
Executing common gateway interfact script. This is what lets our server serve more than
just static files. With CGI any req can trigger a program that will generate content 
dynamically. Our process will appear like so: 

client sents POST /test-cgi.cgi with form data
                v
router sees file is executable, routeresult::cgi
                v
execute_cgi is called
                v
server writes POST body, childs stdin. child reads env vars, 
child writes HTTP headers & body which is its stdout
                v
server reads child stdout, parses into HttpResponse
                v
server sends resp to client encoded

Some notes:
environment variables are populated from RFC 3875
*/
pub fn execute_cgi(
    script_path: &Path,
    request: &HttpRequest,
    vhost: &VirtualHost,
    peer_addr: &str,
    peer_port: u16,
    server_port: u16,
) -> HttpResponse {
    let mut environment: HashMap<String, String> = HashMap::new();

    // RFC 3875 environment variables populated
    environment.insert("REQUEST_METHOD".into(), 
        match request.method {
            crate::request::Method::Get => "GET".into(),
            crate::request::Method::Post => "POST".into(),
        }
    );
    environment.insert(
        "QUERY_STRING".into(),
        request.query_string().unwrap_or("").to_string(),
    );
    environment.insert("SERVER_NAME".into(), vhost.server_name.clone());
    environment.insert("SERVER_PORT".into(), server_port.to_string());
    environment.insert("SERVER_PROTOCOL".into(), "HTTP/1.1".into());
    environment.insert(
        "SERVER_SOFTWARE".into(),
        "RustHTTP/1.0".into(),
    );
    environment.insert("GATEWAY_INTERFACE".into(), "CGI/1.1".into());
    environment.insert("REMOTE_ADDR".into(), peer_addr.to_string());
    environment.insert("REMOTE_PORT".into(), peer_port.to_string());
    environment.insert(
        "SCRIPT_NAME".into(),
        request.path().to_string(),
    );
    if let Some(ct) = request.header("content-type") 
    {
        environment.insert("CONTENT_TYPE".into(), ct.to_string());
    }
    if !request.body.is_empty() 
    {
        environment.insert("CONTENT_LENGTH".into(), request.body.len().to_string());
    }

    let script_dir = script_path.parent().unwrap_or(Path::new("."));

    let mut child = match Command::new(script_path)
        .current_dir(script_dir)
        .env_clear()
        .envs(&environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return HttpResponse::internal_error(&format!("Failed to execute CGI: {}", e));
        }
    };

    if !request.body.is_empty() 
    {
        if let Some(mut stdin) = child.stdin.take() 
        {
            let _ = stdin.write_all(&request.body);
        }
    } 
    else 
    {
        drop(child.stdin.take());
    }

    let output = match child.wait_with_output() 
    {
        Ok(o) => o,
        Err(e) => {
            return HttpResponse::internal_error(&format!("CGI execution failed: {}", e));
        }
    };

    if !output.status.success() && output.stdout.is_empty() 
    {
        return HttpResponse::internal_error("CGI script failed");
    }

    parse_cgi_output(&output.stdout)
}


/*
Parse CGI output into an HTTP response. CGI output will be of the form
headers\r\n\r\nbody  or  headers\n\nbody
*/
fn parse_cgi_output(output: &[u8]) -> HttpResponse {
    let output_str = String::from_utf8_lossy(output);

    let (header_section, body) = if let Some(pos) = output_str.find("\r\n\r\n") 
    {
        (&output_str[..pos], &output[pos + 4..])
    } 
    else if let Some(pos) = output_str.find("\n\n") {
        (&output_str[..pos], &output[pos + 2..])
    } 
    else 
    {
        let mut resp = HttpResponse::ok();
        resp.set_body(output.to_vec(), "text/html");
        return resp;
    };

    let mut content_type = "text/html".to_string();
    let mut status_code = 200u16;
    let mut status_text = "OK".to_string();
    let mut extra_headers: Vec<(String, String)> = Vec::new();

    for line in header_section.lines() {
        if let Some((key, value)) = line.split_once(':') 
        {
            let key = key.trim();
            let value = value.trim();
            match key.to_lowercase().as_str() 
            {
                "content-type" => content_type = value.to_string(),
                "status" => {
                    let parts: Vec<&str> = value.splitn(2, ' ').collect();
                    if let Some(code) = parts.first().and_then(|c| c.parse().ok()) 
                    {
                        status_code = code;
                        status_text = parts.get(1).unwrap_or(&"OK").to_string();
                    }
                }
                _ => {
                    extra_headers.push((key.to_string(), value.to_string()));
                }
            }
        }
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    headers.push(("Date".into(), format_http_date(std::time::SystemTime::now())));
    headers.push(("Server".into(), "RustHTTP/1.0".into()));
    headers.push(("Content-Type".into(), content_type));
    headers.extend(extra_headers);

    let response_bytes = HttpResponse::serialize_chunked(status_code, &status_text, &headers, body);

    let mut resp = HttpResponse::new(status_code, &status_text);
    resp.headers.clear();
    resp.body = response_bytes;
    resp
}

/*
Send a CGI response directly to the stream (already serialized with chunked encoding).
*/
pub fn send_cgi_response(
    stream: &mut TcpStream,
    script_path: &Path,
    request: &HttpRequest,
    vhost: &VirtualHost,
    peer_addr: &str,
    peer_port: u16,
    server_port: u16,
) -> std::io::Result<()> {
    let response = execute_cgi(script_path, request, vhost, peer_addr, peer_port, server_port);

    if response.headers.is_empty() 
    {
        stream.write_all(&response.body)?;
    } 
    else 
    {
        stream.write_all(&response.serialize())?;
    }

    stream.flush()
}
