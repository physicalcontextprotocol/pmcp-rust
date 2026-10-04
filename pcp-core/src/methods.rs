//! PCP JSON-RPC Method Handlers
//!
//! Implements all MCP standard methods and PCP physical extensions

use std::sync::Arc;
use tokio::sync::RwLock;
use crate::types::*;
use crate::error::PcpError;
use crate::lease::LeaseManager;
use crate::safety::SafetyMiddleware;

/// Handler context - shared state for all method handlers
pub struct HandlerContext {
    pub robot_id: String,
    pub name: String,
    pub version: String,
    pub identity: RobotIdentity,
    pub capabilities: Capabilities,
    pub started_at: f64,
    pub call_count: u64,
    pub blocked_count: u64,
    pub actuations: Arc<RwLock<Vec<ActuationSpec>>>,
    pub sensors: Arc<RwLock<Vec<SensorSpec>>>,
    pub lease_mgr: LeaseManager,
    pub safety: RwLock<SafetyMiddleware>,
}

impl HandlerContext {
    pub fn new(name: impl Into<String>, version: impl Into<String>,
               robot_id: Option<String>, robot_class: &str,
               model: &str, serial: &str, location: &str) -> Self {
        let name = name.into();
        let robot_id = robot_id.unwrap_or_else(|| name.clone());
        let identity = RobotIdentity::new(robot_class, model, serial, location);
        let started_at = chrono::Utc::now().timestamp() as f64;

        Self {
            robot_id: robot_id.clone(),
            name: name.clone(),
            version: version.into(),
            identity,
            capabilities: Capabilities::default(),
            started_at,
            call_count: 0,
            blocked_count: 0,
            actuations: Arc::new(RwLock::new(Vec::new())),
            sensors: Arc::new(RwLock::new(Vec::new())),
            lease_mgr: LeaseManager::new(),
            safety: RwLock::new(SafetyMiddleware::new(robot_id)),
        }
    }

    pub fn increment_call_count(&mut self) {
        self.call_count += 1;
    }

    pub fn increment_blocked_count(&mut self) {
        self.blocked_count += 1;
    }

    pub fn uptime(&self) -> f64 {
        chrono::Utc::now().timestamp() as f64 - self.started_at
    }
}

// ============================================================================
// MCP Standard Methods
// ============================================================================

/// Initialize handler
pub async fn handle_initialize(ctx: &mut HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let client_info = params.get("clientInfo")
        .and_then(|v| v.as_object())
        .map(|o| {
            (
                o.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                o.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
            )
        });

    if let Some((name, version)) = client_info {
        tracing::info!("[Server] initialize from {} v{}", name, version);
    }

    ctx.started_at = chrono::Utc::now().timestamp() as f64;

    Ok(serde_json::json!({
        "protocolVersion": MCP_VERSION,
        "capabilities": ctx.capabilities.to_mcp_dict(),
        "serverInfo": {
            "name": ctx.name,
            "version": ctx.version,
        },
        "pcp": {
            "version": PCP_VERSION,
            "robotId": ctx.robot_id,
            "identity": serde_json::to_value(&ctx.identity).unwrap_or(serde_json::Value::Null),
            "constitution": ctx.safety.read().await.constitution.fingerprint.chars().take(16).collect::<String>() + "...",
        },
    }))
}

/// Ping handler
pub async fn handle_ping(ctx: &HandlerContext, _params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    Ok(serde_json::json!({
        "pong": true,
        "ts": chrono::Utc::now().timestamp(),
        "robot_id": ctx.robot_id,
    }))
}

/// Tools list handler
pub async fn handle_tools_list(ctx: &HandlerContext, _params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let actuations = ctx.actuations.read().await;
    let tools: Vec<serde_json::Value> = actuations.iter()
        .map(|spec| spec.to_mcp_tool())
        .collect();
    Ok(serde_json::json!({ "tools": tools }))
}

