//! P-MCP Transport Layer
//!
//! Handles all transport mechanisms: stdio, HTTP, WebSocket, and custom transports

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock, broadcast};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Deserialize, Serialize};
use crate::error::{PmcpError, PmcpErrorCode};
use crate::types::{JsonRpcRequest, JsonRpcResponse, PMCP_VERSION};

/// Transport type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportType {
    Stdio,
    Http,
    WebSocket,
    Tcp,
    Custom,
}

/// Message received from transport
#[derive(Debug, Clone)]
pub struct TransportMessage {
    pub data: serde_json::Value,
    pub sender: Option<String>,
    pub timestamp: f64,
}

/// Transport trait for custom implementations
pub trait Transport: Send + Sync {
    fn transport_type(&self) -> TransportType;
    fn send(&self, data: serde_json::Value) -> impl std::future::Future<Output = Result<(), PmcpError>> + '_;
    fn close(&self) -> impl std::future::Future<Output = ()> + '_;
}

/// Stdio Transport - for Claude Desktop integration
pub struct StdioTransport {
    reader: Option<mpsc::Receiver<String>>,
    writer: mpsc::Sender<String>,
    closed: Arc<RwLock<bool>>,
}

impl StdioTransport {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self {
            reader: Some(rx),
            writer: tx,
            closed: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn read_message(&mut self) -> Option<serde_json::Value> {
        if let Some(rx) = &mut self.reader {
            rx.recv().await.and_then(|s| serde_json::from_str(&s).ok())
        } else {
            None
        }
    }

    pub async fn write_message(&self, msg: serde_json::Value) -> Result<(), PmcpError> {
        if *self.closed.read().await {
            return Err(PmcpError::internal_error("Transport closed"));
        }
        let data = serde_json::to_string(&msg).map_err(|e| PmcpError::internal_error(e.to_string()))?;
        self.writer.send(data).await.map_err(|e| PmcpError::internal_error(e.to_string()))?;
        Ok(())
    }

