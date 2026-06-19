use crate::config::BackendTransport;
use anyhow::Result;
use hyper_util::rt::TokioIo;

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event_type: String,
    pub data: String,
}

#[derive(Clone)]
pub struct BackendClient {
    transport: BackendTransport,
}

impl BackendClient {
    pub fn new(transport: BackendTransport) -> Self {
        Self { transport }
    }

    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let body = self.request("GET", path, None).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Result<(u16, serde_json::Value)> {
        let req_body = serde_json::to_vec(body)?;
        let resp_body = self.request_with_status("POST", path, Some(req_body)).await?;
        Ok(resp_body)
    }

    pub async fn delete(&self, path: &str) -> Result<(u16, serde_json::Value)> {
        self.request_with_status("DELETE", path, None).await
    }

    pub async fn get_text(&self, path: &str) -> Result<String> {
        use http_body_util::BodyExt;

        let req = hyper::Request::builder()
            .method("GET")
            .uri(path)
            .header("host", "localhost")
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))?;

        let resp = match &self.transport {
            BackendTransport::UnixSocket(socket_path) => {
                let stream = tokio::net::UnixStream::connect(socket_path).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
                tokio::spawn(async move { let _ = conn.await; });
                sender.send_request(req).await?
            }
            BackendTransport::Tcp(url) => {
                let parsed: hyper::Uri = url.parse()?;
                let host = parsed.host().unwrap_or("localhost");
                let port = parsed.port_u16().unwrap_or(4002);
                let stream = tokio::net::TcpStream::connect(format!("{}:{}", host, port)).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
                tokio::spawn(async move { let _ = conn.await; });
                sender.send_request(req).await?
            }
        };

        let body_bytes = resp.into_body().collect().await?.to_bytes();
        Ok(String::from_utf8_lossy(&body_bytes).to_string())
    }

    pub async fn sse_stream(&self, path: &str) -> Result<tokio::sync::mpsc::Receiver<SseEvent>> {
        use http_body_util::BodyExt;

        let req = hyper::Request::builder()
            .method("GET")
            .uri(path)
            .header("host", "localhost")
            .header("accept", "text/event-stream")
            .body(http_body_util::Empty::<hyper::body::Bytes>::new())?;

        let resp = match &self.transport {
            BackendTransport::UnixSocket(socket_path) => {
                let stream = tokio::net::UnixStream::connect(socket_path).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
                tokio::spawn(async move { let _ = conn.await; });
                sender.send_request(req).await?
            }
            BackendTransport::Tcp(url) => {
                let parsed: hyper::Uri = url.parse()?;
                let host = parsed.host().unwrap_or("localhost");
                let port = parsed.port_u16().unwrap_or(4002);
                let stream = tokio::net::TcpStream::connect(format!("{}:{}", host, port)).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
                tokio::spawn(async move { let _ = conn.await; });
                sender.send_request(req).await?
            }
        };

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let mut body = resp.into_body();

        tokio::spawn(async move {
            let mut buffer = String::new();
            while let Some(frame) = body.frame().await {
                if let Ok(frame) = frame {
                    if let Some(data) = frame.data_ref() {
                        buffer.push_str(&String::from_utf8_lossy(data));
                        // Parse SSE events from buffer
                        while let Some(pos) = buffer.find("\n\n") {
                            let event_text = buffer[..pos].to_string();
                            buffer = buffer[pos + 2..].to_string();

                            let mut event_type = "message".to_string();
                            let mut data = String::new();
                            for line in event_text.lines() {
                                if let Some(val) = line.strip_prefix("event: ") {
                                    event_type = val.to_string();
                                } else if let Some(val) = line.strip_prefix("data: ") {
                                    data = val.to_string();
                                }
                            }
                            if tx.send(SseEvent { event_type, data }).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn request(&self, method: &str, path: &str, body: Option<Vec<u8>>) -> Result<Vec<u8>> {
        let (_, resp) = self.request_with_status(method, path, body).await?;
        Ok(serde_json::to_vec(&resp)?)
    }

    async fn request_with_status(&self, method: &str, path: &str, body: Option<Vec<u8>>) -> Result<(u16, serde_json::Value)> {
        use http_body_util::BodyExt;

        let req_body = match &body {
            Some(data) => http_body_util::Full::new(hyper::body::Bytes::from(data.clone())),
            None => http_body_util::Full::new(hyper::body::Bytes::new()),
        };

        let req = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .body(req_body)?;

        let resp = match &self.transport {
            BackendTransport::UnixSocket(socket_path) => {
                let stream = tokio::net::UnixStream::connect(socket_path).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
                tokio::spawn(async move { let _ = conn.await; });
                sender.send_request(req).await?
            }
            BackendTransport::Tcp(url) => {
                let parsed: hyper::Uri = url.parse()?;
                let host = parsed.host().unwrap_or("localhost");
                let port = parsed.port_u16().unwrap_or(4002);
                let stream = tokio::net::TcpStream::connect(format!("{}:{}", host, port)).await?;
                let io = TokioIo::new(stream);
                let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
                tokio::spawn(async move { let _ = conn.await; });
                sender.send_request(req).await?
            }
        };

        let status = resp.status().as_u16();
        let body_bytes = resp.into_body().collect().await?.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);

        Ok((status, json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_proxy_unix_socket_get_health() {
        // Start a mock backend on a unix socket
        let store = switch_configurator::config::ConfigStore::new(
            switch_configurator::config::AppConfig { switches: vec![] },
            0,
        );
        let app = switch_configurator::api::create_router(store);

        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("proxy-test.sock");
        let uds = tokio::net::UnixListener::bind(&socket_path).unwrap();
        tokio::spawn(async move {
            switch_configurator::api::server::serve_unix_socket(uds, app).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = BackendClient::new(BackendTransport::UnixSocket(socket_path));
        let result = client.get("/health").await;

        assert!(result.is_ok(), "GET /health should succeed: {:?}", result.err());
        let json = result.unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_proxy_unix_socket_get_switches() {
        let store = switch_configurator::config::ConfigStore::new(
            switch_configurator::config::AppConfig { switches: vec![] },
            0,
        );
        let app = switch_configurator::api::create_router(store);

        let socket_dir = tempfile::tempdir().unwrap();
        let socket_path = socket_dir.path().join("proxy-test2.sock");
        let uds = tokio::net::UnixListener::bind(&socket_path).unwrap();
        tokio::spawn(async move {
            switch_configurator::api::server::serve_unix_socket(uds, app).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let client = BackendClient::new(BackendTransport::UnixSocket(socket_path));
        let result = client.get("/switches").await;

        assert!(result.is_ok(), "GET /switches should succeed: {:?}", result.err());
        let json = result.unwrap();
        assert!(json["switches"].is_array(), "Should return switches array");
    }
}
