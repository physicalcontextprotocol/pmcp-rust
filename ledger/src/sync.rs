use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use async_trait::async_trait;

use crate::crdt::{RobotStateCRDT, DeltaMutator, NodeId, VersionVector};
use crate::partition::PartitionKey;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMessage {
    Delta {
        partition_key: PartitionKey,
        deltas: Vec<DeltaMutator>,
        sender_version: VersionVector,
    },
    FullState {
        partition_key: PartitionKey,
        crdt: RobotStateCRDT,
    },
    GossipRequest {
        partition_key: PartitionKey,
        known_version: VersionVector,
    },
    GossipResponse {
        partition_key: PartitionKey,
        crdt: RobotStateCRDT,
    },
    Heartbeat {
        node_id: NodeId,
        timestamp: u64,
        partitions: Vec<PartitionKey>,
    },
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub node_id: NodeId,
    pub address: String,
    pub port: u16,
    pub last_seen: Instant,
    pub partitions: Vec<PartitionKey>,
}

impl Peer {
    pub fn new(node_id: NodeId, address: String, port: u16) -> Self {
        Self {
            node_id,
            address,
            port,
            last_seen: Instant::now(),
            partitions: Vec::new(),
        }
    }

    pub fn is_alive(&self, timeout: Duration) -> bool {
        self.last_seen.elapsed() < timeout
    }

    pub fn touch(&mut self) {
        self.last_seen = Instant::now();
    }
}

pub trait SyncProtocol: Send + Sync {
    fn get_local_state(&self, partition: &PartitionKey) -> Option<RobotStateCRDT>;
    fn merge_remote_state(&mut self, partition: &PartitionKey, state: RobotStateCRDT);
    fn get_pending_deltas(&self, partition: &PartitionKey) -> Vec<DeltaMutator>;
}

pub struct GossipProtocol {
    node_id: NodeId,
    peers: HashMap<NodeId, Peer>,
    partitions: HashMap<PartitionKey, RobotStateCRDT>,
    fanout: usize,
    interval: Duration,
    max_deltas_per_message: usize,
}

impl GossipProtocol {
    pub fn new(node_id: NodeId, fanout: usize, interval_ms: u64) -> Self {
        Self {
            node_id,
            peers: HashMap::new(),
            partitions: HashMap::new(),
            fanout,
            interval: Duration::from_millis(interval_ms),
            max_deltas_per_message: 100,
        }
    }

    pub fn add_peer(&mut self, peer: Peer) {
        self.peers.insert(peer.node_id.clone(), peer);
    }

    pub fn remove_peer(&mut self, node_id: &NodeId) {
        self.peers.remove(node_id);
    }

    pub fn get_peers(&self) -> Vec<&Peer> {
        self.peers.values().collect()
    }

    pub fn select_targets(&self) -> Vec<NodeId> {
        let alive: Vec<&Peer> = self.peers.values().filter(|p| p.is_alive(Duration::from_secs(30))).collect();
        if alive.is_empty() {
            return Vec::new();
        }

        let count = self.fanout.min(alive.len());
        let mut selected = Vec::with_capacity(count);
        let mut rng_state = self.node_id.0.len();

        for _ in 0..count {
            let idx = rng_state % alive.len();
            selected.push(alive[idx].node_id.clone());
            rng_state = rng_state.wrapping_mul(31).wrapping_add(1);
        }

        selected
    }

    pub fn register_partition(&mut self, key: PartitionKey) {
        if !self.partitions.contains_key(&key) {
            self.partitions.insert(key, RobotStateCRDT::new(self.node_id.clone()));
        }
    }

    pub fn merge(&mut self, key: &PartitionKey, crdt: RobotStateCRDT) {
        if let Some(local) = self.partitions.get_mut(key) {
            local.merge(&crdt);
        } else {
            self.partitions.insert(key.clone(), crdt);
        }
    }

    pub fn get_partition(&self, key: &PartitionKey) -> Option<&RobotStateCRDT> {
        self.partitions.get(key)
    }

    pub fn all_partitions(&self) -> Vec<&PartitionKey> {
        self.partitions.keys().collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub fanout: usize,
    pub interval_ms: u64,
    pub max_message_size: usize,
    pub max_deltas: usize,
    pub anti_entropy_interval_ms: u64,
    pub peer_timeout_seconds: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            fanout: 3,
            interval_ms: 100,
            max_message_size: 1024 * 1024,
            max_deltas: 100,
            anti_entropy_interval_ms: 5000,
            peer_timeout_seconds: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMetrics {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub merge_conflicts: u64,
    pub peer_disconnects: u64,
    pub last_sync: Option<u64>,
}

impl Default for SyncMetrics {
    fn default() -> Self {
        Self {
            messages_sent: 0,
            messages_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            merge_conflicts: 0,
            peer_disconnects: 0,
            last_sync: None,
        }
    }
}

impl SyncMetrics {
    pub fn record_send(&mut self, size: usize) {
        self.messages_sent += 1;
        self.bytes_sent += size as u64;
    }

    pub fn record_receive(&mut self, size: usize) {
        self.messages_received += 1;
        self.bytes_received += size as u64;
    }

    pub fn record_conflict(&mut self) {
        self.merge_conflicts += 1;
    }

    pub fn record_disconnect(&mut self) {
        self.peer_disconnects += 1;
    }
}

pub struct AntiEntropy {
    interval: Duration,
    last_run: Instant,
    enabled: bool,
}

impl AntiEntropy {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            interval: Duration::from_millis(interval_ms),
            last_run: Instant::now(),
            enabled: true,
        }
    }

    pub fn should_run(&self) -> bool {
        self.enabled && self.last_run.elapsed() >= self.interval
    }

    pub fn mark_run(&mut self) {
        self.last_run = Instant::now();
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }
}