use clap::{Parser, ValueEnum};
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use std::sync::Arc;

use pcp_ledger::{LedgerAPI, LedgerConfig};

#[derive(Parser, Debug)]
#[command(name = "pcp-ledger")]
#[command(about = "PCP Distributed Digital Twin Ledger", long_about = None)]
struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    #[arg(short, long, default_value_t = 8082)]
    port: u16,

    #[arg(long)]
    node_id: Option<String>,

    #[arg(long, default_value_t = 4)]
    zones: usize,

    #[arg(long, default_value_t = 4)]
    shards: u32,

    #[arg(long)]
    redis_url: Option<String>,

    #[arg(long, value_delimiter = ',')]
    etcd_endpoints: Vec<String>,

    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let log_level = match args.log_level.to_lowercase().as_str() {
        "debug" => tracing::Level::DEBUG,
        "info" => tracing::Level::INFO,
        "warn" => tracing::Level::WARN,
        "error" => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::new(format!(
            "pcp_ledger={},{}",
            log_level,
            if log_level == tracing::Level::DEBUG {
                "debug"
            } else {
                "info"
            }
        )))
        .init();

    tracing::info!("Starting PCP Ledger - Distributed Digital Twin");

    let config = LedgerConfig {
        node_id: args.node_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        bind_address: args.host.clone(),
        bind_port: args.port,
        num_zones: args.zones,
        shards_per_zone: args.shards,
        redis_url: args.redis_url,
        etcd_endpoints: args.etcd_endpoints,
    };

    let ledger = Arc::new(LedgerAPI::new(config));
    ledger.initialize().await?;

    tracing::info!(
        "Ledger initialized with {} zones, {} shards per zone",
        args.zones,
        args.shards
    );

    let addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&addr).await?;

    tracing::info!("Ledger listening on {}", addr);

    loop {
        let (mut socket, peer_addr) = listener.accept().await?;

        let ledger = Arc::clone(&ledger);

        tokio::spawn(async move {
            tracing::debug!("Connection from {}", peer_addr);

            let mut buf = [0u8; 4096];

            match socket.read(&mut buf).await {
                Ok(0) => {
                    tracing::debug!("Client disconnected");
                }
                Ok(n) => {
                    let request = String::from_utf8_lossy(&buf[..n]);
                    tracing::info!("Received: {}", request.trim());

                    let response = handle_request(&ledger, &request).await;

                    if let Err(e) = socket.write_all(response.as_bytes()).await {
                        tracing::error!("Write error: {}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Read error: {}", e);
                }
            }
        });
    }
}

async fn handle_request(ledger: &LedgerAPI, request: &str) -> String {
    let parts: Vec<&str> = request.trim().split_whitespace().collect();

    if parts.is_empty() {
        return "ERROR: Empty request".to_string();
    }

    match parts[0].to_uppercase().as_str() {
        "REGISTER" => {
            if parts.len() >= 4 {
                let robot_id = parts[1].to_string();
                let name = parts[2].to_string();
                let robot_type = parts[3].to_string();

                let registration = pcp_ledger::state::RobotRegistration::new(robot_id, name, robot_type);

                match ledger.register_robot(registration).await {
                    Ok(id) => format!("OK:{}", id),
                    Err(e) => format!("ERROR:{}", e),
                }
            } else {
                "ERROR: Invalid REGISTER format".to_string()
            }
        }
        "UPDATE" => {
            if parts.len() >= 5 {
                let robot_id = parts[1].to_string();
                let x: f64 = parts[2].parse().unwrap_or(0.0);
                let y: f64 = parts[3].parse().unwrap_or(0.0);
                let z: f64 = parts[4].parse().unwrap_or(0.0);

                match ledger.update_robot_position(robot_id, x, y, z).await {
                    Ok(_) => "OK".to_string(),
                    Err(e) => format!("ERROR:{}", e),
                }
            } else {
                "ERROR: Invalid UPDATE format".to_string()
            }
        }
        "GET" => {
            if parts.len() >= 2 {
                let robot_id = parts[1];

                match ledger.get_robot_state(robot_id).await {
                    Some(state) => {
                        format!(
                            "OK:{},{},{},{},{},{},{},{}",
                            state.robot_id,
                            state.position.x,
                            state.position.y,
                            state.position.z,
                            state.velocity.vx,
                            state.velocity.vy,
                            state.velocity.vz,
                            state.zone
                        )
                    }
                    None => "NOT_FOUND".to_string(),
                }
            } else {
                "ERROR: Invalid GET format".to_string()
            }
        }
        "SNAPSHOT" => {
            let snapshot = ledger.get_fleet_snapshot().await;
            format!(
                "OK:zones={},robots={}",
                snapshot.zones.len(),
                snapshot.total_robots
            )
        }
        "METRICS" => {
            let metrics = ledger.get_metrics().await;
            format!(
                "OK:updates={},robots={},zones={},sync={}",
                metrics.total_updates,
                metrics.total_robots,
                metrics.total_zones,
                metrics.sync_messages
            )
        }
        "STATS" => {
            if parts.len() >= 2 {
                let zone_id = parts[1];
                match ledger.get_partition_stats(zone_id).await {
                    Some(stats) => format!(
                        "OK:zone={},robots={},shards={},rebalance={}",
                        stats.zone_id,
                        stats.total_robots,
                        stats.shards.len(),
                        stats.rebalancing_needed
                    ),
                    None => "NOT_FOUND".to_string(),
                }
            } else {
                "ERROR: Invalid STATS format".to_string()
            }
        }
        _ => format!("ERROR: Unknown command: {}", parts[0]),
    }
}