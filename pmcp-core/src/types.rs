//! P-MCP Core Types
//!
//! Core data structures for the Physical Model Context Protocol v0.5.
//! These types mirror the Python implementation and provide MCP-compatible serialization.

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use uuid::Uuid;
use crate::error::PmcpError;

/// Protocol Constants
pub const PMCP_VERSION: &str = "0.5";
pub const JSONRPC_VERSION: &str = "2.0";
pub const MCP_VERSION: &str = "2024-11-05";

// ============================================================================
// JSON-RPC 2.0 Envelopes
// ============================================================================

/// JSON-RPC 2.0 Request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
            id: None,
        }
    }

    pub fn with_id(method: impl Into<String>, params: serde_json::Value, id: impl Into<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
            id: Some(id.into()),
        }
    }

    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<PmcpError>,
    pub id: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: Option<serde_json::Value>, error: PmcpError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: None,
            error: Some(error),
            id,
        }
    }
}

// ============================================================================
// Robot Identity (W3C DID-based)
// ============================================================================

/// Robot Identity - W3C DID-based hardware identity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RobotIdentity {
    pub did: String,
    pub robot_class: String,
    pub model: String,
    pub serial: String,
    #[serde(default)]
    pub firmware_ver: String,
    #[serde(default)]
    pub cert_hash: String,
    #[serde(default)]
    pub public_key: String,
    pub location: String,
}

impl RobotIdentity {
    pub fn new(robot_class: impl Into<String>, model: impl Into<String>,
              serial: impl Into<String>, location: impl Into<String>) -> Self {
        let uid = Uuid::new_v4().to_string()[..8].to_string();
        let class = robot_class.into().to_lowercase();
        let model = model.into();
        let model_lower = model.to_lowercase();
        let loc = location.into().to_lowercase();
        let did = format!("did:pmcp:{}:{}:{}:{}", class, model_lower, loc, uid);

        Self {
            did,
            robot_class: class,
            model,
            serial: serial.into(),
            firmware_ver: "0.5.0".to_string(),
            cert_hash: String::new(),
            public_key: String::new(),
            location: loc,
        }
    }
}

// ============================================================================
// Actuation Types (MCP Tool equivalent)
// ============================================================================

/// Actuation parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActuationParameter {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
    #[serde(default)]
    pub r#enum: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub unit: String,
}

fn default_true() -> bool { true }

/// Actuation specification - the P-MCP equivalent of an MCP Tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActuationSpec {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ActuationParameter>,
    pub robot_id: String,
    #[serde(default = "default_motion")]
    pub category: String,
    #[serde(default = "default_speed")]
    pub max_speed_m_s: f64,
    #[serde(default = "default_force")]
    pub max_force_n: f64,
    #[serde(default = "default_energy")]
    pub max_energy_j: f64,
    #[serde(default = "default_duration")]
    pub est_duration_s: f64,
    #[serde(default = "default_true")]
    pub requires_lease: bool,
    #[serde(default = "default_true")]
    pub shadow_required: bool,
    #[serde(default = "default_iso")]
    pub iso_class: String,
}

fn default_motion() -> String { "motion".to_string() }
fn default_speed() -> f64 { 1.0 }
fn default_force() -> f64 { 100.0 }
fn default_energy() -> f64 { 500.0 }
fn default_duration() -> f64 { 2.0 }
fn default_iso() -> String { "ISO10218".to_string() }

impl ActuationSpec {
    /// Serialize as MCP Tool
    pub fn to_mcp_tool(&self) -> serde_json::Value {
        let required: Vec<&str> = self.parameters.iter()
            .filter(|p| p.required)
            .map(|p| p.name.as_str())
            .collect();

        let properties: serde_json::Map<String, serde_json::Value> = self.parameters.iter()
            .map(|p| {
                let mut prop = serde_json::json!({
                    "type": p.param_type,
                    "description": p.description
                });
                if let Some(def) = &p.default {
                    prop["default"] = def.clone();
                }
                if let Some(min) = p.minimum {
                    prop["minimum"] = min.into();
                }
                if let Some(max) = p.maximum {
                    prop["maximum"] = max.into();
                }
                (p.name.clone(), prop)
            })
            .collect();

        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
            "annotations": {
                "robot_id": self.robot_id,
                "category": self.category,
                "max_speed_m_s": self.max_speed_m_s,
                "max_force_n": self.max_force_n,
                "max_energy_j": self.max_energy_j,
                "est_duration_s": self.est_duration_s,
                "requires_lease": self.requires_lease,
                "shadow_required": self.shadow_required,
                "iso_class": self.iso_class,
                "protocol": "pmcp/0.5",
            }
        })
    }
}

