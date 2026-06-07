use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;

#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Debug)]
pub struct HttpRequest {
    pub method: Method, // get & post
    pub uri: String, // unique string of characters used to identify a specific abstract resource on the network
    // Parsed and validated (must start with "HTTP/") but not yet consumed downstream;
    // retained as part of the request model. Matches the targeted allows below.
    #[allow(dead_code)]
    pub version: String,
    pub headers: HashMap<String, String>, // headers
    pub body: Vec<u8>,                    // msg body
}

impl HttpRequest {
    pub fn path(&self) -> &str {
        self.uri.split('?').next().unwrap_or(&self.uri)
    }

    pub fn query_string(&self) -> Option<&str> {
        self.uri.split_once('?').map(|(_, q)| q)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(|s| s.as_str())
    }
}

#[derive(Debug)]
pub enum ParseError {
    InvalidRequestLine,
    InvalidMethod,
    InvalidVersion,
    #[allow(dead_code)]
    MissingHostHeader,
    #[allow(dead_code)]
    IoError(std::io::Error),
    ConnectionClosed,
    Timeout,
    Incomplete,
}

impl From<std::io::Error> for ParseError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
            ParseError::Timeout
        } else {
            ParseError::IoError(e)
        }
    }
}

/*
Parsing HTTP request from the TCP stream. We are grabbing a raw TCP socket and
producing a structured HttpRequest. We define the specific format according to
HTTP/1.1 specification. Every HTTP request will look like the following:

GET /index.html HTTP/1.1\r\n       <- request line
Host: host1.cs.yale.edu\r\n        <- header
Accept: text/html\r\n              <- header
Connection: keep-alive\r\n         <- header
\r\n                               <- terminal line signalling end of header
<body bytes>                       <- optional body (POST only)

We want our func to read each of these in order. Note that the ? operator we
leverge extensively corresponds to:

match result {
    Ok(v) => v,
    Err(e) => return Err(e.into()),  // converts io::Error → ParseError via the From impl
}
*/
pub fn parse_request(stream: &mut TcpStream) -> Result<HttpRequest, ParseError> {
    let mut reader = BufReader::new(stream.try_clone().map_err(ParseError::IoError)?);
    parse_request_from_reader(&mut reader)
}

pub(crate) fn parse_request_from_reader<R: BufRead + Read>(
    reader: &mut R,
) -> Result<HttpRequest, ParseError> {
    let mut request_line = String::new();
    let n = reader.read_line(&mut request_line)?;
    if n == 0 {
        return Err(ParseError::ConnectionClosed);
    }

    let request_line = request_line.trim_end_matches(['\r', '\n']);
    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return Err(ParseError::InvalidRequestLine);
    }

    let method = match parts[0] {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => return Err(ParseError::InvalidMethod),
    };

    let uri = parts[1].to_string();
    let version = parts[2].to_string();

    if !version.starts_with("HTTP/") {
        return Err(ParseError::InvalidVersion);
    }

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_lowercase(), value.trim().to_string());
        }
    }

    let mut body = Vec::new();
    if method == Method::Post {
        if let Some(len_str) = headers.get("content-length") {
            if let Ok(len) = len_str.parse::<usize>() {
                body.resize(len, 0);
                reader.read_exact(&mut body)?;
            }
        }
    }

    Ok(HttpRequest {
        method,
        uri,
        version,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(raw: &[u8]) -> Result<HttpRequest, ParseError> {
        let mut reader = Cursor::new(raw);
        parse_request_from_reader(&mut reader)
    }

    #[test]
    fn parses_get_request_line_headers_path_and_query() {
        let request = parse(
            b"GET /search?q=rust HTTP/1.1\r\nHost: example.test\r\nAccept: text/html\r\n\r\n",
        )
        .expect("request should parse");

        assert_eq!(request.method, Method::Get);
        assert_eq!(request.uri, "/search?q=rust");
        assert_eq!(request.version, "HTTP/1.1");
        assert_eq!(request.path(), "/search");
        assert_eq!(request.query_string(), Some("q=rust"));
        assert_eq!(request.header("HOST"), Some("example.test"));
        assert_eq!(request.header("accept"), Some("text/html"));
    }

    #[test]
    fn parses_post_body_from_content_length() {
        let request = parse(
            b"POST /submit HTTP/1.1\r\nHost: example.test\r\nContent-Length: 11\r\n\r\nhello=world",
        )
        .expect("request should parse");

        assert_eq!(request.method, Method::Post);
        assert_eq!(request.body, b"hello=world");
    }

    #[test]
    fn rejects_malformed_request_line() {
        let err = parse(b"GET /only-two-parts\r\n\r\n").unwrap_err();
        assert!(matches!(err, ParseError::InvalidRequestLine));
    }

    #[test]
    fn rejects_unknown_method() {
        let err = parse(b"PUT / HTTP/1.1\r\nHost: example.test\r\n\r\n").unwrap_err();
        assert!(matches!(err, ParseError::InvalidMethod));
    }

    #[test]
    fn rejects_invalid_http_version() {
        let err = parse(b"GET / NOTHTTP/1.1\r\nHost: example.test\r\n\r\n").unwrap_err();
        assert!(matches!(err, ParseError::InvalidVersion));
    }
}
