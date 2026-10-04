//! PCP Server Implementation
//!
//! Main server that handles JSON-RPC messages and dispatches to handlers

use std::sync::Arc;
use tokio::sync::RwLock;
use crate::types::*;
use crate::error::{PcpError, PcpErrorCode};
use crate::methods::*;
use crate::lease::LeaseManager;
use crate::safety::SafetyMiddleware;

/// Actuation function type - would be defined by user
pub type ActuationFn = Box<dyn Fn(serde_json::Value) -> Result<ActuationResult, String> + Send + Sync>;

/// Sensor function type
pub type SensorFn = Box<dyn Fn() -> Result<SensorReading, String> + Send + Sync>;

/// PCPServer - Main server struct
pub struct PCPServer {
    // Arc-shared, not deep-cloned: multiple PCPServer handles (e.g. one per
    // tokio::spawn'd connection/notification task -- see handle_message's
    // notification branch) must observe the SAME lease/safety/audit state.
    // A plain RwLock<HandlerContext> here previously made every clone() an
    // independent snapshot, silently breaking lease-exclusion guarantees
    // across concurrent connections (and didn't even compile, since
    // Clone::clone cannot .await).
    ctx: Arc<RwLock<HandlerContext>>,
    actuation_fn: Option<Arc<ActuationFn>>,
    sensor_fn: Option<Arc<SensorFn>>,
}

impl PCPServer {
    /// Create a new PCP Server
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        robot_id: Option<String>,
        robot_class: &str,
        model: &str,
        serial: &str,
        location: &str,
    ) -> Self {
        let ctx = HandlerContext::new(
            name, version, robot_id, robot_class, model, serial, location
        );

        Self {
            ctx: Arc::new(RwLock::new(ctx)),
            actuation_fn: None,
            sensor_fn: None,
        }
    }

    /// Register an actuation (MCP Tool equivalent)
    pub async fn register_actuation(&self, spec: ActuationSpec) {
        let mut ctx = self.ctx.write().await;
        ctx.actuations.write().await.push(spec);
    }

    /// Register a sensor (MCP Resource equivalent)
    pub async fn register_sensor(&self, spec: SensorSpec) {
        let mut ctx = self.ctx.write().await;
        ctx.sensors.write().await.push(spec);
    }

    /// Set actuation handler function
    pub fn set_actuation_fn<F>(&mut self, f: F)
    where
        F: Fn(serde_json::Value) -> Result<ActuationResult, String> + Send + Sync + 'static,
    {
        self.actuation_fn = Some(Arc::new(Box::new(f)));
    }

    /// Set sensor handler function
    pub fn set_sensor_fn<F>(&mut self, f: F)
    where
        F: Fn() -> Result<SensorReading, String> + Send + Sync + 'static,
    {
        self.sensor_fn = Some(Arc::new(Box::new(f)));
    }

    /// Handle a JSON-RPC request
    pub async fn handle_message(&self, raw: serde_json::Value) -> Option<JsonRpcResponse> {
        // Parse request
        let req: JsonRpcRequest = match serde_json::from_value(raw.clone()) {
            Ok(r) => r,
            Err(e) => {
                return Some(JsonRpcResponse::error(
                    None,
                    PcpError::with_message(PcpErrorCode::ParseError, e.to_string()),
                ));
            }
        };

        // Handle notifications (no response needed)
        if req.is_notification() {
            // Process notification asynchronously
            let server = self.clone();
            tokio::spawn(async move {
                let _ = server.handle_notification(&req.method, req.params).await;
            });
            return None;
        }

        let id = req.id.clone();
        let method = req.method.clone();
        let params = req.params;

        // Dispatch to handler
        let result = self.handle_method(&method, params).await;

        match result {
            Ok(value) => Some(JsonRpcResponse::success(id, value)),
            Err(e) => Some(JsonRpcResponse::error(id, e)),
        }
    }

    /// Handle a single method
    async fn handle_method(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, PcpError> {
        match method {
            // MCP Standard Methods
            "initialize" => {
                let mut ctx = self.ctx.write().await;
                handle_initialize(&mut ctx, &params).await
            }
            "ping" => {
                let ctx = self.ctx.read().await;
                handle_ping(&ctx, &params).await
            }
            "tools/list" => {
                let ctx = self.ctx.read().await;
                handle_tools_list(&ctx, &params).await
            }
            "tools/call" => {
                let mut ctx = self.ctx.write().await;
                handle_tools_call(&mut ctx, &params).await
            }
            "resources/list" => {
                let ctx = self.ctx.read().await;
                handle_resources_list(&ctx, &params).await
            }
            "resources/read" => {
                let ctx = self.ctx.read().await;
                handle_resources_read(&ctx, &params).await
            }
            "logging/setLevel" => {
                let ctx = self.ctx.read().await;
                handle_logging_set_level(&ctx, &params).await
            }

            // PCP Extension Methods
            "shadow/preview" => {
                let ctx = self.ctx.read().await;
                handle_shadow_preview(&ctx, &params).await
            }
            "lease/request" => {
                let ctx = self.ctx.read().await;
                handle_lease_request(&ctx, &params).await
            }
            "lease/release" => {
                let ctx = self.ctx.read().await;
                handle_lease_release(&ctx, &params).await
            }
            "pcp/estop" => {
                let ctx = self.ctx.read().await;
                handle_estop(&ctx, &params).await
            }
            "pcp/status" => {
                let ctx = self.ctx.read().await;
                handle_status(&ctx, &params).await
            }
            "pcp/identity" => {
                let ctx = self.ctx.read().await;
                handle_identity(&ctx, &params).await
            }
            "pcp/constitution" => {
                let ctx = self.ctx.read().await;
                handle_constitution(&ctx, &params).await
            }

            _ => Err(PcpError::method_not_found(method)),
        }
    }

    /// Handle notifications (fire-and-forget)
    async fn handle_notification(&self, method: &str, params: serde_json::Value) -> Result<(), PcpError> {
        match method {
            "pcp/estop" => {
                let active = params.get("active").and_then(|v| v.as_bool()).unwrap_or(true);
                self.ctx.write().await.safety.write().await.set_estop(active);
                tracing::info!("[Server] Notification: E-stop {}", if active { "ON" } else { "OFF" });
            }
            _ => {
                tracing::warn!("[Server] Unknown notification: {}", method);
            }
        }
        Ok(())
    }

    /// Run server in stdio mode (MCP stdio transport)
    pub async fn run_stdio(&self) {
        use std::io::{BufRead, BufReader, Write};

        tracing::info!("[Server] Starting in stdio mode...");

        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut reader = BufReader::new(stdin.lock());
        let mut writer = stdout.lock();

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let raw: serde_json::Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(e) => {
                            let resp = JsonRpcResponse::error(
                                None,
                                PcpError::with_message(PcpErrorCode::ParseError, e.to_string()),
                            );
                            let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap_or_default());
                            continue;
                        }
                    };

                    let response = self.handle_message(raw).await;

                    if let Some(resp) = response {
                        let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap_or_default());
                        let _ = writer.flush();
                    }
                }
                Err(e) => {
                    tracing::error!("[Server] Read error: {}", e);
                    break;
                }
            }
        }
    }

    /// Run server in HTTP mode
    pub async fn run_http(&self, host: &str, port: u16) {
        tracing::info!("[Server] HTTP mode not yet implemented - use stdio mode");
        // In full implementation, would use axum or actix-web
    }
}

