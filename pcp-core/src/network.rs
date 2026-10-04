//! PCP Network Transport Layer
//!
//! Supports stdio, HTTP/SSE, WebSocket, TCP, QUIC transports.
//! Each transport implements the `Transport` trait for plug-and-play use.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, error, info};

use crate::error::{PcpError, PcpErrorCode};

// ─────────────────────────────────────────────────────────────────────────────
//  TRANSPORT TRAIT
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Start the transport and return (sender, receiver) channels.
    async fn connect(
        &self,
    ) -> Result<
        (
            mpsc::Sender<Value>,    // outbound → peer
            mpsc::Receiver<Value>,  // inbound ← peer
        ),
        PcpError,
    >;
    fn name(&self) -> &'static str;
}

// ─────────────────────────────────────────────────────────────────────────────
//  STDIO TRANSPORT
// ─────────────────────────────────────────────────────────────────────────────

pub struct StdioTransport;

#[async_trait]
impl Transport for StdioTransport {
    async fn connect(&self) -> Result<(mpsc::Sender<Value>, mpsc::Receiver<Value>), PcpError> {
        let (out_tx, mut out_rx) = mpsc::channel::<Value>(256);
        let (in_tx, in_rx) = mpsc::channel::<Value>(256);

        // Writer task: in_rx → stdout
        tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(msg) = out_rx.recv().await {
                let mut line = serde_json::to_string(&msg).unwrap_or_default();
                line.push('\n');
                if let Err(e) = stdout.write_all(line.as_bytes()).await {
                    error!("stdio write error: {}", e);
                    break;
                }
                let _ = stdout.flush().await;
            }
        });

        // Reader task: stdin → in_tx
        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut reader = BufReader::new(stdin);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<Value>(trimmed) {
                            Ok(msg) => {
                                if in_tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("stdio parse error: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("stdio read error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok((out_tx, in_rx))
    }

    fn name(&self) -> &'static str { "stdio" }
}

// ─────────────────────────────────────────────────────────────────────────────
//  TCP TRANSPORT
// ─────────────────────────────────────────────────────────────────────────────

pub struct TcpServerTransport {
    pub addr: SocketAddr,
}

impl TcpServerTransport {
    pub fn new(addr: impl Into<SocketAddr>) -> Self {
        Self { addr: addr.into() }
    }
}

#[async_trait]
impl Transport for TcpServerTransport {
    async fn connect(&self) -> Result<(mpsc::Sender<Value>, mpsc::Receiver<Value>), PcpError> {
        let listener = TcpListener::bind(self.addr).await
            .map_err(|e| PcpError::with_message(PcpErrorCode::InternalError, e.to_string()))?;

        info!("TCP transport listening on {}", self.addr);

        let (out_tx, mut out_rx) = mpsc::channel::<Value>(256);
        let (in_tx, in_rx) = mpsc::channel::<Value>(256);

        tokio::spawn(async move {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(c) => c,
                Err(e) => { error!("accept: {}", e); return; }
            };
            info!("TCP client connected from {}", peer_addr);
            handle_tcp_stream(stream, out_rx, in_tx).await;
        });

        Ok((out_tx, in_rx))
    }

    fn name(&self) -> &'static str { "tcp" }
}

pub struct TcpClientTransport {
    pub addr: SocketAddr,
}

impl TcpClientTransport {
    pub fn new(addr: impl Into<SocketAddr>) -> Self {
        Self { addr: addr.into() }
    }
}

#[async_trait]
impl Transport for TcpClientTransport {
    async fn connect(&self) -> Result<(mpsc::Sender<Value>, mpsc::Receiver<Value>), PcpError> {
        let stream = TcpStream::connect(self.addr).await
            .map_err(|e| PcpError::with_message(PcpErrorCode::InternalError, e.to_string()))?;

        info!("TCP connected to {}", self.addr);

        let (out_tx, out_rx) = mpsc::channel::<Value>(256);
        let (in_tx, in_rx) = mpsc::channel::<Value>(256);

        tokio::spawn(handle_tcp_stream(stream, out_rx, in_tx));

        Ok((out_tx, in_rx))
    }

    fn name(&self) -> &'static str { "tcp-client" }
}

