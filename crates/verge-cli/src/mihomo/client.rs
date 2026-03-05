use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

use super::types::{
    ConfigResponse, ConnectionsResponse, DelayResponse, LogEntry, ProxiesResponse, ProxyInfo,
    RulesResponse, TrafficEntry, VersionResponse,
};

pub struct MihomoClient {
    socket_path: Option<PathBuf>,
    api_addr: Option<String>,
    secret: Option<String>,
}

impl MihomoClient {
    pub fn new(
        socket_path: Option<PathBuf>,
        api_addr: Option<String>,
        secret: Option<String>,
    ) -> Result<Self> {
        if socket_path.is_none() && api_addr.is_none() {
            bail!("either socket_path or api_addr must be provided");
        }
        Ok(Self {
            socket_path,
            api_addr,
            secret,
        })
    }

    fn base_url(&self) -> String {
        if let Some(addr) = &self.api_addr {
            format!("http://{}", addr)
        } else {
            // For unix socket, the host in URL doesn't matter
            "http://localhost".to_string()
        }
    }

    /// Make a GET request and return response body as string
    async fn get(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.base_url(), path);
        self.send_request("GET", &url, None).await
    }

    /// Make a PUT request with JSON body
    async fn put(&self, path: &str, body: &str) -> Result<String> {
        let url = format!("{}{}", self.base_url(), path);
        self.send_request("PUT", &url, Some(body.to_string())).await
    }

    /// Make a PATCH request with JSON body (e.g. update running config: mode, mixed-port)
    async fn patch(&self, path: &str, body: &str) -> Result<String> {
        let url = format!("{}{}", self.base_url(), path);
        self.send_request("PATCH", &url, Some(body.to_string())).await
    }

    /// Make a POST request
    async fn post(&self, path: &str, body: Option<&str>) -> Result<String> {
        let url = format!("{}{}", self.base_url(), path);
        self.send_request("POST", &url, body.map(|s| s.to_string()))
            .await
    }

    /// Make a DELETE request
    async fn delete(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.base_url(), path);
        self.send_request("DELETE", &url, None).await
    }

    async fn send_request(
        &self,
        method: &str,
        url: &str,
        body: Option<String>,
    ) -> Result<String> {
        if let Some(socket) = &self.socket_path {
            self.send_unix_request(socket, method, url, body).await
        } else {
            self.send_tcp_request(method, url, body).await
        }
    }

    async fn send_unix_request(
        &self,
        socket_path: &Path,
        method: &str,
        url: &str,
        body: Option<String>,
    ) -> Result<String> {
        use http_body_util::{BodyExt as _, Full};
        use hyper::body::Bytes;
        use hyper_util::rt::TokioIo;

        let stream = tokio::net::UnixStream::connect(socket_path)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to mihomo socket: {}",
                    socket_path.display()
                )
            })?;

        let io = TokioIo::new(stream);

        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .context("HTTP handshake with mihomo failed")?;

        tokio::spawn(async move {
            if let Err(_e) = conn.await {}
        });

        let uri: hyper::Uri = url.parse().context("invalid URL")?;
        let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

        let body_bytes = body.unwrap_or_default();
        let mut req_builder = hyper::Request::builder()
            .method(method)
            .uri(path_and_query)
            .header("Host", "localhost")
            .header("Content-Type", "application/json");

        if let Some(secret) = &self.secret {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", secret));
        }

        let req = req_builder
            .body(Full::new(Bytes::from(body_bytes)))
            .context("failed to build request")?;

        let resp = sender
            .send_request(req)
            .await
            .context("request to mihomo failed")?;
        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("failed to read response body")?;
        let body_str =
            String::from_utf8(body.to_bytes().to_vec()).context("invalid UTF-8 in response")?;

        if !status.is_success() {
            bail!("mihomo returned {}: {}", status, body_str);
        }

        Ok(body_str)
    }

    async fn send_tcp_request(
        &self,
        method: &str,
        url: &str,
        body: Option<String>,
    ) -> Result<String> {
        let client = reqwest::Client::new();
        let mut req = match method {
            "GET" => client.get(url),
            "PUT" => client.put(url),
            "POST" => client.post(url),
            "DELETE" => client.delete(url),
            "PATCH" => client.patch(url),
            _ => bail!("unsupported HTTP method: {}", method),
        };

        req = req.header("Content-Type", "application/json");

        if let Some(secret) = &self.secret {
            req = req.header("Authorization", format!("Bearer {}", secret));
        }

        if let Some(body) = body {
            req = req.body(body);
        }

        let resp = req.send().await.context("request to mihomo failed")?;
        let status = resp.status();
        let body_str = resp.text().await.context("failed to read response body")?;

        if !status.is_success() {
            bail!("mihomo returned {}: {}", status, body_str);
        }

        Ok(body_str)
    }

    // === High-level API methods ===

    pub async fn get_version(&self) -> Result<VersionResponse> {
        let body = self.get("/version").await?;
        serde_json::from_str(&body).context("failed to parse version response")
    }

    pub async fn get_config(&self) -> Result<ConfigResponse> {
        let body = self.get("/configs").await?;
        serde_json::from_str(&body).context("failed to parse config response")
    }

    pub async fn reload_config(&self, path: &str, force: bool) -> Result<()> {
        let force_param = if force { "?force=true" } else { "" };
        let body = serde_json::json!({ "path": path }).to_string();
        self.put(&format!("/configs{}", force_param), &body).await?;
        Ok(())
    }

    pub async fn get_proxies(&self) -> Result<ProxiesResponse> {
        let body = self.get("/proxies").await?;
        serde_json::from_str(&body).context("failed to parse proxies response")
    }

    #[allow(dead_code)]
    pub async fn get_proxy(&self, name: &str) -> Result<ProxyInfo> {
        let encoded = urlencoding_encode(name);
        let body = self.get(&format!("/proxies/{}", encoded)).await?;
        serde_json::from_str(&body).context("failed to parse proxy response")
    }

    pub async fn set_proxy(&self, group: &str, node: &str) -> Result<()> {
        let encoded = urlencoding_encode(group);
        let body = serde_json::json!({ "name": node }).to_string();
        self.put(&format!("/proxies/{}", encoded), &body).await?;
        Ok(())
    }

    pub async fn get_group_delay(
        &self,
        group: &str,
        url: &str,
        timeout: u64,
    ) -> Result<serde_json::Value> {
        let encoded = urlencoding_encode(group);
        let body = self
            .get(&format!(
                "/group/{}/delay?url={}&timeout={}",
                encoded,
                urlencoding_encode(url),
                timeout
            ))
            .await?;
        serde_json::from_str(&body).context("failed to parse delay response")
    }

    pub async fn get_proxy_delay(
        &self,
        name: &str,
        url: &str,
        timeout: u64,
    ) -> Result<DelayResponse> {
        let encoded = urlencoding_encode(name);
        let body = self
            .get(&format!(
                "/proxies/{}/delay?url={}&timeout={}",
                encoded,
                urlencoding_encode(url),
                timeout
            ))
            .await?;
        serde_json::from_str(&body).context("failed to parse delay response")
    }

    pub async fn get_rules(&self) -> Result<RulesResponse> {
        let body = self.get("/rules").await?;
        serde_json::from_str(&body).context("failed to parse rules response")
    }

    pub async fn get_connections(&self) -> Result<ConnectionsResponse> {
        let body = self.get("/connections").await?;
        serde_json::from_str(&body).context("failed to parse connections response")
    }

    pub async fn close_all_connections(&self) -> Result<()> {
        self.delete("/connections").await?;
        Ok(())
    }

    pub async fn flush_dns(&self) -> Result<()> {
        self.post("/cache/dns/flush", None).await?;
        Ok(())
    }

    pub async fn set_mode(&self, mode: &str) -> Result<()> {
        let body = serde_json::json!({ "mode": mode }).to_string();
        self.patch("/configs", &body).await?;
        Ok(())
    }

    /// Get the traffic stream endpoint path.
    /// mihomo streams JSON objects line by line.
    #[allow(dead_code)]
    pub const fn traffic_stream_path() -> &'static str {
        "/traffic"
    }

    /// Read traffic stream line by line from Unix socket or TCP
    pub async fn stream_traffic(
        &self,
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<TrafficEntry>> + Send>>> {
        use futures_util::StreamExt as _;

        if let Some(socket) = &self.socket_path {
            let stream = self.stream_unix_lines(socket, "/traffic").await?;
            Ok(Box::pin(stream.map(|line| {
                let line = line?;
                serde_json::from_str(&line).context("failed to parse traffic entry")
            })))
        } else {
            let addr = self
                .api_addr
                .as_ref()
                .context("no api_addr configured")?;
            let stream = self
                .stream_tcp_lines(&format!("http://{}/traffic", addr))
                .await?;
            Ok(Box::pin(stream.map(|line| {
                let line = line?;
                serde_json::from_str(&line).context("failed to parse traffic entry")
            })))
        }
    }

    pub async fn stream_logs(
        &self,
        level: Option<&str>,
    ) -> Result<std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<LogEntry>> + Send>>> {
        use futures_util::StreamExt as _;

        let path = match level {
            Some(l) => format!("/logs?level={}", l),
            None => "/logs".to_string(),
        };

        if let Some(socket) = &self.socket_path {
            let stream = self.stream_unix_lines(socket, &path).await?;
            Ok(Box::pin(stream.map(|line| {
                let line = line?;
                serde_json::from_str(&line).context("failed to parse log entry")
            })))
        } else {
            let addr = self
                .api_addr
                .as_ref()
                .context("no api_addr configured")?;
            let stream = self
                .stream_tcp_lines(&format!("http://{}{}", addr, path))
                .await?;
            Ok(Box::pin(stream.map(|line| {
                let line = line?;
                serde_json::from_str(&line).context("failed to parse log entry")
            })))
        }
    }

    async fn stream_unix_lines(
        &self,
        socket_path: &Path,
        path: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<String>>> {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        let stream = tokio::net::UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("failed to connect to socket: {}", socket_path.display()))?;

        // Send raw HTTP request
        let secret_header = if let Some(s) = &self.secret {
            format!("Authorization: Bearer {}\r\n", s)
        } else {
            String::new()
        };
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\n{}Connection: keep-alive\r\n\r\n",
            path, secret_header
        );

        use tokio::io::AsyncWriteExt as _;
        let (reader, mut writer) = tokio::io::split(stream);
        writer
            .write_all(request.as_bytes())
            .await
            .context("failed to write request")?;

        let buf_reader = BufReader::new(reader);
        let mut lines = buf_reader.lines();

        // Skip HTTP headers (read until empty line)
        while let Some(line) = lines
            .next_line()
            .await
            .context("failed to read header")?
        {
            if line.is_empty() {
                break;
            }
        }

        // Return a stream of JSON lines
        Ok(futures_util::stream::unfold(lines, |mut lines| async move {
            match lines.next_line().await {
                Ok(Some(line)) if !line.is_empty() => Some((Ok(line), lines)),
                Ok(_) => None,
                Err(e) => Some((Err(anyhow::anyhow!("stream read error: {}", e)), lines)),
            }
        }))
    }

    async fn stream_tcp_lines(
        &self,
        url: &str,
    ) -> Result<impl futures_util::Stream<Item = Result<String>>> {
        use futures_util::StreamExt as _;

        let client = reqwest::Client::new();
        let mut req = client.get(url);
        if let Some(secret) = &self.secret {
            req = req.header("Authorization", format!("Bearer {}", secret));
        }

        let resp = req.send().await.context("request to mihomo failed")?;
        let stream = resp.bytes_stream();

        // Buffer partial lines
        Ok(stream.map(|chunk| match chunk {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).to_string();
                Ok(text)
            }
            Err(e) => Err(anyhow::anyhow!("stream error: {}", e)),
        }))
    }
}

