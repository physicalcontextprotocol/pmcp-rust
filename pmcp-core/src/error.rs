//! P-MCP Error Types

use serde::{Deserialize, Serialize};

/// P-MCP Error Codes (JSON-RPC standard + P-MCP physical extensions)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum PmcpErrorCode {
    // JSON-RPC 2.0 standard
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,

    // P-MCP Physical Safety (-33000 range)
    ShadowBlocked = -33001,
    ConstitutionBlocked = -33002,
    LeaseRequired = -33003,
    LeaseExpired = -33004,
    EstopActive = -33005,
    FloorGuard = -33006,
    SpeedLimit = -33007,
    EnergyBudget = -33008,
    HumanProximity = -33009,
    ZkProofInvalid = -33010,
    JointLimit = -33011,
    TorqueLimit = -33012,
    WorkspaceViolation = -33013,
    CollisionDetected = -33014,
    RobotFault = -33015,
    // Identity/crypto (DID, capability tokens -- see identity.rs)
    SignatureInvalid = -33016,
    TokenExpired = -33017,
    NotFound = -33018,
    RateLimited = -33019,
}

impl PmcpErrorCode {
    pub fn code(&self) -> i32 {
        *self as i32
    }

    pub fn message(&self) -> &'static str {
        match self {
            PmcpErrorCode::ParseError => "Parse error",
            PmcpErrorCode::InvalidRequest => "Invalid Request",
            PmcpErrorCode::MethodNotFound => "Method not found",
            PmcpErrorCode::InvalidParams => "Invalid params",
            PmcpErrorCode::InternalError => "Internal error",
            PmcpErrorCode::ShadowBlocked => "Shadow validator rejected trajectory",
            PmcpErrorCode::ConstitutionBlocked => "Safety constitution rule violated",
            PmcpErrorCode::LeaseRequired => "No valid lease for this zone",
            PmcpErrorCode::LeaseExpired => "Lease expired mid-execution",
            PmcpErrorCode::EstopActive => "Emergency stop is active",
            PmcpErrorCode::FloorGuard => "Z target below floor limit",
            PmcpErrorCode::SpeedLimit => "Speed exceeds hard limit",
            PmcpErrorCode::EnergyBudget => "Energy budget exhausted",
            PmcpErrorCode::HumanProximity => "Human too close to workspace",
            PmcpErrorCode::ZkProofInvalid => "ZK safety proof failed",
            PmcpErrorCode::JointLimit => "Joint position/velocity out of range",
            PmcpErrorCode::TorqueLimit => "Torque exceeds hardware limit",
            PmcpErrorCode::WorkspaceViolation => "Target outside defined workspace",
            PmcpErrorCode::CollisionDetected => "Shadow simulation found collision",
            PmcpErrorCode::RobotFault => "Hardware fault state",
            PmcpErrorCode::SignatureInvalid => "Signature verification failed",
            PmcpErrorCode::TokenExpired => "Capability token has expired",
            PmcpErrorCode::NotFound => "Requested entity not found",
            PmcpErrorCode::RateLimited => "Rate limit exceeded",
        }
    }
}

impl std::fmt::Display for PmcpErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

/// P-MCP Error structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PmcpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl PmcpError {
    pub fn new(code: PmcpErrorCode) -> Self {
        Self {
            code: code.code(),
            message: code.message().to_string(),
            data: None,
        }
    }

    pub fn with_message(code: PmcpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.code(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(code: PmcpErrorCode, data: serde_json::Value) -> Self {
        Self {
            code: code.code(),
            message: code.message().to_string(),
            data: Some(data),
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::with_message(PmcpErrorCode::MethodNotFound, format!("Method not found: {}", method))
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::with_message(PmcpErrorCode::InvalidParams, msg)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::with_message(PmcpErrorCode::InternalError, msg)
    }
}

impl std::fmt::Display for PmcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PmcpError {}

impl From<serde_json::Error> for PmcpError {
    fn from(e: serde_json::Error) -> Self {
        Self::with_message(PmcpErrorCode::ParseError, e.to_string())
    }
}

impl From<serde_json::Error> for PmcpErrorCode {
    fn from(_: serde_json::Error) -> Self {
        PmcpErrorCode::ParseError
    }
}