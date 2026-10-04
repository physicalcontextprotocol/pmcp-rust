//! PCP Error Types

use serde::{Deserialize, Serialize};

/// PCP Error Codes (JSON-RPC standard + PCP physical extensions)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum PcpErrorCode {
    // JSON-RPC 2.0 standard
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,

    // PCP Physical Safety (-33000 range)
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

impl PcpErrorCode {
    pub fn code(&self) -> i32 {
        *self as i32
    }

    pub fn message(&self) -> &'static str {
        match self {
            PcpErrorCode::ParseError => "Parse error",
            PcpErrorCode::InvalidRequest => "Invalid Request",
            PcpErrorCode::MethodNotFound => "Method not found",
            PcpErrorCode::InvalidParams => "Invalid params",
            PcpErrorCode::InternalError => "Internal error",
            PcpErrorCode::ShadowBlocked => "Shadow validator rejected trajectory",
            PcpErrorCode::ConstitutionBlocked => "Safety constitution rule violated",
            PcpErrorCode::LeaseRequired => "No valid lease for this zone",
            PcpErrorCode::LeaseExpired => "Lease expired mid-execution",
            PcpErrorCode::EstopActive => "Emergency stop is active",
            PcpErrorCode::FloorGuard => "Z target below floor limit",
            PcpErrorCode::SpeedLimit => "Speed exceeds hard limit",
            PcpErrorCode::EnergyBudget => "Energy budget exhausted",
            PcpErrorCode::HumanProximity => "Human too close to workspace",
            PcpErrorCode::ZkProofInvalid => "ZK safety proof failed",
            PcpErrorCode::JointLimit => "Joint position/velocity out of range",
            PcpErrorCode::TorqueLimit => "Torque exceeds hardware limit",
            PcpErrorCode::WorkspaceViolation => "Target outside defined workspace",
            PcpErrorCode::CollisionDetected => "Shadow simulation found collision",
            PcpErrorCode::RobotFault => "Hardware fault state",
            PcpErrorCode::SignatureInvalid => "Signature verification failed",
            PcpErrorCode::TokenExpired => "Capability token has expired",
            PcpErrorCode::NotFound => "Requested entity not found",
            PcpErrorCode::RateLimited => "Rate limit exceeded",
        }
    }
}

impl std::fmt::Display for PcpErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

/// PCP Error structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl PcpError {
    pub fn new(code: PcpErrorCode) -> Self {
        Self {
            code: code.code(),
            message: code.message().to_string(),
            data: None,
        }
    }

    pub fn with_message(code: PcpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.code(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(code: PcpErrorCode, data: serde_json::Value) -> Self {
        Self {
            code: code.code(),
            message: code.message().to_string(),
            data: Some(data),
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::with_message(PcpErrorCode::MethodNotFound, format!("Method not found: {}", method))
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::with_message(PcpErrorCode::InvalidParams, msg)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::with_message(PcpErrorCode::InternalError, msg)
    }
}

impl std::fmt::Display for PcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PcpError {}

impl From<serde_json::Error> for PcpError {
    fn from(e: serde_json::Error) -> Self {
        Self::with_message(PcpErrorCode::ParseError, e.to_string())
    }
}

impl From<serde_json::Error> for PcpErrorCode {
    fn from(_: serde_json::Error) -> Self {
        PcpErrorCode::ParseError
    }
}