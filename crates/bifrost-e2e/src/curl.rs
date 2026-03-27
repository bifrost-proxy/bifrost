use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct CurlCommand {
    proxy: Option<String>,
    url: String,
    method: Option<String>,
    headers: Vec<(String, String)>,
    data: Option<String>,
    insecure: bool,
    ca_cert: Option<PathBuf>,
    verbose: bool,
    connect_timeout: Option<u32>,
    max_time: Option<u32>,
}

#[derive(Debug)]
pub struct CurlResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub http_code: Option<u16>,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl CurlCommand {
    pub fn new(url: &str) -> Self {
        Self {
            proxy: None,
            url: url.to_string(),
            method: None,
            headers: Vec::new(),
            data: None,
            insecure: false,
            ca_cert: None,
            verbose: true,
            connect_timeout: Some(10),
            max_time: Some(30),
        }
    }

    pub fn with_proxy(proxy_url: &str, target_url: &str) -> Self {
        Self {
            proxy: Some(proxy_url.to_string()),
            url: target_url.to_string(),
            method: None,
            headers: Vec::new(),
            data: None,
            insecure: false,
            ca_cert: None,
            verbose: true,
            connect_timeout: Some(10),
            max_time: Some(30),
        }
    }

    pub fn proxy(mut self, proxy_url: &str) -> Self {
        self.proxy = Some(proxy_url.to_string());
        self
    }

    pub fn method(mut self, method: &str) -> Self {
        self.method = Some(method.to_string());
        self
    }

    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.to_string(), value.to_string()));
        self
    }

    pub fn data(mut self, data: &str) -> Self {
        self.data = Some(data.to_string());
        self
    }

    pub fn insecure(mut self) -> Self {
        self.insecure = true;
        self
    }

    pub fn ca_cert(mut self, path: PathBuf) -> Self {
        self.ca_cert = Some(path);
        self
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn connect_timeout(mut self, seconds: u32) -> Self {
        self.connect_timeout = Some(seconds);
        self
    }

    pub fn max_time(mut self, seconds: u32) -> Self {
        self.max_time = Some(seconds);
        self
    }

    fn build_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        args.push("-s".to_string());
        args.push("-S".to_string());

        if self.verbose {
            args.push("-v".to_string());
        }

        args.push("-i".to_string());

        if let Some(ref proxy) = self.proxy {
            args.push("-x".to_string());
            args.push(proxy.clone());
        }

        if let Some(ref method) = self.method {
            args.push("-X".to_string());
            args.push(method.clone());
        }

        for (key, value) in &self.headers {
            args.push("-H".to_string());
            args.push(format!("{}: {}", key, value));
        }

        if let Some(ref data) = self.data {
            args.push("-d".to_string());
            args.push(data.clone());
        }

        if self.insecure {
            args.push("-k".to_string());
        }

        if let Some(ref ca_cert) = self.ca_cert {
            args.push("--cacert".to_string());
            args.push(ca_cert.to_string_lossy().to_string());
        }

        if let Some(timeout) = self.connect_timeout {
            args.push("--connect-timeout".to_string());
            args.push(timeout.to_string());
        }

        if let Some(max_time) = self.max_time {
            args.push("-m".to_string());
            args.push(max_time.to_string());
        }

        args.push(self.url.clone());

        args
    }

    pub async fn execute(&self) -> Result<CurlResult, std::io::Error> {
        let args = self.build_args();

        let output = Command::new("curl")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let (http_code, headers, body) = parse_response(&stdout);

        Ok(CurlResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout,
            stderr,
            http_code,
            headers,
            body,
        })
    }
}

fn parse_response(response: &str) -> (Option<u16>, HashMap<String, String>, String) {
    let mut headers = HashMap::new();
    let mut http_code = None;
    let mut body = String::new();
    let mut in_headers = true;
    let mut found_status_line = false;

    for line in response.lines() {
        if in_headers {
            if line.starts_with("HTTP/") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    http_code = parts[1].parse().ok();
                }
                found_status_line = true;
            } else if found_status_line && line.trim().is_empty() {
                in_headers = false;
            } else if found_status_line && line.contains(':') {
                if let Some((key, value)) = line.split_once(':') {
                    headers.insert(key.trim().to_lowercase(), value.trim().to_string());
                }
            }
        } else {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }

    (http_code, headers, body)
}