    pub async fn close(&self) {
        *self.closed.write().await = true;
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// HTTP Transport - for web-based clients
pub struct HttpTransport {
    host: String,
    port: u16,
    prefix: String,
    closed: Arc<RwLock<bool>>,
    event_sender: broadcast::Sender<TransportMessage>,
}

impl HttpTransport {
    pub fn new(host: impl Into<String>, port: u16, prefix: impl Into<String>) -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            host: host.into(),
            port,
            prefix: prefix.into(),
            closed: Arc::new(RwLock::new(false)),
            event_sender: tx,
        }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TransportMessage> {
        self.event_sender.subscribe()
    }

    pub async fn broadcast(&self, msg: TransportMessage) {
        let _ = self.event_sender.send(msg);
    }

    pub async fn close(&self) {
        *self.closed.write().await = true;
    }

    pub async fn start_server(&self) -> Result<(), PmcpError> {
        let addr = format!("{}:{}", self.host, self.port);
        let listener = TcpListener::bind(&addr).await.map_err(|e| PmcpError::internal_error(e.to_string()))?;
        
        tracing::info!("[HTTP Transport] Listening on http://{}/pmcp", addr);

        loop {
            if *self.closed.read().await {
                break;
            }

            match listener.accept().await {
                Ok((stream, addr)) => {
                    let closed = self.closed.clone();
                    let prefix = self.prefix.clone();
                    tokio::spawn(async move {
                        let _ = Self::handle_connection(stream, closed, &prefix).await;
                    });
                }
                Err(e) => {
                    tracing::error!("[HTTP Transport] Accept error: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_connection(mut stream: TcpStream, closed: Arc<RwLock<bool>>, prefix: &str) -> Result<(), PmcpError> {
        let mut buffer = [0u8; 8192];
        
        loop {
            if *closed.read().await {
                break;
            }

            match stream.read(&mut buffer).await {
                Ok(0) => break, // Connection closed
                Ok(n) => {
                    let request = String::from_utf8_lossy(&buffer[..n]);
                    
                    // Simple HTTP parsing (not production-ready)
                    if request.contains("POST") && request.contains(prefix) {
                        // Extract JSON body
                        if let Some(body_start) = request.find("\r\n\r\n") {
                            let body = &request[body_start + 4..];
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
                                // Handle the request (would call server)
                                let response = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": json.get("id"),
                                    "result": {}
                                });
                                
                                let resp_str = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                    serde_json::to_string(&response).unwrap_or_default().len(),
                                    serde_json::to_string(&response).unwrap_or_default()
                                );
                                
                                stream.write_all(resp_str.as_bytes()).await.ok();
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("[HTTP Transport] Read error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }
}

/// WebSocket Transport - for real-time clients
pub struct WebSocketTransport {
    connections: Arc<RwLock<HashMap<String, mpsc::Sender<serde_json::Value>>>>,
    closed: Arc<RwLock<bool>>,
    event_sender: broadcast::Sender<TransportMessage>,
}

impl WebSocketTransport {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            closed: Arc::new(RwLock::new(false)),
            event_sender: tx,
        }
    }

    pub async fn add_client(&self, id: impl Into<String>, sender: mpsc::Sender<serde_json::Value>) {
        self.connections.write().await.insert(id.into(), sender);
    }

    pub async fn remove_client(&self, id: &str) {
        self.connections.write().await.remove(id);
    }

    pub async fn broadcast(&self, msg: TransportMessage) {
        let _ = self.event_sender.send(msg);
    }

    pub async fn send_to(&self, client_id: &str, msg: serde_json::Value) -> Result<(), PmcpError> {
        let connections = self.connections.read().await;
        if let Some(sender) = connections.get(client_id) {
            sender.send(msg).await.map_err(|e| PmcpError::internal_error(e.to_string()))?;
            Ok(())
        } else {
            Err(PmcpError::invalid_params(format!("Client not found: {}", client_id)))
        }
    }

    pub async fn close(&self) {
        *self.closed.write().await = true;
        self.connections.write().await.clear();
    }
}

impl Default for WebSocketTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// TCP Transport - for machine-to-machine communication
pub struct TcpTransport {
    connections: Arc<RwLock<HashMap<String, TcpStream>>>,
    closed: Arc<RwLock<bool>>,
}

impl TcpTransport {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            closed: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn connect(&self, addr: &str) -> Result<String, PmcpError> {
        let stream = TcpStream::connect(addr).await.map_err(|e| PmcpError::internal_error(e.to_string()))?;
        let id = format!("tcp-{}", uuid::Uuid::new_v4());
        self.connections.write().await.insert(id.clone(), stream);
        Ok(id)
    }

    pub async fn send(&self, client_id: &str, data: serde_json::Value) -> Result<(), PmcpError> {
        // tokio::net::TcpStream has no try_clone() (that's the std/blocking
        // API); take a write lock and use get_mut() for direct &mut access
        // instead of attempting to clone the stream.
        let mut connections = self.connections.write().await;
        if let Some(stream) = connections.get_mut(client_id) {
            let bytes = serde_json::to_vec(&data).map_err(|e| PmcpError::internal_error(e.to_string()))?;
            
            // Frame: 4-byte length + JSON
            let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
            frame.extend(bytes);
            
            stream.write_all(&frame).await.map_err(|e| PmcpError::internal_error(e.to_string()))?;
            Ok(())
        } else {
            Err(PmcpError::invalid_params(format!("Connection not found: {}", client_id)))
        }
    }

    pub async fn receive(&self, client_id: &str) -> Result<Option<serde_json::Value>, PmcpError> {
        let mut connections = self.connections.write().await;
        if let Some(stream) = connections.get_mut(client_id) {
            let mut length_buf = [0u8; 4];
            match stream.read_exact(&mut length_buf).await {
                Ok(_) => {
                    let length = u32::from_be_bytes(length_buf) as usize;
                    let mut data_buf = vec![0u8; length];
                    stream.read_exact(&mut data_buf).await.map_err(|e| PmcpError::internal_error(e.to_string()))?;
                    Ok(serde_json::from_slice(&data_buf).ok())
                }
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    pub async fn close(&self) {
        *self.closed.write().await = true;
    }
}

impl Default for TcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// Transport Manager - coordinates multiple transport types
pub struct TransportManager {
    stdio: Option<StdioTransport>,
    http: Option<HttpTransport>,
    websocket: Option<WebSocketTransport>,
    tcp: Option<TcpTransport>,
    message_sender: mpsc::Sender<TransportMessage>,
}

impl TransportManager {
    pub fn new() -> Self {
        let (tx, _) = mpsc::channel(1000);
        Self {
            stdio: None,
            http: None,
            websocket: None,
            tcp: None,
            message_sender: tx,
        }
    }

    pub fn with_stdio(mut self) -> Self {
        self.stdio = Some(StdioTransport::new());
        self
    }

    pub fn with_http(mut self, host: &str, port: u16) -> Self {
        self.http = Some(HttpTransport::new(host, port, "/pmcp"));
        self
    }

    pub fn with_websocket(mut self) -> Self {
        self.websocket = Some(WebSocketTransport::new());
        self
    }

    pub fn with_tcp(mut self) -> Self {
        self.tcp = Some(TcpTransport::new());
        self
    }

    pub async fn start_stdio(&mut self) -> Result<(), PmcpError> {
        if let Some(stdio) = &self.stdio {
            tracing::info!("[Transport Manager] Stdio transport ready");
            Ok(())
        } else {
            Err(PmcpError::invalid_params("Stdio transport not configured"))
        }
    }

    pub async fn start_http(&mut self) -> Result<(), PmcpError> {
        if let Some(http) = &self.http {
            http.start_server().await
        } else {
            Err(PmcpError::invalid_params("HTTP transport not configured"))
        }
    }

    pub fn get_stdio(&self) -> Option<&StdioTransport> {
        self.stdio.as_ref()
    }

    pub fn get_http(&self) -> Option<&HttpTransport> {
        self.http.as_ref()
    }

    pub fn get_websocket(&self) -> Option<&WebSocketTransport> {
        self.websocket.as_ref()
    }

    pub fn get_tcp(&self) -> Option<&TcpTransport> {
        self.tcp.as_ref()
    }

    pub async fn shutdown(&self) {
        if let Some(stdio) = &self.stdio {
            stdio.close().await;
        }
        if let Some(http) = &self.http {
            http.close().await;
        }
        if let Some(ws) = &self.websocket {
            ws.close().await;
        }
        if let Some(tcp) = &self.tcp {
            tcp.close().await;
        }
        tracing::info!("[Transport Manager] All transports shut down");
    }
}

impl Default for TransportManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Transport Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    pub transport_type: TransportType,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub path: Option<String>,
    pub tls_enabled: Option<bool>,
    pub tls_cert_path: Option<String>,
    pub tls_key_path: Option<String>,
    pub max_connections: Option<usize>,
    pub timeout_ms: Option<u64>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            transport_type: TransportType::Stdio,
            host: None,
            port: None,
            path: None,
            tls_enabled: None,
            tls_cert_path: None,
            tls_key_path: None,
            max_connections: Some(100),
            timeout_ms: Some(30000),
        }
    }
}

impl TransportConfig {
    pub fn stdio() -> Self {
        Self {
            transport_type: TransportType::Stdio,
            ..Default::default()
        }
    }

    pub fn http(host: &str, port: u16) -> Self {
        Self {
            transport_type: TransportType::Http,
            host: Some(host.to_string()),
            port: Some(port),
            ..Default::default()
        }
    }

    pub fn websocket(host: &str, port: u16) -> Self {
        Self {
            transport_type: TransportType::WebSocket,
            host: Some(host.to_string()),
            port: Some(port),
            ..Default::default()
        }
    }

    pub fn tcp(host: &str, port: u16) -> Self {
        Self {
            transport_type: TransportType::Tcp,
            host: Some(host.to_string()),
            port: Some(port),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_config_stdio() {
        let config = TransportConfig::stdio();
        assert_eq!(config.transport_type, TransportType::Stdio);
    }

    #[test]
    fn test_transport_config_http() {
        let config = TransportConfig::http("localhost", 8080);
        assert_eq!(config.transport_type, TransportType::Http);
        assert_eq!(config.host, Some("localhost".to_string()));
        assert_eq!(config.port, Some(8080));
    }

    #[test]
    fn test_transport_manager() {
        let manager = TransportManager::new()
            .with_stdio()
            .with_http("0.0.0.0", 8080)
            .with_websocket()
            .with_tcp();
        
        assert!(manager.get_stdio().is_some());
        assert!(manager.get_http().is_some());
        assert!(manager.get_websocket().is_some());
        assert!(manager.get_tcp().is_some());
    }
}