impl Clone for PCPServer {
    fn clone(&self) -> Self {
        Self {
            ctx: Arc::clone(&self.ctx),
            actuation_fn: self.actuation_fn.clone(),
            sensor_fn: self.sensor_fn.clone(),
        }
    }
}

// ============================================================================
// Python Bindings Support
// ============================================================================

/// Builder pattern for creating servers
pub struct PCPServerBuilder {
    name: String,
    version: String,
    robot_id: Option<String>,
    robot_class: String,
    model: String,
    serial: String,
    location: String,
    actuations: Vec<ActuationSpec>,
    sensors: Vec<SensorSpec>,
}

impl PCPServerBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: "1.0.0".to_string(),
            robot_id: None,
            robot_class: "arm".to_string(),
            model: "generic".to_string(),
            serial: "000000".to_string(),
            location: "lab-01".to_string(),
            actuations: Vec::new(),
            sensors: Vec::new(),
        }
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn robot_id(mut self, id: impl Into<String>) -> Self {
        self.robot_id = Some(id.into());
        self
    }

    pub fn robot_class(mut self, class: impl Into<String>) -> Self {
        self.robot_class = class.into();
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn serial(mut self, serial: impl Into<String>) -> Self {
        self.serial = serial.into();
        self
    }

    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = location.into();
        self
    }

    pub fn actuation(mut self, spec: ActuationSpec) -> Self {
        self.actuations.push(spec);
        self
    }

    pub fn sensor(mut self, spec: SensorSpec) -> Self {
        self.sensors.push(spec);
        self
    }

    pub fn build(self) -> PCPServer {
        let server = PCPServer::new(
            self.name,
            self.version,
            self.robot_id,
            &self.robot_class,
            &self.model,
            &self.serial,
            &self.location,
        );

        // Register actuations and sensors
        let server = server;
        // In real impl, would register here
        server
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_creation() {
        let server = PCPServer::new(
            "test-robot",
            "1.0.0",
            None,
            "arm",
            "UR5",
            "serial123",
            "lab-01",
        );

        let ctx = server.ctx.read().await;
        assert_eq!(ctx.name, "test-robot");
        assert_eq!(ctx.robot_id, "test-robot");
        assert_eq!(ctx.identity.robot_class, "arm");
    }

    #[tokio::test]
    async fn test_initialize() {
        let server = PCPServer::new("test", "1.0.0", None, "arm", "test", "000", "lab");

        let req = JsonRpcRequest::with_id(
            "initialize",
            serde_json::json!({
                "clientInfo": {"name": "test-client", "version": "1.0.0"}
            }),
            "1",
        );

        let response = server.handle_message(serde_json::to_value(req).unwrap()).await;
        assert!(response.is_some());

        let resp = response.unwrap();
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn test_tools_list_empty() {
        let server = PCPServer::new("test", "1.0.0", None, "arm", "test", "000", "lab");

        let req = JsonRpcRequest::with_id("tools/list", serde_json::json!({}), "1");
        let response = server.handle_message(serde_json::to_value(req).unwrap()).await;

        let resp = response.unwrap();
        let result = resp.result.unwrap();
        let tools = result.get("tools").unwrap().as_array().unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_ping() {
        let server = PCPServer::new("test", "1.0.0", None, "arm", "test", "000", "lab");

        let req = JsonRpcRequest::with_id("ping", serde_json::json!({}), "1");
        let response = server.handle_message(serde_json::to_value(req).unwrap()).await;

        let resp = response.unwrap();
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let server = PCPServer::new("test", "1.0.0", None, "arm", "test", "000", "lab");

        let req = JsonRpcRequest::with_id("unknown/method", serde_json::json!({}), "1");
        let response = server.handle_message(serde_json::to_value(req).unwrap()).await;

        let resp = response.unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, PcpErrorCode::MethodNotFound.code());
    }
}