/// Actuation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActuationResult {
    pub success: bool,
    #[serde(default)]
    pub robot_id: String,
    #[serde(default)]
    pub actuation_name: String,
    #[serde(default)]
    pub output: serde_json::Value,
    #[serde(default)]
    pub error_message: String,
    #[serde(default)]
    pub duration_s: f64,
    #[serde(default)]
    pub energy_consumed_j: f64,
    #[serde(default)]
    pub final_pose: Option<serde_json::Value>,
    #[serde(default)]
    pub shadow_delta_m: f64,
    #[serde(default)]
    pub timestamp: f64,
}

impl ActuationResult {
    pub fn success(output: serde_json::Value) -> Self {
        Self {
            success: true,
            output,
            timestamp: Utc::now().timestamp() as f64,
            ..Default::default()
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            error_message: message.into(),
            timestamp: Utc::now().timestamp() as f64,
            ..Default::default()
        }
    }

    pub fn to_mcp_content(&self) -> serde_json::Value {
        let mut body = serde_json::json!({
            "success": self.success,
            "robot_id": self.robot_id,
            "actuation": self.actuation_name,
            "output": self.output,
            "metrics": {
                "duration_s": self.duration_s,
                "energy_consumed_j": self.energy_consumed_j,
                "shadow_delta_m": self.shadow_delta_m,
            },
        });
        if !self.success {
            body["error"] = serde_json::json!(self.error_message);
        }
        if let Some(pose) = &self.final_pose {
            body["final_pose"] = pose.clone();
        }
        serde_json::json!([{"type": "text", "text": body.to_string()}])
    }
}

impl Default for ActuationResult {
    fn default() -> Self {
        Self {
            success: false,
            robot_id: String::new(),
            actuation_name: String::new(),
            output: serde_json::Value::Null,
            error_message: String::new(),
            duration_s: 0.0,
            energy_consumed_j: 0.0,
            final_pose: None,
            shadow_delta_m: 0.0,
            timestamp: Utc::now().timestamp() as f64,
        }
    }
}

// ============================================================================
// Sensor Types (MCP Resource equivalent)
// ============================================================================

/// Sensor type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorType {
    JointStates,
    EndEffector,
    ForceTorque,
    CameraRgb,
    CameraDepth,
    Lidar,
    Imu,
    Battery,
    Temperature,
    Proximity,
    Gps,
    Odometry,
    PlantHealth,
    EnergyMeter,
    Custom,
}

impl Default for SensorType {
    fn default() -> Self {
        SensorType::Custom
    }
}

impl SensorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SensorType::JointStates => "joint_states",
            SensorType::EndEffector => "end_effector",
            SensorType::ForceTorque => "force_torque",
            SensorType::CameraRgb => "camera_rgb",
            SensorType::CameraDepth => "camera_depth",
            SensorType::Lidar => "lidar",
            SensorType::Imu => "imu",
            SensorType::Battery => "battery",
            SensorType::Temperature => "temperature",
            SensorType::Proximity => "proximity",
            SensorType::Gps => "gps",
            SensorType::Odometry => "odometry",
            SensorType::PlantHealth => "plant_health",
            SensorType::EnergyMeter => "energy_meter",
            SensorType::Custom => "custom",
        }
    }
}

/// Sensor specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorSpec {
    pub name: String,
    pub description: String,
    pub robot_id: String,
    pub sensor_type: SensorType,
    #[serde(default)]
    pub unit: String,
    #[serde(default = "default_hz")]
    pub hz: f64,
    #[serde(default)]
    pub is_stream: bool,
}

fn default_hz() -> f64 { 10.0 }

