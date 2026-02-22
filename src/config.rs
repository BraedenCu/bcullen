use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct VirtualHost 
{
    pub document_root: PathBuf,
    pub server_name: String,
}

#[derive(Debug, Clone)]
pub enum ProcessingMode 
{
    Threads(usize),
    SelectLoops(usize),
}

/*
Instead of recommended hash map, typed struct is used.
The typed struct is great because we get compile-time type
safety rather than string lookups (with hash maps). We still
follow the httpd.conf template tho.
*/
#[derive(Debug, Clone)]
pub struct ServerConfig 
{
    pub listen_port: u16,
    pub processing_mode: ProcessingMode,
    pub virtual_hosts: Vec<VirtualHost>,
}

impl ServerConfig 
{
    pub fn parse(path: &str) -> Result<ServerConfig, String> 
    {
        let config_path = Path::new(path);
        let config_dir = config_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let content = fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read config file '{}': {}", path, e))?;

        let mut listen_port: Option<u16> = None;
        let mut processing_mode: Option<ProcessingMode> = None;
        let mut virtual_hosts: Vec<VirtualHost> = Vec::new();

        let mut in_vhost = false;
        let mut current_doc_root: Option<String> = None;
        let mut current_server_name: Option<String> = None;

        for line in content.lines() 
        {
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') 
            {
                continue;
            }

            if trimmed.starts_with("<VirtualHost") 
            {
                in_vhost = true;
                current_doc_root = None;
                current_server_name = None;
                continue;
            }

            if trimmed.starts_with("</VirtualHost") 
            {
                if in_vhost 
                {
                    let doc_root = current_doc_root
                        .take()
                        .ok_or("VirtualHost missing DocumentRoot")?;
                    let server_name = current_server_name
                        .take()
                        .ok_or("VirtualHost missing ServerName")?;

                    let doc_root_path = config_dir.join(&doc_root);
                    let canonical = fs::canonicalize(&doc_root_path).map_err(|e| {
                        format!(
                            "Failed to resolve DocumentRoot '{}': {}",
                            doc_root_path.display(),
                            e
                        )
                    })?;

                    virtual_hosts.push(VirtualHost {
                        document_root: canonical,
                        server_name,
                    });
                }
                in_vhost = false;
                continue;
            }

            let parts: Vec<&str> = trimmed.splitn(2, char::is_whitespace).collect();
            if parts.len() < 2 
            {
                continue;
            }

            let key = parts[0];
            let value = parts[1].trim();

            if in_vhost 
            {
                match key 
                {
                    "DocumentRoot" => current_doc_root = Some(value.to_string()),
                    "ServerName" => current_server_name = Some(value.to_string()),
                    _ => {}
                }
            } 
            else 
            {
                match key 
                {
                    "Listen" => {
                        listen_port = Some(
                            value
                                .parse::<u16>()
                                .map_err(|_| format!("Invalid port: {}", value))?,
                        );
                    }
                    "nThreads" => {
                        processing_mode = Some(ProcessingMode::Threads(
                            value
                                .parse::<usize>()
                                .map_err(|_| format!("Invalid nThreads: {}", value))?,
                        ));
                    }
                    "nSelectLoops" => {
                        processing_mode = Some(ProcessingMode::SelectLoops(
                            value
                                .parse::<usize>()
                                .map_err(|_| format!("Invalid nSelectLoops: {}", value))?,
                        ));
                    }
                    _ => {}
                }
            }
        }

        Ok(ServerConfig 
        {
            listen_port: listen_port.ok_or("Missing 'Listen' directive")?,
            processing_mode: processing_mode
                .unwrap_or(ProcessingMode::Threads(4)),
            virtual_hosts,
        })
    }

    pub fn default_host(&self) -> &VirtualHost 
    {
        &self.virtual_hosts[0]
    }

    pub fn find_host(&self, hostname: &str) -> &VirtualHost 
    {
        self.virtual_hosts
            .iter()
            .find(|vh| vh.server_name == hostname)
            .unwrap_or(self.default_host())
    }
}
