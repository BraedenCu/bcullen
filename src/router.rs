use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::config::VirtualHost;
use crate::mime::mime_from_extension;
use crate::request::HttpRequest;
use crate::response::HttpResponse;
use crate::util::{format_http_date, parse_http_date};

pub enum RouteResult {
    Response(HttpResponse),
    Cgi {
        script_path: PathBuf,
        request: HttpRequest,
    },
}

pub fn route_request(request: HttpRequest, vhost: &VirtualHost) -> RouteResult {
    let path = request.path().to_string();

    let file_path = resolve_path(&path, &request, vhost);

    let file_path = match file_path {
        Some(p) => p,
        None => return RouteResult::Response(HttpResponse::not_found()),
    };

    let canonical = match fs::canonicalize(&file_path) {
        Ok(p) => p,
        Err(_) => return RouteResult::Response(HttpResponse::not_found()),
    };

    if !canonical.starts_with(&vhost.document_root) {
        return RouteResult::Response(HttpResponse::forbidden());
    }

    if let Some(resp) = check_auth(&canonical, &request) {
        return RouteResult::Response(resp);
    }

    if is_executable(&canonical) {
        return RouteResult::Cgi {
            script_path: canonical,
            request,
        };
    }

    serve_static_file(&canonical, &request)
}

fn resolve_path(uri_path: &str, request: &HttpRequest, vhost: &VirtualHost) -> Option<PathBuf> {
    let decoded = percent_decode(uri_path);
    let clean = decoded.trim_start_matches('/');

    if uri_path.ends_with('/') || uri_path == "/" {
        let dir = vhost.document_root.join(clean);

        // mobile detection only applies to request for /
        if uri_path == "/" && is_mobile_user_agent(request) {
            let mobile_index = dir.join("index_m.html");
            if mobile_index.is_file() {
                return Some(mobile_index);
            }
        }

        let index = dir.join("index.html");
        if index.is_file() {
            return Some(index);
        }

        return None;
    }

    let file_path = vhost.document_root.join(clean);
    if file_path.is_file() {
        Some(file_path)
    } else {
        None
    }
}

fn is_mobile_user_agent(request: &HttpRequest) -> bool {
    request
        .header("user-agent")
        .map(|ua| ua.contains("iPhone"))
        .unwrap_or(false)
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(hi), Some(lo)) = (hi, lo) {
                if let Ok(byte) = u8::from_str_radix(&format!("{}{}", hi as char, lo as char), 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            return meta.permissions().mode() & 0o111 != 0;
        }
    }
    false
}

fn serve_static_file(path: &Path, request: &HttpRequest) -> RouteResult {
    let content_type = mime_from_extension(path);

    if let Some(accept) = request.header("accept") {
        if !is_type_acceptable(accept, content_type) {
            return RouteResult::Response(HttpResponse::not_acceptable());
        }
    }

    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return RouteResult::Response(HttpResponse::not_found()),
    };

    let modified = metadata.modified().unwrap_or(SystemTime::now());

    if let Some(ims) = request.header("if-modified-since") {
        if let Some(ims_time) = parse_http_date(ims) {
            if let (Ok(file_dur), Ok(ims_dur)) = (
                modified.duration_since(SystemTime::UNIX_EPOCH),
                ims_time.duration_since(SystemTime::UNIX_EPOCH),
            ) {
                if file_dur.as_secs() <= ims_dur.as_secs() {
                    let mut resp = HttpResponse::not_modified();
                    resp.set_header("Last-Modified", &format_http_date(modified));
                    return RouteResult::Response(resp);
                }
            }
        }
    }

    let body = match fs::read(path) {
        Ok(b) => b,
        Err(_) => {
            return RouteResult::Response(HttpResponse::internal_error("Failed to read file"));
        }
    };

    let mut resp = HttpResponse::ok();
    resp.set_header("Last-Modified", &format_http_date(modified));
    resp.set_body(body, content_type);

    RouteResult::Response(resp)
}

fn is_type_acceptable(accept: &str, content_type: &str) -> bool {
    for item in accept.split(',') {
        let mime = item.trim().split(';').next().unwrap_or("").trim();
        if mime == "*/*" || mime == content_type {
            return true;
        }
        if let Some(prefix) = mime.strip_suffix("/*") {
            if content_type.starts_with(prefix) {
                return true;
            }
        }
    }
    false
}

fn check_auth(file_path: &Path, request: &HttpRequest) -> Option<HttpResponse> {
    let dir = file_path.parent()?;
    let htaccess = dir.join(".htaccess");

    if !htaccess.is_file() {
        return None;
    }

    let content = fs::read_to_string(&htaccess).ok()?;

    let mut auth_name = String::from("Restricted");
    let mut stored_user: Option<String> = None;
    let mut stored_password: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some((key, value)) = trimmed.split_once(char::is_whitespace) {
            let value = value.trim().trim_matches('"');
            match key {
                "AuthName" => auth_name = value.to_string(),
                "User" => {
                    stored_user = BASE64
                        .decode(value)
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok());
                }
                "Password" => {
                    stored_password = BASE64
                        .decode(value)
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok());
                }
                _ => {}
            }
        }
    }

    let (expected_user, expected_pass) = match (stored_user, stored_password) {
        (Some(u), Some(p)) => (u, p),
        _ => return None,
    };

    let auth_header = request.header("authorization");
    match auth_header {
        Some(auth) if auth.starts_with("Basic ") => {
            let encoded = &auth[6..];
            if let Ok(decoded_bytes) = BASE64.decode(encoded) {
                if let Ok(decoded) = String::from_utf8(decoded_bytes) {
                    if let Some((user, pass)) = decoded.split_once(':') {
                        if user == expected_user && pass == expected_pass {
                            return None;
                        }
                    }
                }
            }
            Some(HttpResponse::unauthorized(&auth_name))
        }
        _ => Some(HttpResponse::unauthorized(&auth_name)),
    }
}
