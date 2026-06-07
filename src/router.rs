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
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = bytes[i + 1] as char;
            let lo = bytes[i + 2] as char;
            if let Ok(byte) = u8::from_str_radix(&format!("{}{}", hi, lo), 16) {
                result.push(byte as char);
                i += 3;
                continue;
            }
        }

        result.push(bytes[i] as char);
        i += 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Method;
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("bcullen-router-test-{}-{}", name, nonce))
    }

    fn request(uri: &str) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            uri: uri.to_string(),
            version: "HTTP/1.1".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    fn request_with_header(uri: &str, name: &str, value: &str) -> HttpRequest {
        let mut req = request(uri);
        req.headers.insert(name.to_lowercase(), value.to_string());
        req
    }

    fn vhost(root: &Path) -> VirtualHost {
        VirtualHost {
            document_root: fs::canonicalize(root).unwrap(),
            server_name: "example.test".to_string(),
        }
    }

    fn response_status(result: RouteResult) -> u16 {
        match result {
            RouteResult::Response(resp) => resp.status_code,
            RouteResult::Cgi { .. } => panic!("expected static response"),
        }
    }

    #[test]
    fn resolves_directory_to_index_html() {
        let dir = temp_dir("index");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.html"), "home").unwrap();

        let result = route_request(request("/"), &vhost(&dir));

        match result {
            RouteResult::Response(resp) => {
                assert_eq!(resp.status_code, 200);
                assert_eq!(resp.body, b"home");
            }
            RouteResult::Cgi { .. } => panic!("expected static response"),
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn percent_decodes_file_paths() {
        let dir = temp_dir("percent");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("space file.txt"), "decoded").unwrap();

        let result = route_request(request("/space%20file.txt"), &vhost(&dir));

        match result {
            RouteResult::Response(resp) => {
                assert_eq!(resp.status_code, 200);
                assert_eq!(resp.body, b"decoded");
            }
            RouteResult::Cgi { .. } => panic!("expected static response"),
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn serves_mobile_index_for_iphone_user_agent() {
        let dir = temp_dir("mobile");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.html"), "desktop").unwrap();
        fs::write(dir.join("index_m.html"), "mobile").unwrap();

        let result = route_request(
            request_with_header("/", "User-Agent", "Mozilla iPhone"),
            &vhost(&dir),
        );

        match result {
            RouteResult::Response(resp) => {
                assert_eq!(resp.status_code, 200);
                assert_eq!(resp.body, b"mobile");
            }
            RouteResult::Cgi { .. } => panic!("expected static response"),
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_path_traversal_outside_document_root() {
        let dir = temp_dir("traversal");
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(dir.join("secret.txt"), "secret").unwrap();

        let result = route_request(request("/../secret.txt"), &vhost(&root));

        assert_eq!(response_status(result), 403);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn returns_not_found_for_missing_file() {
        let dir = temp_dir("missing");
        fs::create_dir_all(&dir).unwrap();

        let result = route_request(request("/missing.txt"), &vhost(&dir));

        assert_eq!(response_status(result), 404);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_unacceptable_accept_header() {
        let dir = temp_dir("accept");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.html"), "home").unwrap();

        let result = route_request(
            request_with_header("/index.html", "Accept", "image/png"),
            &vhost(&dir),
        );

        assert_eq!(response_status(result), 406);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn percent_decode_preserves_invalid_escape() {
        assert_eq!(percent_decode("/bad%zzescape"), "/bad%zzescape");
    }
}