/// Tools call handler
pub async fn handle_tools_call(ctx: &mut HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let name = params.get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PcpError::invalid_params("Missing 'name' parameter"))?;

    let arguments = params.get("arguments")
        .and_then(|v| v.as_object())
        .map(|o| serde_json::Value::Object(o.clone()))
        .unwrap_or(serde_json::Value::Null);

    let lease_token = params.get("lease_token").and_then(|v| v.as_str());
    let zone_id = params.get("zone_id").and_then(|v| v.as_str());
    let fence_token = params.get("fence_token").and_then(|v| v.as_u64());

    // Find actuation
    let actuations = ctx.actuations.read().await;
    let spec = actuations.iter()
        .find(|s| s.name == name)
        .ok_or_else(|| PcpError::method_not_found(name))?;
    let requires_lease = spec.requires_lease;
    let shadow_required = spec.shadow_required;
    drop(actuations);

    ctx.increment_call_count();

    // ── Layer 1: Lease + fencing check ──────────────────────────────────────
    // This is real enforcement against ctx.lease_mgr's actual lease state --
    // previously lease_token/zone_id were extracted from the wire but only
    // ever passed into SafetyMiddleware::check, which silently ignored them
    // (see safety.rs history). That meant no actuation call in this SDK was
    // ever actually blocked by a missing/invalid/stale lease. Fixed here.
    if requires_lease {
        let zid = zone_id.unwrap_or("default");
        let (lease_ok, lease_reason) = ctx.lease_mgr.check(lease_token, zid, fence_token).await;
        if !lease_ok {
            ctx.increment_blocked_count();
            let mut err = PcpError::with_message(
                crate::error::PcpErrorCode::LeaseRequired, lease_reason.clone());
            err.data = Some(serde_json::json!({"violations": [lease_reason]}));
            return Err(err);
        }
    }

    // Safety pipeline
    let (safe, preview, violations) = {
        let safety = ctx.safety.read().await;
        safety.check(name, &arguments, !shadow_required)
    };

    if !safe {
        ctx.increment_blocked_count();
        let code = if violations.iter().any(|v| v.contains("ESTOP")) {
            crate::error::PcpErrorCode::EstopActive
        } else if preview.is_some() {
            crate::error::PcpErrorCode::ShadowBlocked
        } else {
            crate::error::PcpErrorCode::ConstitutionBlocked
        };
        let mut err = PcpError::with_message(code, violations.join("; "));
        err.data = Some(serde_json::json!({
            "violations": violations,
            "shadow": preview.map(|p| p.to_dict()),
        }));
        return Err(err);
    }

    // Build response - in real implementation, this would call the actuation function
    let duration = 0.0; // Would be measured
    let result = ActuationResult {
        success: true,
        robot_id: ctx.robot_id.clone(),
        actuation_name: name.to_string(),
        output: serde_json::json!({"executed": true}),
        duration_s: duration,
        timestamp: chrono::Utc::now().timestamp() as f64,
        ..Default::default()
    };

    let content = result.to_mcp_content();
    let mut response = serde_json::json!({
        "content": content,
        "isError": false,
    });

    if let Some(p) = preview {
        response["_shadow"] = p.to_dict();
    }

    Ok(response)
}

/// Resources list handler
pub async fn handle_resources_list(ctx: &HandlerContext, _params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let sensors = ctx.sensors.read().await;
    let resources: Vec<serde_json::Value> = sensors.iter()
        .map(|spec| spec.to_mcp_resource())
        .collect();
    Ok(serde_json::json!({ "resources": resources }))
}

/// Resources read handler
pub async fn handle_resources_read(ctx: &HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let uri = params.get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PcpError::invalid_params("Missing 'uri' parameter"))?;

    let sensors = ctx.sensors.read().await;
    let name = uri.split('/').last().unwrap_or(uri);

    // Find sensor - in real impl, would call sensor function
    let _spec = sensors.iter()
        .find(|s| s.name == name || s.uri() == uri)
        .ok_or_else(|| PcpError::method_not_found(&format!("Sensor not found: {}", uri)))?;

    // Return simulated reading
    let reading = SensorReading {
        sensor_name: name.to_string(),
        robot_id: ctx.robot_id.clone(),
        value: serde_json::json!({"simulated": true}),
        timestamp: chrono::Utc::now().timestamp() as f64,
        ..Default::default()
    };

    Ok(serde_json::json!({ "contents": reading.to_mcp_content() }))
}

// ============================================================================
// PCP Extension Methods
// ============================================================================