async fn handle_tcp_stream(
    stream: TcpStream,
    mut out_rx: mpsc::Receiver<Value>,
    in_tx: mpsc::Sender<Value>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    // Writer task
    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let mut s = serde_json::to_string(&msg).unwrap_or_default();
            s.push('\n');
            if writer.write_all(s.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    // Reader loop
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let t = line.trim();
                if t.is_empty() { continue; }
                if let Ok(msg) = serde_json::from_str::<Value>(t) {
                    if in_tx.send(msg).await.is_err() { break; }
                }
            }
            Err(_) => break,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  WEBSOCKET TRANSPORT
// ─────────────────────────────────────────────────────────────────────────────

pub struct WebSocketTransport {
    pub addr: SocketAddr,
    pub path: String,
}

impl WebSocketTransport {
    pub fn new(addr: impl Into<SocketAddr>, path: impl Into<String>) -> Self {
        Self { addr: addr.into(), path: path.into() }
    }
}

// Full WebSocket implementation using tokio-tungstenite would go here;
// for now, delegate to TCP with a WS upgrade handshake stub.
#[async_trait]
impl Transport for WebSocketTransport {
    async fn connect(&self) -> Result<(mpsc::Sender<Value>, mpsc::Receiver<Value>), PcpError> {
        // Use underlying TCP for now; real impl would do the HTTP upgrade.
        TcpServerTransport::new(self.addr).connect().await
    }

    fn name(&self) -> &'static str { "websocket" }
}

// ─────────────────────────────────────────────────────────────────────────────
//  MULTIPLEXED TRANSPORT  (fan-in/fan-out for fleet scenarios)
// ─────────────────────────────────────────────────────────────────────────────

pub struct MultiplexedTransport {
    transports: Vec<Box<dyn Transport>>,
}

impl MultiplexedTransport {
    pub fn new(transports: Vec<Box<dyn Transport>>) -> Self {
        Self { transports }
    }
}

#[async_trait]
impl Transport for MultiplexedTransport {
    async fn connect(&self) -> Result<(mpsc::Sender<Value>, mpsc::Receiver<Value>), PcpError> {
        let (out_tx, out_rx) = mpsc::channel::<Value>(1024);
        let (in_tx, in_rx) = mpsc::channel::<Value>(1024);

        let (broadcast_tx, _) = broadcast::channel::<Value>(1024);

        // Connect each transport and wire up fan-in / fan-out
        for transport in &self.transports {
            let (t_out, mut t_in) = transport.connect().await?;
            let bc_rx = broadcast_tx.subscribe();
            let in_tx_clone = in_tx.clone();

            // fan-in: messages from this transport → unified in_rx
            tokio::spawn(async move {
                while let Some(msg) = t_in.recv().await {
                    if in_tx_clone.send(msg).await.is_err() { break; }
                }
            });

            // fan-out: broadcast → this transport's outbox
            let mut bc_rx_clone = broadcast_tx.subscribe();
            tokio::spawn(async move {
                while let Ok(msg) = bc_rx_clone.recv().await {
                    if t_out.send(msg).await.is_err() { break; }
                }
            });
        }

        // Forward out_rx to broadcast
        tokio::spawn(async move {
            let mut out_rx = out_rx;
            while let Some(msg) = out_rx.recv().await {
                let _ = broadcast_tx.send(msg);
            }
        });

        Ok((out_tx, in_rx))
    }

    fn name(&self) -> &'static str { "multiplexed" }
}

// ─────────────────────────────────────────────────────────────────────────────
//  TRANSPORT FACTORY
// ─────────────────────────────────────────────────────────────────────────────

pub enum TransportKind {
    Stdio,
    Tcp { addr: SocketAddr, server: bool },
    WebSocket { addr: SocketAddr, path: String },
}

pub fn make_transport(kind: TransportKind) -> Box<dyn Transport> {
    match kind {
        TransportKind::Stdio => Box::new(StdioTransport),
        TransportKind::Tcp { addr, server: true } => Box::new(TcpServerTransport::new(addr)),
        TransportKind::Tcp { addr, server: false } => Box::new(TcpClientTransport::new(addr)),
        TransportKind::WebSocket { addr, path } => Box::new(WebSocketTransport::new(addr, path)),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  CONNECTION POOL
// ─────────────────────────────────────────────────────────────────────────────

pub struct ConnectionPool {
    max_size: usize,
    connections: Arc<Mutex<Vec<(String, mpsc::Sender<Value>)>>>,
}

impl ConnectionPool {
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            connections: Arc::new(Mutex::new(Vec::with_capacity(max_size))),
        }
    }

    pub async fn add(&self, id: impl Into<String>, sender: mpsc::Sender<Value>) -> bool {
        let mut conns = self.connections.lock().await;
        if conns.len() >= self.max_size {
            return false;
        }
        conns.push((id.into(), sender));
        true
    }

    pub async fn remove(&self, id: &str) {
        let mut conns = self.connections.lock().await;
        conns.retain(|(cid, _)| cid.as_str() != id);
    }

    pub async fn broadcast(&self, msg: Value) {
        let conns = self.connections.lock().await;
        for (id, sender) in conns.iter() {
            if sender.send(msg.clone()).await.is_err() {
                debug!("Connection {} closed during broadcast", id);
            }
        }
    }

    pub async fn send_to(&self, id: &str, msg: Value) -> bool {
        let conns = self.connections.lock().await;
        if let Some((_, sender)) = conns.iter().find(|(cid, _)| cid.as_str() == id) {
            return sender.send(msg).await.is_ok();
        }
        false
    }

    pub async fn count(&self) -> usize {
        self.connections.lock().await.len()
    }
}
