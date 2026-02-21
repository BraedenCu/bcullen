use std::path::Path;

/*
mime (multipurpose internet mail extensions) format: type/subtype

Leveraged when serving up a static file.
*/
pub fn mime_from_extension(path: &Path) -> &'static str {
    match path
        .extension()                      // get the part after the last dot: "html", "jpg", etc.
        .and_then(|e| e.to_str())   // convert OsStr → &str (might fail on weird filenames)
        .map(|e| e.to_lowercase())  // normalize: "HTML" and "html" both work
        .as_deref()                                      // &String → &str for the match
    {
        Some("html") | Some("htm") => "text/html",
        Some("txt") => "text/plain",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("cgi") => "text/html",
        _ => "application/octet-stream",
    }
}
