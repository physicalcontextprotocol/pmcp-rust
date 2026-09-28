//! P-MCP REST API Server
//!
//! HTTP REST API for remote management and control

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use async_trait::async_trait;

/// API Route
#[derive(Clone)]
pub struct Route {
    pub method: HttpMethod,
    pub path: String,
    pub handler: Arc<dyn ApiHandler>,
}

impl std::fmt::Debug for Route {
    // dyn ApiHandler doesn't implement Debug (and forcing that bound onto
    // every handler impl is more invasive than warranted), so this is
    // hand-rolled rather than derived.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Route")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("handler", &"<dyn ApiHandler>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

/// API Handler Trait
///
/// NOTE: Route.handler is Arc<dyn ApiHandler>, so this trait must be
/// dyn-compatible. The original `impl Future<Output = ...> + '_` return-
/// position-impl-trait style is NOT object-safe (the concrete Future type
/// varies per implementor, which `dyn` cannot express) -- this never
/// actually compiled as a trait object. #[async_trait] boxes the future,
/// which is the standard, working pattern for this.
#[async_trait]
pub trait ApiHandler: Send + Sync {
    async fn handle(&self, request: &ApiRequest) -> ApiResponse;
}

/// API Request
#[derive(Debug, Clone)]
pub struct ApiRequest {
    pub method: HttpMethod,
    pub path: String,
    pub query_params: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
}

impl ApiRequest {
    pub fn get_query(&self, key: &str) -> Option<&str> {
        self.query_params.get(key).map(|s| s.as_str())
    }

    pub fn get_header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(|s| s.as_str())
    }
}

/// API Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Option<serde_json::Value>,
}

impl ApiResponse {
    pub fn ok(body: serde_json::Value) -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: Some(body),
        }
    }

    pub fn created(body: serde_json::Value) -> Self {
        Self {
            status: 201,
            headers: HashMap::new(),
            body: Some(body),
        }
    }

    pub fn not_found(message: &str) -> Self {
        Self {
            status: 404,
            headers: HashMap::new(),
            body: Some(serde_json::json!({ "error": message })),
        }
    }

    pub fn error(status: u16, message: &str) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: Some(serde_json::json!({ "error": message })),
        }
    }

    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.insert(key.to_string(), value.to_string());
        self
    }
}

/// API Server
pub struct ApiServer {
    routes: Arc<RwLock<Vec<Route>>>,
    prefix: String,
}

impl ApiServer {
    pub fn new(prefix: &str) -> Self {
        Self {
            routes: Arc::new(RwLock::new(Vec::new())),
            prefix: prefix.to_string(),
        }
    }

    pub async fn register(&self, method: HttpMethod, path: &str, handler: impl ApiHandler + 'static) {
        let route = Route {
            method,
            path: path.to_string(),
            handler: Arc::new(handler),
        };
        self.routes.write().await.push(route);
    }

    pub async fn get(&self, path: &str, handler: impl ApiHandler + 'static) {
        self.register(HttpMethod::Get, path, handler).await;
    }

    pub async fn post(&self, path: &str, handler: impl ApiHandler + 'static) {
        self.register(HttpMethod::Post, path, handler).await;
    }

    pub async fn put(&self, path: &str, handler: impl ApiHandler + 'static) {
        self.register(HttpMethod::Put, path, handler).await;
    }

    pub async fn delete(&self, path: &str, handler: impl ApiHandler + 'static) {
        self.register(HttpMethod::Delete, path, handler).await;
    }

    pub async fn handle(&self, request: &ApiRequest) -> ApiResponse {
        let routes = self.routes.read().await;
        
        for route in routes.iter() {
            if route.method == request.method && self.match_path(&route.path, &request.path) {
                return route.handler.handle(request).await;
            }
        }

        ApiResponse::not_found(&format!("Route not found: {} {}", 
            format!("{:?}", request.method), request.path))
    }