impl SensorSpec {
    pub fn uri(&self) -> String {
        format!("pmcp://{}/sensors/{}", self.robot_id, self.name)
    }

    pub fn to_mcp_resource(&self) -> serde_json::Value {
        serde_json::json!({
            "uri": self.uri(),
            "name": self.name,
            "description": self.description,
            "mimeType": "application/json",
            "annotations": {
                "robot_id": self.robot_id,
                "sensor_type": self.sensor_type.as_str(),
                "unit": self.unit,
                "hz": self.hz,
                "is_stream": self.is_stream,
                "protocol": "pmcp/0.5",
            }
        })
    }
}

/// Sensor reading
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub sensor_name: String,
    pub robot_id: String,
    pub value: serde_json::Value,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub timestamp: f64,
    #[serde(default = "default_quality")]
    pub quality: f64,
}

fn default_quality() -> f64 { 1.0 }

impl SensorReading {
    pub fn new(sensor_name: impl Into<String>, robot_id: impl Into<String>, value: serde_json::Value) -> Self {
        Self {
            sensor_name: sensor_name.into(),
            robot_id: robot_id.into(),
            value,
            timestamp: Utc::now().timestamp() as f64,
            quality: 1.0,
            ..Default::default()
        }
    }

    pub fn to_mcp_content(&self) -> serde_json::Value {
        serde_json::json!([{
            "type": "text",
            "text": serde_json::json!({
                "sensor": self.sensor_name,
                "robot_id": self.robot_id,
                "value": self.value,
                "unit": self.unit,
                "timestamp": self.timestamp,
                "quality": self.quality,
            }).to_string()
        }])
    }
}

impl Default for SensorReading {
    fn default() -> Self {
        Self {
            sensor_name: String::new(),
            robot_id: String::new(),
            value: serde_json::Value::Null,
            unit: String::new(),
            timestamp: Utc::now().timestamp() as f64,
            quality: 1.0,
        }
    }
}

// ============================================================================
// Lease System (Temporal Zone Ownership)
// ============================================================================

/// Lease state enumeration
/// NOTE: values match pmcp-spec/schema/v0.6.0's LeaseState enum exactly
/// (FREE/PENDING/ACTIVE/EXPIRED/DENIED). Earlier code used a non-conformant
/// "GRANTED" value -- confirmed a drift bug (see pmcp-labs/v04, which
/// already used ACTIVE), not a deliberate naming choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LeaseState {
    Free,
    Pending,
    Active,
    Expired,
    Denied,
}

impl Default for LeaseState {
    fn default() -> Self {
        LeaseState::Active
    }
}

/// Lease request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub robot_id: String,
    pub zone_id: String,
    #[serde(default = "default_lease_ms")]
    pub duration_ms: u64,
    #[serde(default = "default_bid")]
    pub bid_energy_j: f64,
    #[serde(default = "default_priority")]
    pub priority: u8,
}

fn default_lease_ms() -> u64 { 10_000 }
fn default_bid() -> f64 { 100.0 }
fn default_priority() -> u8 { 5 }

/// Lease grant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseGrant {
    #[serde(default = "default_lease_id")]
    pub lease_id: String,
    #[serde(default)]
    pub robot_id: String,
    #[serde(default)]
    pub zone_id: String,
    #[serde(default)]
    pub state: LeaseState,
    #[serde(default)]
    pub expires_at: f64,
    #[serde(default)]
    pub bid_energy_j: f64,
    /// Monotonically increasing fencing token (Kleppmann 2016). Must be
    /// re-presented on every actuation call against this zone, not only
    /// at lease-request time. See LeaseManager::check in lease.rs.
    #[serde(default)]
    pub fence_token: u64,
}

fn default_lease_id() -> String { Uuid::new_v4().to_string()[..12].to_string() }

impl LeaseGrant {
    pub fn new(robot_id: impl Into<String>, zone_id: impl Into<String>, duration_ms: u64, bid_energy_j: f64, fence_token: u64) -> Self {
        Self {
            lease_id: Uuid::new_v4().to_string()[..12].to_string(),
            robot_id: robot_id.into(),
            zone_id: zone_id.into(),
            state: LeaseState::Active,
            expires_at: (Utc::now().timestamp() as f64) + (duration_ms as f64 / 1000.0),
            bid_energy_j,
            fence_token,
        }
    }

