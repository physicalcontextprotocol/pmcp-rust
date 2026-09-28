use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

use crate::crdt::{RobotStateCRDT, DeltaMutator, RobotState, VersionVector, NodeId};
use crate::state::{ZoneState, ZoneBounds, FleetSnapshot};

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Conflict: {0}")]
    Conflict(String),
    #[error("Timeout: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn connect(&mut self) -> Result<()>;
    async fn disconnect(&mut self) -> Result<()>;

    async fn save_robot_state(&mut self, robot_id: &str, state: &RobotState) -> Result<()>;
    async fn get_robot_state(&mut self, robot_id: &str) -> Result<Option<RobotState>>;
    async fn delete_robot_state(&mut self, robot_id: &str) -> Result<()>;
    async fn list_robots(&mut self) -> Result<Vec<String>>;

    async fn save_zone(&mut self, zone_id: &str, state: &ZoneState) -> Result<()>;
    async fn get_zone(&mut self, zone_id: &str) -> Result<Option<ZoneState>>;
    async fn list_zones(&mut self) -> Result<Vec<String>>;

    async fn append_delta(&mut self, robot_id: &str, delta: &DeltaMutator) -> Result<()>;
    async fn get_deltas(&mut self, robot_id: &str, since_version: u64) -> Result<Vec<DeltaMutator>>;

    async fn save_version_vector(&mut self, robot_id: &str, vv: &VersionVector) -> Result<()>;
    async fn get_version_vector(&mut self, robot_id: &str) -> Result<Option<VersionVector>>;

    async fn publish_state_update(&mut self, robot_id: &str, state: &RobotState) -> Result<()>;
    async fn subscribe_state_updates(&mut self) -> Result<Box<dyn StateUpdateSubscriber>>;
}

pub trait StateUpdateSubscriber: Send {
    fn receive(&self) -> Option<(String, RobotState)>;
}

pub struct InMemoryBackend {
    states: HashMap<String, RobotState>,
    zones: HashMap<String, ZoneState>,
    deltas: HashMap<String, Vec<DeltaMutator>>,
    version_vectors: HashMap<String, VersionVector>,
    node_id: NodeId,
}

impl InMemoryBackend {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            states: HashMap::new(),
            zones: HashMap::new(),
            deltas: HashMap::new(),
            version_vectors: HashMap::new(),
            node_id,
        }
    }
}

#[async_trait]
impl StorageBackend for InMemoryBackend {
    async fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn save_robot_state(&mut self, robot_id: &str, state: &RobotState) -> Result<()> {
        self.states.insert(robot_id.to_string(), state.clone());
        Ok(())
    }

    async fn get_robot_state(&mut self, robot_id: &str) -> Result<Option<RobotState>> {
        Ok(self.states.get(robot_id).cloned())
    }

    async fn delete_robot_state(&mut self, robot_id: &str) -> Result<()> {
        self.states.remove(robot_id);
        Ok(())
    }

    async fn list_robots(&mut self) -> Result<Vec<String>> {
        Ok(self.states.keys().cloned().collect())
    }

    async fn save_zone(&mut self, zone_id: &str, state: &ZoneState) -> Result<()> {
        self.zones.insert(zone_id.to_string(), state.clone());
        Ok(())
    }

    async fn get_zone(&mut self, zone_id: &str) -> Result<Option<ZoneState>> {
        Ok(self.zones.get(zone_id).cloned())
    }

    async fn list_zones(&mut self) -> Result<Vec<String>> {
        Ok(self.zones.keys().cloned().collect())
    }

    async fn append_delta(&mut self, robot_id: &str, delta: &DeltaMutator) -> Result<()> {
        self.deltas.entry(robot_id.to_string()).or_insert_with(Vec::new).push(delta.clone());
        Ok(())
    }

    async fn get_deltas(&mut self, robot_id: &str, since_version: u64) -> Result<Vec<DeltaMutator>> {
        Ok(self.deltas.get(robot_id).map(|v| v.clone()).unwrap_or_default())
    }

    async fn save_version_vector(&mut self, robot_id: &str, vv: &VersionVector) -> Result<()> {
        self.version_vectors.insert(robot_id.to_string(), vv.clone());
        Ok(())
    }

    async fn get_version_vector(&mut self, robot_id: &str) -> Result<Option<VersionVector>> {
        Ok(self.version_vectors.get(robot_id).cloned())
    }

    async fn publish_state_update(&mut self, _robot_id: &str, _state: &RobotState) -> Result<()> {
        Ok(())
    }

    async fn subscribe_state_updates(&mut self) -> Result<Box<dyn StateUpdateSubscriber>> {
        struct DummySubscriber;
        impl StateUpdateSubscriber for DummySubscriber {
            fn receive(&self) -> Option<(String, RobotState)> { None }
        }
        Ok(Box::new(DummySubscriber))
    }
}

pub struct RedisBackend {
    connection_string: String,
    prefix: String,
    node_id: NodeId,
}

impl RedisBackend {
    pub fn new(connection_string: String, node_id: NodeId) -> Self {
        Self {
            connection_string,
            prefix: "pmcp:ledger:".to_string(),
            node_id,
        }
    }
}

