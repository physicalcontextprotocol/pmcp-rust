//! Minimal P-MCP server and client, in two processes.
//!
//! ```text
//! cargo run --example server_and_client -- server   # terminal 1
//! cargo run --example server_and_client             # terminal 2
//! ```

use physicalcontextprotocol::{
    ActuationSpec, PMCPServer, PMCPServerBuilder, TcpClientTransport, Transport,
};
use std::error::Error;
use std::net::SocketAddr;

fn actuation_spec() -> ActuationSpec {
    ActuationSpec {
        name: "move_to".into(),
        description: "Move the end effector to a Cartesian target".into(),
        parameters: vec![],
        robot_id: "ur5-arm-01".into(),
        category: "motion".into(),
        max_speed_m_s: 1.0,
        max_force_n: 100.0,
        max_energy_j: 500.0,
        est_duration_s: 2.0,
        requires_lease: true,
        shadow_required: true,
        iso_class: "ISO 10218-1".into(),
    }
}

/// Server process. `run_stdio()` speaks MCP over stdin/stdout; `run_http(host,
/// port)` is the TCP variant a `TcpClientTransport` connects to.
async fn run_server() -> Result<(), Box<dyn Error>> {
    let server: PMCPServer = PMCPServerBuilder::new("ur5-arm-01")
        .version("1.0.0")
        .robot_id("ur5-arm-01")
        .actuation(actuation_spec())
        .build();

    server.run_stdio().await;
    Ok(())
}

/// Client process. The crate exposes transports rather than a high-level
/// client, so you drive the JSON-RPC exchange over the returned channels.
async fn run_client() -> Result<(), Box<dyn Error>> {
    let addr: SocketAddr = "127.0.0.1:7000".parse()?;
    let transport = TcpClientTransport::new(addr);
    let (outbound, mut inbound) = transport.connect().await?;

    outbound
        .send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "0.5" }
        }))
        .await?;

    if let Some(message) = inbound.recv().await {
        println!("{message}");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("server") => run_server().await?,
        _ => run_client().await?,
    }
    Ok(())
}
