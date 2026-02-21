use crate::util::format_http_date;
use std::time::SystemTime;

#[derive(Debug)]
pub struct HttpResponse 
{
    pub status_code: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse 
{
    pub fn new(status_code: u16, status_text: &str) -> Self 
    {
        let mut resp = HttpResponse 
        {
            status_code,
            status_text: status_text.to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        resp.set_header("Date", &format_http_date(SystemTime::now()));
        resp.set_header("Server", "RustHTTP/1.0");
        resp
    }

    pub fn set_header(&mut self, name: &str, value: &str) 
    {
        if let Some(h) = self.headers.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) 
        {
            h.1 = value.to_string();
        } 
        else 
        {
            self.headers.push((name.to_string(), value.to_string()));
        }
    }

    pub fn set_body(&mut self, body: Vec<u8>, content_type: &str) 
    {
        self.set_header("Content-Type", content_type);
        self.set_header("Content-Length", &body.len().to_string());
        self.body = body;
    }

    pub fn serialize(&self) -> Vec<u8> 
    {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status_code, self.status_text);
        for (key, value) in &self.headers 
        {
            out.push_str(&format!("{}: {}\r\n", key, value));
        }
        out.push_str("\r\n");

        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }

    pub fn serialize_chunked(status_code: u16, status_text: &str, headers: &[(String, String)], body: &[u8]) -> Vec<u8> 
    {
        let mut out = format!("HTTP/1.1 {} {}\r\n", status_code, status_text);
        for (key, value) in headers 
        {
            out.push_str(&format!("{}: {}\r\n", key, value));
        }
        out.push_str("Transfer-Encoding: chunked\r\n");
        out.push_str("\r\n");

        let mut bytes = out.into_bytes();

        if !body.is_empty() 
        {
            let chunk_header = format!("{:x}\r\n", body.len());
            bytes.extend_from_slice(chunk_header.as_bytes());
            bytes.extend_from_slice(body);
            bytes.extend_from_slice(b"\r\n");
        }

        bytes.extend_from_slice(b"0\r\n\r\n");

        bytes
    }

    pub fn ok() -> Self 
    {
        Self::new(200, "OK")
    }

    pub fn not_modified() -> Self 
    {
        Self::new(304, "Not Modified")
    }

    pub fn bad_request(msg: &str) -> Self 
    {
        let mut resp = Self::new(400, "Bad Request");
        resp.set_body(
            format!("<html><body><h1>400 Bad Request</h1><p>{}</p></body></html>", msg).into_bytes(),
            "text/html",
        );
        resp
    }

    pub fn unauthorized(realm: &str) -> Self 
    {
        let mut resp = Self::new(401, "Unauthorized");
        resp.set_header("WWW-Authenticate", &format!("Basic realm=\"{}\"", realm));
        resp.set_body(
            b"<html><body><h1>401 Unauthorized</h1></body></html>".to_vec(),
            "text/html",
        );
        resp
    }

    pub fn forbidden() -> Self 
    {
        let mut resp = Self::new(403, "Forbidden");
        resp.set_body(
            b"<html><body><h1>403 Forbidden</h1></body></html>".to_vec(),
            "text/html",
        );
        resp
    }

    pub fn not_found() -> Self 
    {
        let mut resp = Self::new(404, "Not Found");
        resp.set_body(
            b"<html><body><h1>404 Not Found</h1></body></html>".to_vec(),
            "text/html",
        );
        resp
    }

    pub fn not_acceptable() -> Self 
    {
        let mut resp = Self::new(406, "Not Acceptable");
        resp.set_body(
            b"<html><body><h1>406 Not Acceptable</h1></body></html>".to_vec(),
            "text/html",
        );
        resp
    }

    pub fn internal_error(msg: &str) -> Self 
    {
        let mut resp = Self::new(500, "Internal Server Error");
        resp.set_body(
            format!(
                "<html><body><h1>500 Internal Server Error</h1><p>{}</p></body></html>",
                msg
            )
            .into_bytes(),
            "text/html",
        );
        resp
    }

    pub fn service_unavailable() -> Self 
    {
        let mut resp = Self::new(503, "Service Unavailable");
        resp.set_body(
            b"<html><body><h1>503 Service Unavailable</h1></body></html>".to_vec(),
            "text/html",
        );
        resp
    }
}