/// Simple percent-encoding for URL path segments
/// Percent-encode a string for use in URL path/query. UTF-8 bytes are encoded per byte.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", byte));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_ascii_letters_unchanged() {
        assert_eq!(urlencoding_encode("hello"), "hello");
        assert_eq!(urlencoding_encode("HelloWorld"), "HelloWorld");
    }

    #[test]
    fn urlencoding_digits_unchanged() {
        assert_eq!(urlencoding_encode("12345"), "12345");
    }

    #[test]
    fn urlencoding_unreserved_chars_unchanged() {
        assert_eq!(urlencoding_encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn urlencoding_spaces_encoded() {
        assert_eq!(urlencoding_encode("hello world"), "hello%20world");
    }

    #[test]
    fn urlencoding_special_chars() {
        assert_eq!(urlencoding_encode("/"), "%2F");
        assert_eq!(urlencoding_encode("?"), "%3F");
        assert_eq!(urlencoding_encode("&"), "%26");
        assert_eq!(urlencoding_encode("="), "%3D");
    }

    #[test]
    fn urlencoding_chinese_chars() {
        let encoded = urlencoding_encode("香港节点");
        assert!(!encoded.contains('香'));
        assert!(encoded.contains('%'));
        // Each Chinese char is a single Unicode codepoint, encoded as %XXXX
        assert!(!encoded.is_empty());
    }

    #[test]
    fn urlencoding_mixed_content() {
        let encoded = urlencoding_encode("Proxy Group 1");
        assert_eq!(encoded, "Proxy%20Group%201");
    }

    #[test]
    fn urlencoding_empty_string() {
        assert_eq!(urlencoding_encode(""), "");
    }

    #[test]
    fn client_new_requires_socket_or_addr() {
        let result = MihomoClient::new(None, None, None);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("either socket_path or api_addr"));
    }

    #[test]
    fn client_new_with_socket() {
        let client = MihomoClient::new(Some(PathBuf::from("/tmp/test.sock")), None, None);
        assert!(client.is_ok());
    }

    #[test]
    fn client_new_with_addr() {
        let client = MihomoClient::new(None, Some("127.0.0.1:9090".to_string()), None);
        assert!(client.is_ok());
    }

    #[test]
    fn client_new_with_both() {
        let client = MihomoClient::new(
            Some(PathBuf::from("/tmp/test.sock")),
            Some("127.0.0.1:9090".to_string()),
            Some("secret".to_string()),
        );
        assert!(client.is_ok());
    }

    #[test]
    fn base_url_with_addr() {
        let client = MihomoClient::new(None, Some("127.0.0.1:9090".to_string()), None).unwrap();
        assert_eq!(client.base_url(), "http://127.0.0.1:9090");
    }

    #[test]
    fn base_url_with_socket_only() {
        let client =
            MihomoClient::new(Some(PathBuf::from("/tmp/test.sock")), None, None).unwrap();
        assert_eq!(client.base_url(), "http://localhost");
    }

    #[test]
    fn base_url_prefers_addr_when_both() {
        let client = MihomoClient::new(
            Some(PathBuf::from("/tmp/test.sock")),
            Some("10.0.0.1:9090".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(client.base_url(), "http://10.0.0.1:9090");
    }
}