/// Shadow preview handler
pub async fn handle_shadow_preview(ctx: &HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let name = params.get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PcpError::invalid_params("Missing 'name' parameter"))?;

    let arguments = params.get("arguments")
        .and_then(|v| v.as_object())
        .map(|o| serde_json::Value::Object(o.clone()))
        .unwrap_or(serde_json::Value::Null);

    // Find actuation to check if shadow is required
    let actuations = ctx.actuations.read().await;
    let spec = actuations.iter()
        .find(|s| s.name == name);

    let skip_shadow = spec.map(|s| !s.shadow_required).unwrap_or(false);

    let safety = ctx.safety.read().await;
    let (_, preview, _) = safety.check(name, &arguments, skip_shadow);

    match preview {
        Some(p) => Ok(serde_json::json!({ "preview": p.to_dict() })),
        None => Ok(serde_json::json!({ "preview": null })),
    }
}

/// Lease request handler
pub async fn handle_lease_request(ctx: &HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let robot_id = params.get("robot_id")
        .and_then(|v| v.as_str())
        .unwrap_or(&ctx.robot_id);

    let zone_id = params.get("zone_id")
        .and_then(|v| v.as_str())
        .unwrap_or("default");

    let duration_ms = params.get("duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(10_000);

    let bid_energy_j = params.get("bid_energy_j")
        .and_then(|v| v.as_f64())
        .unwrap_or(100.0);

    let priority = params.get("priority")
        .and_then(|v| v.as_u64())
        .unwrap_or(5) as u8;

    let req = LeaseRequest {
        robot_id: robot_id.to_string(),
        zone_id: zone_id.to_string(),
        duration_ms,
        bid_energy_j,
        priority,
    };

    let grant = ctx.lease_mgr.request(&req).await;
    Ok(serde_json::json!({ "lease": grant.to_dict() }))
}

/// Lease release handler
pub async fn handle_lease_release(ctx: &HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let lease_id = params.get("lease_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| PcpError::invalid_params("Missing 'lease_id' parameter"))?;

    let released = ctx.lease_mgr.release(lease_id).await;
    Ok(serde_json::json!({ "released": released, "lease_id": lease_id }))
}

/// Emergency stop handler.
/// Response is a schema-conformant EStopMessage (robot_id, triggered_at,
/// stop_category, source) when triggering; a simple ack when clearing.
/// stop_category is always 0 (schema: const 0 on this message type).
pub async fn handle_estop(ctx: &HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let active = params.get("active").and_then(|v| v.as_bool()).unwrap_or(true);
    let robot_id = params.get("robot_id").and_then(|v| v.as_str()).unwrap_or(&ctx.robot_id);
    let source = params.get("source").and_then(|v| v.as_str());

    ctx.safety.write().await.set_estop(active);

    if active {
        tracing::warn!("[Server] E-STOP TRIGGERED robot={} source={}",
            robot_id, source.unwrap_or("unspecified"));
        let mut estop = serde_json::json!({
            "robot_id": robot_id,
            "triggered_at": chrono::Utc::now().timestamp() as f64,
            "stop_category": 0,
        });
        if let Some(src) = source {
            estop["source"] = serde_json::json!(src);
        }
        Ok(serde_json::json!({ "estop": estop }))
    } else {
        tracing::warn!("[Server] E-Stop reset robot={}", robot_id);
        Ok(serde_json::json!({ "was_estopped": true, "estopped": false }))
    }
}

/// Status handler
pub async fn handle_status(ctx: &HandlerContext, _params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let actuations = ctx.actuations.read().await;
    let sensors = ctx.sensors.read().await;

    Ok(serde_json::json!({
        "robot_id": ctx.robot_id,
        "pcp_version": PCP_VERSION,
        "uptime_s": ctx.uptime(),
        "call_count": ctx.call_count,
        "blocked_count": ctx.blocked_count,
        "safety_stats": {
            "constitution": "loaded",
            "shadow": "enabled",
        },
        "actuations": actuations.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        "sensors": sensors.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        "missions": [],
    }))
}

/// Identity handler
pub async fn handle_identity(ctx: &HandlerContext, _params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    serde_json::to_value(&ctx.identity)
        .map_err(|e| PcpError::internal_error(e.to_string()))
}

/// Constitution handler
pub async fn handle_constitution(ctx: &HandlerContext, _params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let safety = ctx.safety.read().await;
    Ok(safety.constitution.summary())
}

/// Logging set level handler
pub async fn handle_logging_set_level(_ctx: &HandlerContext, params: &serde_json::Value) -> Result<serde_json::Value, PcpError> {
    let level = params.get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("info");

    // In real implementation, would set log level
    tracing::info!("[Server] Log level set to {}", level);
    Ok(serde_json::json!({}))
}