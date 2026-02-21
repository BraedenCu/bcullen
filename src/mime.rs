use std::path::Path;

/*
mime (multipurpose internet mail extensions) format: type/subtype

Leveraged when serving up a static file.
*/
pub fn mime_from_extension(path: &Path) -> &'static str 
{
    match path
        .extension()                      // get the part after the last dot (jpg, html, etc)
        .and_then(|e| e.to_str())   // convert OsStr to &str 
        .map(|e| e.to_lowercase())  // normalize caps
        .as_deref()                                      // convert &String to &str
    {
        // only need to handle jpg, html, txt
        Some("html") | Some("htm") => "text/html",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("cgi") => "text/html",
        _ => "application/octet-stream", // fallback
    }
}