    pub fn denied(robot_id: impl Into<String>, zone_id: impl Into<String>) -> Self {
        Self {
            lease_id: String::new(),
            robot_id: robot_id.into(),
            zone_id: zone_id.into(),
            state: LeaseState::Denied,
            expires_at: 0.0,
            bid_energy_j: 0.0,
            fence_token: 0,
        }
    }

    pub fn remaining_ms(&self) -> f64 {
        let now = Utc::now().timestamp() as f64;
        if self.expires_at > now {
            (self.expires_at - now) * 1000.0
        } else {
            0.0
        }
    }

    pub fn is_valid(&self) -> bool {
        self.state == LeaseState::Active && self.expires_at > Utc::now().timestamp() as f64
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "lease_id": self.lease_id,
            "robot_id": self.robot_id,
            "zone_id": self.zone_id,
            "state": self.state,
            "expires_at": self.expires_at,
            "remaining_ms": self.remaining_ms(),
            "fence_token": self.fence_token,
        })
    }
}

impl Default for LeaseGrant {
    fn default() -> Self {
        Self {
            lease_id: Uuid::new_v4().to_string()[..12].to_string(),
            robot_id: String::new(),
            zone_id: String::new(),
            state: LeaseState::Active,
            expires_at: Utc::now().timestamp() as f64 + 10.0,
            bid_energy_j: 0.0,
            fence_token: 0,
        }
    }
}

// ============================================================================
// Capabilities
// ============================================================================

/// Server capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default = "default_true")]
    pub tools: bool,
    #[serde(default = "default_true")]
    pub resources: bool,
    #[serde(default = "default_true")]
    pub prompts: bool,
    #[serde(default = "default_true")]
    pub logging: bool,
    #[serde(default)]
    pub sampling: bool,
    #[serde(default = "default_true")]
    pub shadow: bool,
    #[serde(default = "default_true")]
    pub leases: bool,
    #[serde(default = "default_true")]
    pub estop: bool,
    #[serde(default = "default_true")]
    pub constitution: bool,
    #[serde(default)]
    pub streaming: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            tools: true,
            resources: true,
            prompts: true,
            logging: true,
            sampling: false,
            shadow: true,
            leases: true,
            estop: true,
            constitution: true,
            streaming: false,
        }
    }
}

impl Capabilities {
    pub fn to_mcp_dict(&self) -> serde_json::Value {
        let mut caps = serde_json::Map::new();
        if self.tools {
            caps.insert("tools".to_string(), serde_json::json!({"listChanged": true}));
        }
        if self.resources {
            caps.insert("resources".to_string(), serde_json::json!({
                "subscribe": self.streaming,
                "listChanged": true
            }));
        }
        if self.prompts {
            caps.insert("prompts".to_string(), serde_json::json!({"listChanged": false}));
        }
        if self.logging {
            caps.insert("logging".to_string(), serde_json::json!({}));
        }
        if self.sampling {
            caps.insert("sampling".to_string(), serde_json::json!({}));
        }
        caps.insert("experimental".to_string(), serde_json::json!({
            "pmcp": {
                "version": PMCP_VERSION,
                "shadow": self.shadow,
                "leases": self.leases,
                "estop": self.estop,
                "constitution": self.constitution,
            }
        }));
        serde_json::Value::Object(caps)
    }
}

// ============================================================================
// Shadow Preview
// ============================================================================

/// Shadow status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ShadowStatus {
    Safe,
    Collision,
    JointLimit,
    WorkspaceViolation,
    SpeedExceeded,
    EnergyExceeded,
    Simulated,
    Skipped,
}

impl Default for ShadowStatus {
    fn default() -> Self {
        ShadowStatus::Simulated
    }
}

/// Shadow preview result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowPreview {
    pub actuation_name: String,
    pub arguments: serde_json::Value,
    pub status: ShadowStatus,
    pub safe: bool,
    #[serde(default)]
    pub risk_score: f64,
    #[serde(default)]
    pub sim_duration_s: f64,
    #[serde(default)]
    pub est_duration_s: f64,
    #[serde(default)]
    pub est_energy_j: f64,
    #[serde(default)]
    pub collision_body: String,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default)]
    pub timestamp: f64,
}

