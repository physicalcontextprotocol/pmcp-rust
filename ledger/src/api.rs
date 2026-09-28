use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use async_trait::async_trait;

use crate::crdt::{RobotStateCRDT, DeltaMutator, RobotState, Position3D, Velocity3D, Orientation, NodeId};
use crate::state::{ZoneState, ZoneBounds, FleetSnapshot, RobotRegistration};
use crate::partition::{ZonePartition, PartitionKey, PartitionStats};
use crate::sync::{GossipProtocol, Peer, SyncConfig, SyncMetrics};
use crate::storage::{StorageBackend, InMemoryBackend};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerConfig {
    pub node_id: String,
    pub bind_address: String,
    pub bind_port: u16,
    pub num_zones: usize,
    pub shards_per_zone: u32,
    pub redis_url: Option<String>,
    pub etcd_endpoints: Vec<String>,
}

impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            node_id: uuid::Uuid::new_v4().to_string(),
            bind_address: "0.0.0.0".to_string(),
            bind_port: 8082,
            num_zones: 4,
            shards_per_zone: 4,
            redis_url: None,
            etcd_endpoints: Vec::new(),
        }
    }
}

pub struct LedgerAPI {
    config: LedgerConfig,
    node_id: NodeId,
    partitions: RwLock<HashMap<String, ZonePartition>>,
    gossip: RwLock<Option<GossipProtocol>>,
    storage: Box<dyn StorageBackend>,
    metrics: RwLock<LedgerMetrics>,
    crdt: RwLock<HashMap<String, RobotStateCRDT>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LedgerMetrics {
    pub total_updates: u64,
    pub total_robots: u64,
    pub total_zones: u64,
    pub sync_messages: u64,
    pub last_update_timestamp: Option<u64>,
}

impl LedgerAPI {
    pub fn new(config: LedgerConfig) -> Self {
        let node_id = NodeId::new(config.node_id.clone());
        let storage = Box::new(InMemoryBackend::new(node_id.clone()));

        Self {
            config,
            node_id,
            partitions: RwLock::new(HashMap::new()),
            gossip: RwLock::new(None),
            storage,
            metrics: RwLock::new(LedgerMetrics::default()),
            crdt: RwLock::new(HashMap::new()),
        }
    }

    pub async fn initialize(&self) -> Result<(), String> {
        let default_bounds = ZoneBounds::new(0.0, 100.0, 0.0, 100.0, 0.0, 10.0);

        for i in 0..self.config.num_zones {
            let zone_id = format!("zone{}", i);
            let partition = ZonePartition::new(
                zone_id.clone(),
                default_bounds.clone(),
                self.config.shards_per_zone,
            );
            self.partitions.write().await.insert(zone_id, partition);
        }

        let mut gossip = GossipProtocol::new(self.node_id.clone(), 3, 100);
        let partition_keys: Vec<PartitionKey> = (0..self.config.num_zones)
            .flat_map(|i| {
                (0..self.config.shards_per_zone).map(move |s| {
                    PartitionKey::new(format!("zone{}", i), s)
                })
            })
            .collect();

        for key in partition_keys {
            gossip.register_partition(key);
        }

        *self.gossip.write().await = Some(gossip);

        Ok(())
    }

    pub async fn register_robot(&self, registration: RobotRegistration) -> Result<String, String> {
        let robot_id = registration.robot_id.clone();
        let zone_id = registration.initial_zone.clone();

        let position = Position3D::new(0.0, 0.0, 0.0);

        let mut partitions = self.partitions.write().await;
        if let Some(partition) = partitions.get_mut(&zone_id) {
            partition.assign_robot(robot_id.clone(), &position);
        }

        let mut crdt_map = self.crdt.write().await;
        let crdt = crdt_map.entry(zone_id.clone()).or_insert_with(|| {
            RobotStateCRDT::new(self.node_id.clone())
        });

        crdt.apply_delta(DeltaMutator::UpdatePosition {
            robot_id: robot_id.clone(),
            position,
            timestamp: chrono::Utc::now(),
            zone: zone_id,
        });

        let mut metrics = self.metrics.write().await;
        metrics.total_robots += 1;
        metrics.total_updates += 1;

        Ok(robot_id)
    }