    fn match_path(&self, pattern: &str, path: &str) -> bool {
        let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if pattern_parts.len() != path_parts.len() {
            return false;
        }

        for (p, a) in pattern_parts.iter().zip(path_parts.iter()) {
            if p.starts_with(':') {
                continue; // Path parameter
            }
            if p != a {
                return false;
            }
        }

        true
    }
}

/// Health Endpoint Handler
pub struct HealthHandler;

#[async_trait]
impl ApiHandler for HealthHandler {
    async fn handle(&self, _: &ApiRequest) -> ApiResponse {
        ApiResponse::ok(serde_json::json!({
            "status": "healthy",
            "timestamp": chrono::Utc::now().timestamp(),
        }))
    }
}

/// Status Endpoint Handler
pub struct StatusHandler {
    server_state: Arc<RwLock<ServerState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerState {
    pub robot_id: String,
    pub uptime_s: f64,
    pub call_count: u64,
    pub blocked_count: u64,
    pub active_sessions: usize,
}

impl StatusHandler {
    fn new(state: Arc<RwLock<ServerState>>) -> Self {
        Self { server_state: state }
    }
}

#[async_trait]
impl ApiHandler for StatusHandler {
    async fn handle(&self, _: &ApiRequest) -> ApiResponse {
        let state = self.server_state.read().await;
        ApiResponse::ok(serde_json::json!({
            "robot_id": state.robot_id,
            "uptime_s": state.uptime_s,
            "call_count": state.call_count,
            "blocked_count": state.blocked_count,
            "active_sessions": state.active_sessions,
        }))
    }
}

/// List Tools Handler
pub struct ListToolsHandler {
    actuations: Arc<RwLock<Vec<serde_json::Value>>>,
}

impl ListToolsHandler {
    fn new(actuations: Arc<RwLock<Vec<serde_json::Value>>>) -> Self {
        Self { actuations }
    }
}

#[async_trait]
impl ApiHandler for ListToolsHandler {
    async fn handle(&self, _: &ApiRequest) -> ApiResponse {
        let actuations = self.actuations.read().await;
        ApiResponse::ok(serde_json::json!({
            "tools": actuations.clone(),
        }))
    }
}

/// Call Tool Handler
pub struct CallToolHandler {
    actuations: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl CallToolHandler {
    fn new(actuations: Arc<RwLock<HashMap<String, serde_json::Value>>>) -> Self {
        Self { actuations }
    }
}

#[async_trait]
impl ApiHandler for CallToolHandler {
    async fn handle(&self, request: &ApiRequest) -> ApiResponse {
        if let Some(body) = &request.body {
            let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = body.get("arguments").cloned().unwrap_or(serde_json::json!({}));

            let actuations = self.actuations.read().await;
            if actuations.contains_key(name) {
                // In real impl, would execute actuation
                return ApiResponse::ok(serde_json::json!({
                    "success": true,
                    "output": args,
                }));
            }
            return ApiResponse::error(404, &format!("Tool not found: {}", name));
        }
        ApiResponse::error(400, "Missing request body")
    }
}

/// Resources Handler
pub struct ResourcesHandler {
    sensors: Arc<RwLock<Vec<serde_json::Value>>>,
}

impl ResourcesHandler {
    fn new(sensors: Arc<RwLock<Vec<serde_json::Value>>>) -> Self {
        Self { sensors }
    }
}

#[async_trait]
impl ApiHandler for ResourcesHandler {
    async fn handle(&self, request: &ApiRequest) -> ApiResponse {
        if request.path.contains("/sensors/") {
            // Get specific sensor
            let name = request.path.split("/sensors/").nth(1).unwrap_or("");
            let sensors = self.sensors.read().await;
            for s in sensors.iter() {
                if s.get("name").and_then(|v| v.as_str()) == Some(name) {
                    return ApiResponse::ok(s.clone());
                }
            }
            return ApiResponse::not_found(&format!("Sensor not found: {}", name));
        }

        // List all
        let sensors = self.sensors.read().await;
        ApiResponse::ok(serde_json::json!({
            "resources": sensors.clone(),
        }))
    }
}

/// Leases Handler
pub struct LeasesHandler {
    leases: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl LeasesHandler {
    fn new(leases: Arc<RwLock<HashMap<String, serde_json::Value>>>) -> Self {
        Self { leases }
    }
}

#[async_trait]
impl ApiHandler for LeasesHandler {
    async fn handle(&self, request: &ApiRequest) -> ApiResponse {
        match request.method {
            HttpMethod::Get => {
                let leases = self.leases.read().await;
                ApiResponse::ok(serde_json::json!({
                    "leases": leases.values().cloned().collect::<Vec<_>>(),
                }))
            }
            HttpMethod::Post => {
                if let Some(body) = &request.body {
                    let zone_id = body.get("zone_id").and_then(|v| v.as_str()).unwrap_or("default");
                    let robot_id = body.get("robot_id").and_then(|v| v.as_str()).unwrap_or("unknown");
                    let mut leases = self.leases.write().await;
                    let lease_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
                    leases.insert(zone_id.to_string(), serde_json::json!({
                        "lease_id": lease_id,
                        "zone_id": zone_id,
                        "robot_id": robot_id,
                        // schema/v0.6.0 LeaseState -- previously "GRANTED",
                        // which isn't a legal value (enum is
                        // FREE/PENDING/ACTIVE/EXPIRED/DENIED). Same drift
                        // bug already fixed in Python and Rust lease.rs.
                        "state": "ACTIVE",
                    }));
                    return ApiResponse::created(serde_json::json!({
                        "lease_id": lease_id,
                    }));
                }
                ApiResponse::error(400, "Missing request body")
            }
            HttpMethod::Delete => {
                let path = request.path.split("/leases/").nth(1).unwrap_or("");
                let mut leases = self.leases.write().await;
                leases.remove(path);
                ApiResponse::ok(serde_json::json!({ "released": true }))
            }
            _ => ApiResponse::error(405, "Method not allowed"),
        }
    }
}

/// Configuration Handler
pub struct ConfigHandler {
    configs: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl ConfigHandler {
    fn new(configs: Arc<RwLock<HashMap<String, serde_json::Value>>>) -> Self {
        Self { configs }
    }
}

#[async_trait]
impl ApiHandler for ConfigHandler {
    async fn handle(&self, request: &ApiRequest) -> ApiResponse {
        match request.method {
            HttpMethod::Get => {
                if request.path.contains('/') && !request.path.ends_with("/config") {
                    let key = request.path.split("/config/").nth(1).unwrap_or("");
                    let configs = self.configs.read().await;
                    if let Some(v) = configs.get(key) {
                        return ApiResponse::ok(v.clone());
                    }
                    return ApiResponse::not_found(&format!("Config not found: {}", key));
                }
                let configs = self.configs.read().await;
                ApiResponse::ok(serde_json::json!({
                    "configs": configs.clone(),
                }))
            }
            HttpMethod::Put => {
                if let Some(body) = &request.body {
                    let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("");
                    let value = body.get("value").cloned().unwrap_or(serde_json::json!(null));
                    let mut configs = self.configs.write().await;
                    configs.insert(key.to_string(), value);
                    return ApiResponse::ok(serde_json::json!({ "updated": key }));
                }
                ApiResponse::error(400, "Missing request body")
            }
            _ => ApiResponse::error(405, "Method not allowed"),
        }
    }
}

/// WebSocket Upgrade Handler
pub struct WebSocketHandler;

#[async_trait]
impl ApiHandler for WebSocketHandler {
    async fn handle(&self, request: &ApiRequest) -> ApiResponse {
        // Return upgrade response
        ApiResponse::ok(serde_json::json!({
            "message": "WebSocket upgrade required",
            "headers": {
                "Upgrade": "websocket",
                "Connection": "Upgrade",
            },
        }))
    }
}

/// OpenAPI/Swagger Documentation
pub struct OpenAPIHandler;

impl OpenAPIHandler {
    fn spec() -> serde_json::Value {
        serde_json::json!({
            "openapi": "3.0.0",
            "info": {
                "title": "P-MCP REST API",
                "version": "0.5.0",
                "description": "Physical Model Context Protocol REST API",
            },
            "servers": [
                { "url": "http://localhost:8080/pmcp/api", "description": "Local server" }
            ],
            "paths": {
                "/health": {
                    "get": {
                        "summary": "Health check",
                        "responses": {
                            "200": { "description": "OK" }
                        }
                    }
                },
                "/status": {
                    "get": {
                        "summary": "Server status",
                        "responses": {
                            "200": { "description": "Status response" }
                        }
                    }
                },
                "/tools": {
                    "get": {
                        "summary": "List tools (actuations)",
                        "responses": {
                            "200": { "description": "Tools list" }
                        }
                    },
                    "post": {
                        "summary": "Call a tool",
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "name": { "type": "string" },
                                            "arguments": { "type": "object" }
                                        }
                                    }
                                }
                            }
                        },
                        "responses": {
                            "200": { "description": "Tool result" }
                        }
                    }
                },
                "/sensors": {
                    "get": {
                        "summary": "List sensors (resources)",
                        "responses": {
                            "200": { "description": "Sensors list" }
                        }
                    }
                },
                "/leases": {
                    "get": {
                        "summary": "List leases",
                        "responses": {
                            "200": { "description": "Leases list" }
                        }
                    },
                    "post": {
                        "summary": "Request lease",
                        "responses": {
                            "201": { "description": "Lease granted" }
                        }
                    }
                },
                "/config": {
                    "get": {
                        "summary": "List configurations",
                        "responses": {
                            "200": { "description": "Configs list" }
                        }
                    }
                }
            }
        })
    }
}