fn default_engine() -> String { "geometric".to_string() }

impl Default for ShadowPreview {
    fn default() -> Self {
        Self {
            actuation_name: String::new(),
            arguments: serde_json::Value::Null,
            status: ShadowStatus::Simulated,
            safe: true,
            risk_score: 0.0,
            sim_duration_s: 0.0,
            est_duration_s: 0.0,
            est_energy_j: 0.0,
            collision_body: String::new(),
            warnings: Vec::new(),
            engine: "geometric".to_string(),
            timestamp: Utc::now().timestamp() as f64,
        }
    }
}

impl ShadowPreview {
    pub fn safe(actuation_name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            actuation_name: actuation_name.into(),
            arguments,
            status: ShadowStatus::Safe,
            safe: true,
            ..Default::default()
        }
    }

    pub fn blocked(actuation_name: impl Into<String>, arguments: serde_json::Value,
                   status: ShadowStatus, warnings: Vec<String>) -> Self {
        Self {
            actuation_name: actuation_name.into(),
            arguments,
            status,
            safe: false,
            warnings,
            ..Default::default()
        }
    }

    /// schema/v0.6.0 ShadowResult.verdict (PASS/CONDITIONAL_PASS/FAIL/INDETERMINATE),
    /// derived from status+safe. See pmcp/types.py's ShadowVerdict for the
    /// matching Python logic and rationale (confidence/monitoring/determinism
    /// blocks require the HNN-Simplex monitor -- not implemented anywhere in
    /// this org yet, so this SDK never fabricates them; verdict is derived
    /// from the physics/geometric simulator's status instead).
    pub fn verdict(&self) -> &'static str {
        match (self.safe, self.status) {
            (true, ShadowStatus::Safe) => "PASS",
            (_, ShadowStatus::Simulated) | (_, ShadowStatus::Skipped) => "INDETERMINATE",
            _ => "FAIL",
        }
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            // schema/v0.6.0 ShadowResult core fields (canonical, snake_case):
            "verdict": self.verdict(),
            "predicted_trajectory": serde_json::Value::Null,
            "confidence": serde_json::Value::Null,
            "monitoring": serde_json::Value::Null,
            "determinism": serde_json::Value::Null,
            // Additional fields -- real, working output from this SDK's
            // physics/geometric simulator.
            "actuation_name": self.actuation_name,
            "status": self.status,
            "safe": self.safe,
            "risk_score": self.risk_score,
            "est_duration_s": self.est_duration_s,
            "est_energy_j": self.est_energy_j,
            "warnings": self.warnings,
            "engine": self.engine,
            "collision_body": self.collision_body,
        })
    }
}

// ============================================================================
// Initialize Response Types
// ============================================================================

/// Server info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String { "1.0.0".to_string() }

impl Default for ServerInfo {
    fn default() -> Self {
        Self {
            name: "pmcp-server".to_string(),
            version: "1.0.0".to_string(),
        }
    }
}

/// Client info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
}

// ============================================================================
// Server Status
// ============================================================================

/// Server status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub robot_id: String,
    pub pmcp_version: String,
    #[serde(rename = "uptimeS")]
    pub uptime_s: f64,
    #[serde(rename = "callCount")]
    pub call_count: u64,
    #[serde(rename = "blockedCount")]
    pub blocked_count: u64,
    #[serde(default)]
    pub safety_stats: serde_json::Value,
    #[serde(default)]
    pub actuations: Vec<String>,
    #[serde(default)]
    pub sensors: Vec<String>,
    #[serde(default)]
    pub missions: Vec<String>,
}

impl Default for ServerStatus {
    fn default() -> Self {
        Self {
            robot_id: String::new(),
            pmcp_version: PMCP_VERSION.to_string(),
            uptime_s: 0.0,
            call_count: 0,
            blocked_count: 0,
            safety_stats: serde_json::Value::Null,
            actuations: Vec::new(),
            sensors: Vec::new(),
            missions: Vec::new(),
        }
    }
}