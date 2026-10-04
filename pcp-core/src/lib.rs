//! PCP Core - Physical Context Protocol Rust Implementation
//!
//! This is the core Rust implementation of PCP v0.5, providing:
//! - Full MCP wire protocol compatibility
//! - JSON-RPC 2.0 message handling
//! - Robot actuation and sensor management
//! - Safety pipeline (constitution -> shadow -> execute)
//! - Temporal lease system for zone coordination
//! - Transport layer (stdio, HTTP, WebSocket, TCP)
//! - Security module (DID, signatures, mTLS)
//! - State management
//! - Metrics and monitoring
//! - Plugin system
//! - Workflow engine
//! - REST API server

pub mod types;
pub mod server;
pub mod methods;
pub mod lease;
pub mod safety;
pub mod error;
pub mod transport;
pub mod security;
pub mod state;
pub mod metrics;
pub mod plugin;
pub mod workflow;
pub mod api;
pub mod consensus;
pub mod network;
pub mod rate_limit;
pub mod telemetry;
pub mod identity;

#[cfg(feature = "python")]
pub mod python;

pub use types::*;
pub use server::{PCPServer, PCPServerBuilder};
pub use error::{PcpError, PcpErrorCode};
pub use consensus::{RaftNode, RaftConfig, FleetStateMachine, LogCommand, ConsensusCluster};
pub use network::{
    Transport, StdioTransport, TcpServerTransport, TcpClientTransport, ConnectionPool,
};
pub use rate_limit::{ActuationRateLimiter, EnergyBudgetTracker, PriorityScheduler};
pub use telemetry::{TelemetryPipeline, MetricsRegistry, EventLog, StructuredEvent};
pub use identity::{Did, DidDocument, RobotIdentity, CapabilityToken, DidRegistry, ZkSafetyProver};