#[async_trait]
impl ApiHandler for OpenAPIHandler {
    async fn handle(&self, _: &ApiRequest) -> ApiResponse {
        ApiResponse::ok(Self::spec())
    }
}

/// CORS Middleware
pub struct CorsMiddleware;

impl CorsMiddleware {
    fn apply(response: &mut ApiResponse) {
        response.headers.insert("Access-Control-Allow-Origin".to_string(), "*".to_string());
        response.headers.insert("Access-Control-Allow-Methods".to_string(), "GET, POST, PUT, DELETE, OPTIONS".to_string());
        response.headers.insert("Access-Control-Allow-Headers".to_string(), "Content-Type, Authorization".to_string());
    }
}

/// Rate Limiter Middleware
pub struct RateLimiter {
    requests: Arc<RwLock<HashMap<String, Vec<f64>>>>,
    max_requests: u32,
    window_seconds: u64,
}

impl RateLimiter {
    fn new(max_requests: u32, window_seconds: u64) -> Self {
        Self {
            requests: Arc::new(RwLock::new(HashMap::new())),
            max_requests,
            window_seconds,
        }
    }

    async fn check(&self, client_id: &str) -> bool {
        let now = chrono::Utc::now().timestamp() as f64;
        let mut requests = self.requests.write().await;
        
        let timestamps = requests.entry(client_id.to_string()).or_insert_with(Vec::new);
        
        // Remove old timestamps
        timestamps.retain(|t| *t > now - self.window_seconds as f64);
        
        if timestamps.len() >= self.max_requests as usize {
            return false;
        }
        
        timestamps.push(now);
        true
    }
}