impl CurlResult {
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
            && self
                .http_code
                .map(|c| (200..300).contains(&c))
                .unwrap_or(false)
    }

    pub fn assert_success(&self) -> Result<(), String> {
        if self.exit_code != 0 {
            return Err(format!(
                "curl failed with exit code {}: {}",
                self.exit_code, self.stderr
            ));
        }

        if let Some(code) = self.http_code {
            if (200..300).contains(&code) {
                return Ok(());
            }
            return Err(format!("HTTP status {} is not success", code));
        }

        Err("No HTTP status code in response".to_string())
    }

    pub fn assert_status(&self, expected: u16) -> Result<(), String> {
        match self.http_code {
            Some(code) if code == expected => Ok(()),
            Some(code) => Err(format!("Expected HTTP {}, got {}", expected, code)),
            None => Err("No HTTP status code in response".to_string()),
        }
    }

    pub fn assert_body_contains(&self, substring: &str) -> Result<(), String> {
        if self.body.contains(substring) {
            Ok(())
        } else {
            let preview = if self.body.len() > 200 {
                format!("{}...", &self.body[..200])
            } else {
                self.body.clone()
            };
            Err(format!(
                "Body does not contain '{}', preview: '{}'",
                substring, preview
            ))
        }
    }

    pub fn assert_header(&self, header: &str, expected: &str) -> Result<(), String> {
        let header_lower = header.to_lowercase();
        match self.headers.get(&header_lower) {
            Some(value) if value == expected => Ok(()),
            Some(value) => Err(format!(
                "Header '{}' expected '{}', got '{}'",
                header, expected, value
            )),
            None => Err(format!(
                "Header '{}' not found. Available: {:?}",
                header,
                self.headers.keys().collect::<Vec<_>>()
            )),
        }
    }

    pub fn assert_header_contains(&self, header: &str, substring: &str) -> Result<(), String> {
        let header_lower = header.to_lowercase();
        match self.headers.get(&header_lower) {
            Some(value) if value.contains(substring) => Ok(()),
            Some(value) => Err(format!(
                "Header '{}' does not contain '{}', value: '{}'",
                header, substring, value
            )),
            None => Err(format!("Header '{}' not found", header)),
        }
    }

    pub fn assert_header_missing(&self, header: &str) -> Result<(), String> {
        let header_lower = header.to_lowercase();
        if self.headers.contains_key(&header_lower) {
            Err(format!(
                "Header '{}' should not exist but found value: '{}'",
                header,
                self.headers.get(&header_lower).unwrap()
            ))
        } else {
            Ok(())
        }
    }

    pub fn get_header(&self, header: &str) -> Option<&String> {
        self.headers.get(&header.to_lowercase())
    }

    pub fn parse_json(&self) -> Result<serde_json::Value, String> {
        serde_json::from_str(&self.body).map_err(|e| format!("Failed to parse JSON: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::EnhancedMockServer;

    #[tokio::test]
    async fn test_curl_command_build_args() {
        let cmd = CurlCommand::with_proxy("http://127.0.0.1:8080", "http://example.com")
            .method("POST")
            .header("Content-Type", "application/json")
            .data(r#"{"test": true}"#)
            .insecure();

        let args = cmd.build_args();
        assert!(args.contains(&"-x".to_string()));
        assert!(args.contains(&"http://127.0.0.1:8080".to_string()));
        assert!(args.contains(&"-X".to_string()));
        assert!(args.contains(&"POST".to_string()));
        assert!(args.contains(&"-k".to_string()));
    }

    #[tokio::test]
    async fn test_curl_basic_request() {
        let mock = EnhancedMockServer::start().await;
        let result = CurlCommand::new(&mock.url("/get"))
            .verbose(false)
            .execute()
            .await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn test_parse_response() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Custom: test\r\n\r\n{\"status\": \"ok\"}";
        let (code, headers, body) = parse_response(response);

        assert_eq!(code, Some(200));
        assert_eq!(
            headers.get("content-type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(headers.get("x-custom"), Some(&"test".to_string()));
        assert!(body.contains("status"));
    }
}