    pub async fn update_robot_position(
        &self,
        robot_id: String,
        x: f64,
        y: f64,
        z: f64,
    ) -> Result<(), String> {
        let position = Position3D::new(x, y, z);

        let mut partitions = self.partitions.write().await;
        for partition in partitions.values_mut() {
            if partition.robots.contains_key(&robot_id) {
                partition.assign_robot(robot_id.clone(), &position);
                break;
            }
        }

        let delta = DeltaMutator::UpdatePosition {
            robot_id: robot_id.clone(),
            position,
            timestamp: chrono::Utc::now(),
            zone: "default".to_string(),
        };

        let mut crdt_map = self.crdt.write().await;
        for crdt in crdt_map.values_mut() {
            crdt.apply_delta(delta.clone());
        }

        let mut metrics = self.metrics.write().await;
        metrics.total_updates += 1;
        metrics.last_update_timestamp = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );

        Ok(())
    }

    pub async fn update_robot_velocity(
        &self,
        robot_id: String,
        vx: f64,
        vy: f64,
        vz: f64,
        omega: f64,
    ) -> Result<(), String> {
        let velocity = Velocity3D::new(vx, vy, vz, omega);

        let delta = DeltaMutator::UpdateVelocity {
            robot_id,
            velocity,
            timestamp: chrono::Utc::now(),
        };

        let mut crdt_map = self.crdt.write().await;
        for crdt in crdt_map.values_mut() {
            crdt.apply_delta(delta.clone());
        }

        Ok(())
    }

    pub async fn get_robot_state(&self, robot_id: &str) -> Option<RobotState> {
        let crdt_map = self.crdt.read().await;
        for crdt in crdt_map.values() {
            if let Some(state) = crdt.get_state(robot_id) {
                return Some(state.clone());
            }
        }
        None
    }

    pub async fn get_zone_state(&self, zone_id: &str) -> Option<ZoneState> {
        let partitions = self.partitions.read().await;
        let partition = partitions.get(zone_id)?;

        let crdt_map = self.crdt.read().await;
        let crdt = crdt_map.get(zone_id)?;

        let states: Vec<RobotState> = crdt
            .get_states_in_zone(zone_id)
            .into_iter()
            .map(|s| s.clone())
            .collect();

        let mut zone_state = ZoneState::new(zone_id.to_string(), ZoneBounds::default());
        for state in states {
            zone_state.add_robot(state);
        }

        Some(zone_state)
    }

    pub async fn get_fleet_snapshot(&self) -> FleetSnapshot {
        let partitions = self.partitions.read().await;
        let zones: Vec<ZoneState> = partitions
            .values()
            .map(|p| {
                let mut zs = ZoneState::new(p.zone_id.clone(), ZoneBounds::default());
                for robot_id in p.robots.keys() {
                    if let Some(state) = self.get_robot_state(robot_id).await {
                        zs.add_robot(state);
                    }
                }
                zs
            })
            .collect();

        FleetSnapshot::from_zones(zones)
    }

    pub async fn get_partition_stats(&self, zone_id: &str) -> Option<PartitionStats> {
        let partitions = self.partitions.read().await;
        partitions.get(zone_id).map(|p| PartitionStats::analyze(p))
    }

    pub async fn get_metrics(&self) -> LedgerMetrics {
        self.metrics.read().await.clone()
    }

    pub async fn add_peer(&self, address: String, port: u16) -> Result<(), String> {
        let peer_id = format!("{}:{}", address, port);
        let peer = Peer::new(NodeId::new(peer_id), address, port);

        let mut gossip_guard = self.gossip.write().await;
        if let Some(gossip) = gossip_guard.as_mut() {
            gossip.add_peer(peer);
        }

        Ok(())
    }

    pub async fn trigger_sync(&self) -> Result<(), String> {
        let gossip_guard = self.gossip.read().await;
        if let Some(gossip) = gossip_guard.as_ref() {
            let targets = gossip.select_targets();
            tracing::info!("Syncing with {} peers", targets.len());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ledger_initialization() {
        let config = LedgerConfig::default();
        let ledger = LedgerAPI::new(config);
        ledger.initialize().await.unwrap();

        let metrics = ledger.get_metrics().await;
        assert_eq!(metrics.total_zones, 4);
    }

    #[tokio::test]
    async fn test_robot_registration() {
        let config = LedgerConfig::default();
        let ledger = LedgerAPI::new(config);
        ledger.initialize().await.unwrap();

        let registration = RobotRegistration::new(
            "robot1".to_string(),
            "Test Robot".to_string(),
            "turtlebot".to_string(),
        );
        let robot_id = ledger.register_robot(registration).await.unwrap();

        assert_eq!(robot_id, "robot1");
    }
}