#[async_trait]
impl StorageBackend for RedisBackend {
    async fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn save_robot_state(&mut self, robot_id: &str, state: &RobotState) -> Result<()> {
        let key = format!("{}robot:{}", self.prefix, robot_id);
        let data = serde_json::to_string(state).map_err(|e| StorageError::Serialization(e.to_string()))?;
        Ok(())
    }

    async fn get_robot_state(&mut self, robot_id: &str) -> Result<Option<RobotState>> {
        let key = format!("{}robot:{}", self.prefix, robot_id);
        Ok(None)
    }

    async fn delete_robot_state(&mut self, robot_id: &str) -> Result<()> {
        let key = format!("{}robot:{}", self.prefix, robot_id);
        Ok(())
    }

    async fn list_robots(&mut self) -> Result<Vec<String>> {
        let pattern = format!("{}robot:*", self.prefix);
        Ok(Vec::new())
    }

    async fn save_zone(&mut self, zone_id: &str, state: &ZoneState) -> Result<()> {
        let key = format!("{}zone:{}", self.prefix, zone_id);
        let data = serde_json::to_string(state).map_err(|e| StorageError::Serialization(e.to_string()))?;
        Ok(())
    }

    async fn get_zone(&mut self, zone_id: &str) -> Result<Option<ZoneState>> {
        let key = format!("{}zone:{}", self.prefix, zone_id);
        Ok(None)
    }

    async fn list_zones(&mut self) -> Result<Vec<String>> {
        let pattern = format!("{}zone:*", self.prefix);
        Ok(Vec::new())
    }

    async fn append_delta(&mut self, robot_id: &str, delta: &DeltaMutator) -> Result<()> {
        let key = format!("{}deltas:{}", self.prefix, robot_id);
        Ok(())
    }

    async fn get_deltas(&mut self, robot_id: &str, _since_version: u64) -> Result<Vec<DeltaMutator>> {
        Ok(Vec::new())
    }

    async fn save_version_vector(&mut self, robot_id: &str, vv: &VersionVector) -> Result<()> {
        let key = format!("{}vv:{}", self.prefix, robot_id);
        Ok(())
    }

    async fn get_version_vector(&mut self, robot_id: &str) -> Result<Option<VersionVector>> {
        Ok(None)
    }

    async fn publish_state_update(&mut self, robot_id: &str, state: &RobotState) -> Result<()> {
        let channel = format!("{}updates", self.prefix);
        Ok(())
    }

    async fn subscribe_state_updates(&mut self) -> Result<Box<dyn StateUpdateSubscriber>> {
        struct DummySubscriber;
        impl StateUpdateSubscriber for DummySubscriber {
            fn receive(&self) -> Option<(String, RobotState)> { None }
        }
        Ok(Box::new(DummySubscriber))
    }
}

pub struct EtcdBackend {
    endpoints: Vec<String>,
    prefix: String,
    node_id: NodeId,
}

impl EtcdBackend {
    pub fn new(endpoints: Vec<String>, node_id: NodeId) -> Self {
        Self {
            endpoints,
            prefix: "pmcp/ledger/".to_string(),
            node_id,
        }
    }
}

#[async_trait]
impl StorageBackend for EtcdBackend {
    async fn connect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn save_robot_state(&mut self, robot_id: &str, state: &RobotState) -> Result<()> {
        let key = format!("{}{}", self.prefix, robot_id);
        Ok(())
    }

    async fn get_robot_state(&mut self, robot_id: &str) -> Result<Option<RobotState>> {
        Ok(None)
    }

    async fn delete_robot_state(&mut self, robot_id: &str) -> Result<()> {
        let key = format!("{}{}", self.prefix, robot_id);
        Ok(())
    }

    async fn list_robots(&mut self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn save_zone(&mut self, zone_id: &str, state: &ZoneState) -> Result<()> {
        let key = format!("{}zone/{}", self.prefix, zone_id);
        Ok(())
    }

    async fn get_zone(&mut self, zone_id: &str) -> Result<Option<ZoneState>> {
        Ok(None)
    }

    async fn list_zones(&mut self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn append_delta(&mut self, robot_id: &str, delta: &DeltaMutator) -> Result<()> {
        Ok(())
    }

    async fn get_deltas(&mut self, robot_id: &str, _since_version: u64) -> Result<Vec<DeltaMutator>> {
        Ok(Vec::new())
    }

    async fn save_version_vector(&mut self, robot_id: &str, vv: &VersionVector) -> Result<()> {
        Ok(())
    }

    async fn get_version_vector(&mut self, robot_id: &str) -> Result<Option<VersionVector>> {
        Ok(None)
    }

    async fn publish_state_update(&mut self, _robot_id: &str, _state: &RobotState) -> Result<()> {
        Ok(())
    }

    async fn subscribe_state_updates(&mut self) -> Result<Box<dyn StateUpdateSubscriber>> {
        struct DummySubscriber;
        impl StateUpdateSubscriber for DummySubscriber {
            fn receive(&self) -> Option<(String, RobotState)> { None }
        }
        Ok(Box::new(DummySubscriber))
    }
}