/// Authentication Middleware
pub struct AuthMiddleware {
    valid_tokens: Arc<RwLock<Vec<String>>>,
}

impl AuthMiddleware {
    fn new() -> Self {
        Self {
            valid_tokens: Arc::new(RwLock::new(Vec::new())),
        }
    }

    async fn add_token(&self, token: &str) {
        self.valid_tokens.write().await.push(token.to_string());
    }

    async fn validate(&self, token: &str) -> bool {
        let tokens = self.valid_tokens.read().await;
        tokens.contains(&token.to_string())
    }

    async fn check(&self, request: &ApiRequest) -> Result<(), String> {
        if let Some(auth) = request.get_header("Authorization") {
            if auth.starts_with("Bearer ") {
                let token = &auth[7..];
                if self.validate(token).await {
                    return Ok(());
                }
            }
        }
        Err("Unauthorized".to_string())
    }
}

/// API Builder for easy setup
pub struct ApiServerBuilder {
    prefix: String,
    server: ApiServer,
    state: Arc<RwLock<ServerState>>,
    actuations: Arc<RwLock<Vec<serde_json::Value>>>,
    actuation_map: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    sensors: Arc<RwLock<Vec<serde_json::Value>>>,
    leases: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    configs: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl ApiServerBuilder {
    pub fn new() -> Self {
        Self {
            prefix: "/pmcp/api".to_string(),
            server: ApiServer::new("/pmcp/api"),
            state: Arc::new(RwLock::new(ServerState {
                robot_id: "default".to_string(),
                uptime_s: 0.0,
                call_count: 0,
                blocked_count: 0,
                active_sessions: 0,
            })),
            actuations: Arc::new(RwLock::new(Vec::new())),
            actuation_map: Arc::new(RwLock::new(HashMap::new())),
            sensors: Arc::new(RwLock::new(Vec::new())),
            leases: Arc::new(RwLock::new(HashMap::new())),
            configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn prefix(mut self, prefix: &str) -> Self {
        self.prefix = prefix.to_string();
        self
    }

    pub fn robot_id(self, id: &str) -> Self {
        let state = self.state.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            let mut s = state.write().await;
            s.robot_id = id;
        });
        self
    }

    pub async fn build(mut self) -> ApiServer {
        // Register routes
        self.server.get("/health", HealthHandler).await;
        self.server.get("/status", StatusHandler::new(self.state.clone())).await;
        self.server.get("/tools", ListToolsHandler::new(self.actuations.clone())).await;
        self.server.post("/tools/call", CallToolHandler::new(self.actuation_map.clone())).await;
        self.server.get("/sensors", ResourcesHandler::new(self.sensors.clone())).await;
        self.server.get("/leases", LeasesHandler::new(self.leases.clone())).await;
        self.server.post("/leases", LeasesHandler::new(self.leases.clone())).await;
        self.server.get("/config", ConfigHandler::new(self.configs.clone())).await;
        self.server.put("/config", ConfigHandler::new(self.configs.clone())).await;
        self.server.get("/openapi.json", OpenAPIHandler).await;

        self.server
    }
}

impl Default for ApiServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Request Logger Middleware
pub struct RequestLogger;

impl RequestLogger {
    fn log(request: &ApiRequest, response: &ApiResponse) {
        tracing::info!("[API] {} {} -> {}",
            format!("{:?}", request.method),
            request.path,
            response.status,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_response() {
        let resp = ApiResponse::ok(serde_json::json!({"test": true}));
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn test_cors_middleware() {
        let mut resp = ApiResponse::ok(serde_json::json!({}));
        CorsMiddleware::apply(&mut resp);
        assert!(resp.headers.contains_key("Access-Control-Allow-Origin"));
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = RateLimiter::new(5, 60);
        assert!(limiter.check("client1").await);
        assert!(limiter.check("client1").await);
        assert!(limiter.check("client1").await);
        assert!(limiter.check("client1").await);
        assert!(limiter.check("client1").await);
        assert!(!limiter.check("client1").await); // 6th should fail
    }

    #[tokio::test]
    async fn test_api_server_builder() {
        let server = ApiServerBuilder::new()
            .robot_id("test-robot")
            .build()
            .await;
        
        let request = ApiRequest {
            method: HttpMethod::Get,
            path: "/health".to_string(),
            query_params: HashMap::new(),
            headers: HashMap::new(),
            body: None,
        };
        
        let response = server.handle(&request).await;
        assert_eq!(response.status, 200);
    }
}