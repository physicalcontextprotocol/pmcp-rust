use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use async_trait::async_trait;
use thiserror::Error;

use crate::crdt::{RobotStateCRDT, NodeId, VersionVector};
use crate::partition::PartitionKey;

#[derive(Error, Debug)]
pub enum RPCError {
    #[error("Timeout: {0}")]
    Timeout(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Not a leader")]
    NotLeader,
    #[error("Forward to leader failed: {0}")]
    ForwardFailed(String),
}

pub type Result<T> = std::result::Result<T, RPCError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RPCMessage {
    Request { method: String, params: Vec<u8>, request_id: String },
    Response { request_id: String, result: Vec<u8>, error: Option<String> },
    Forward { method: String, params: Vec<u8>, request_id: String },
    Heartbeat { node_id: String, timestamp: u64, term: u64 },
    VoteRequest { term: u64, candidate_id: String, last_log_index: u64, last_log_term: u64 },
    VoteResponse { term: u64, vote_granted: bool },
    AppendEntries { term: u64, leader_id: String, prev_log_index: u64, prev_log_term: u64, entries: Vec<LogEntry>, leader_commit: u64 },
    AppendResponse { term: u64, success: bool, match_index: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub index: u64,
    pub term: u64,
    pub data: Vec<u8>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RPCClientConfig {
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub max_retries: u32,
    pub keep_alive_ms: u64,
}

impl Default for RPCClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 1000,
            request_timeout_ms: 5000,
            max_retries: 3,
            keep_alive_ms: 30000,
        }
    }
}

pub struct RPCClient {
    config: RPCClientConfig,
    connections: HashMap<String, mpsc::Sender<RPCMessage>>,
    node_id: NodeId,
}

impl RPCClient {
    pub fn new(config: RPCClientConfig, node_id: NodeId) -> Self {
        Self {
            config,
            connections: HashMap::new(),
            node_id,
        }
    }

    pub async fn connect(&mut self, address: String) -> Result<()> {
        let (tx, _rx) = mpsc::channel(100);
        self.connections.insert(address, tx);
        Ok(())
    }

    pub async fn call(&self, address: &str, method: &str, params: Vec<u8>) -> Result<Vec<u8>> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let message = RPCMessage::Request {
            method: method.to_string(),
            params,
            request_id: request_id.clone(),
        };

        if let Some(sender) = self.connections.get(address) {
            sender.send(message).await.map_err(|e| RPCError::ConnectionFailed(e.to_string()))?;
        } else {
            return Err(RPCError::ConnectionFailed(format!("Not connected to {}", address)));
        }

        Ok(vec![])
    }

    pub async fn send_async(&self, address: &str, method: &str, params: Vec<u8>) -> Result<()> {
        let message = RPCMessage::Request {
            method: method.to_string(),
            params,
            request_id: uuid::Uuid::new_v4().to_string(),
        };

        if let Some(sender) = self.connections.get(address) {
            sender.send(message).await.map_err(|e| RPCError::ConnectionFailed(e.to_string()))?;
        }
        Ok(())
    }
}

pub trait RPCService: Send + Sync {
    fn handle_request(&self, method: &str, params: &[u8]) -> Result<Vec<u8>>;
    fn handle_forward(&self, method: &str, params: &[u8]) -> Result<Vec<u8>>;
}

pub struct RPCServer {
    node_id: NodeId,
    address: SocketAddr,
    handlers: HashMap<String, Box<dyn RPCService>>,
}

impl RPCServer {
    pub fn new(node_id: NodeId, address: SocketAddr) -> Self {
        Self {
            node_id,
            address,
            handlers: HashMap::new(),
        }
    }

    pub fn register_handler(&mut self, method: &str, handler: Box<dyn RPCService>) {
        self.handlers.insert(method.to_string(), handler);
    }

    pub async fn start(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node_id: String,
    pub address: String,
    pub port: u16,
    pub is_leader: bool,
    pub term: u64,
    pub last_contact: u64,
    pub match_index: u64,
    pub next_index: u64,
}

impl PeerInfo {
    pub fn new(node_id: String, address: String, port: u16) -> Self {
        Self {
            node_id,
            address,
            port,
            is_leader: false,
            term: 0,
            last_contact: 0,
            match_index: 0,
            next_index: 1,
        }
    }

    pub fn is_active(&self, timeout_ms: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        now - self.last_contact < timeout_ms
    }
}

pub struct RaftState {
    pub term: u64,
    pub voted_for: Option<String>,
    pub is_leader: bool,
    pub leader_id: Option<String>,
    pub commit_index: u64,
    pub last_applied: u64,
    pub log: Vec<LogEntry>,
    pub peers: HashMap<String, PeerInfo>,
    pub voted_nodes: HashSet<String>,
}

impl RaftState {
    pub fn new(node_id: String) -> Self {
        Self {
            term: 0,
            voted_for: None,
            is_leader: false,
            leader_id: None,
            commit_index: 0,
            last_applied: 0,
            log: Vec::new(),
            peers: HashMap::new(),
            voted_nodes: HashSet::new(),
        }
    }

    pub fn become_follower(&mut self, term: u64, leader_id: Option<String>) {
        self.term = term;
        self.is_leader = false;
        self.leader_id = leader_id;
        self.voted_for = None;
        self.voted_nodes.clear();
    }

    pub fn become_candidate(&mut self) {
        self.term += 1;
        self.voted_for = Some(self.peers.keys().next().cloned().unwrap_or_default());
        self.is_leader = false;
    }

    pub fn become_leader(&mut self) {
        self.is_leader = true;
        self.leader_id = Some(self.peers.keys().next().cloned().unwrap_or_default());
        for peer in self.peers.values_mut() {
            peer.is_leader = false;
            peer.next_index = self.log.len() as u64 + 1;
            peer.match_index = 0;
        }
    }

    pub fn add_log_entry(&mut self, data: Vec<u8>) -> u64 {
        let index = self.log.len() as u64 + 1;
        let entry = LogEntry {
            index,
            term: self.term,
            data,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        self.log.push(entry);
        index
    }

    pub fn get_entries_since(&self, start_index: u64) -> Vec<&LogEntry> {
        self.log.iter()
            .filter(|e| e.index >= start_index)
            .collect()
    }
}

pub struct RaftProtocol {
    state: RaftState,
    node_id: String,
    election_timeout_ms: u64,
    heartbeat_timeout_ms: u64,
}

impl RaftProtocol {
    pub fn new(node_id: String, election_timeout_ms: u64, heartbeat_timeout_ms: u64) -> Self {
        Self {
            state: RaftState::new(node_id.clone()),
            node_id,
            election_timeout_ms,
            heartbeat_timeout_ms,
        }
    }

    pub fn add_peer(&mut self, peer: PeerInfo) {
        self.state.peers.insert(peer.node_id.clone(), peer);
    }

    pub fn remove_peer(&mut self, node_id: &str) {
        self.state.peers.remove(node_id);
    }

    pub fn handle_vote_request(&mut self, candidate_id: String, term: u64, last_log_index: u64, last_log_term: u64) -> (u64, bool) {
        let voted_for = self.state.voted_for.clone();
        let can_vote = voted_for.is_none() || voted_for == Some(candidate_id.clone());
        let log_ok = last_log_term > self.state.log.last().map(|e| e.term).unwrap_or(0) ||
                     (last_log_term == self.state.log.last().map(|e| e.term).unwrap_or(0) && 
                      last_log_index >= self.state.log.len() as u64);

        if term > self.state.term && can_vote && log_ok {
            self.state.voted_for = Some(candidate_id.clone());
            self.state.voted_nodes.insert(candidate_id);
            (self.state.term, true)
        } else {
            (self.state.term, false)
        }
    }

    pub fn handle_append_entries(&mut self, term: u64, leader_id: String, prev_log_index: u64, 
                                   prev_log_term: u64, entries: Vec<LogEntry>, leader_commit: u64) -> (u64, bool, u64) {
        if term < self.state.term {
            return (self.state.term, false, 0);
        }

        self.state.become_follower(term, Some(leader_id));

        if prev_log_index > 0 {
            if prev_log_index as usize > self.state.log.len() {
                return (term, false, 0);
            }
            if let Some(entry) = self.state.log.get((prev_log_index - 1) as usize) {
                if entry.term != prev_log_term {
                    return (term, false, 0);
                }
            }
        }

        if !entries.is_empty() {
            let start = prev_log_index as usize;
            if start < self.state.log.len() {
                self.state.log.truncate(start);
            }
            self.state.log.extend(entries);
        }

        if leader_commit > self.state.commit_index {
            self.state.commit_index = leader_commit.min(self.state.log.len() as u64);
        }

        (term, true, self.state.log.len() as u64)
    }

    pub fn start_election(&mut self) -> bool {
        if self.state.is_leader {
            return false;
        }

        self.state.become_candidate();
        let votes = self.state.voted_nodes.len() + 1;
        let quorum = (self.state.peers.len() + 2) / 2 + 1;

        if votes >= quorum {
            self.state.become_leader();
            return true;
        }

        false
    }

    pub fn get_leader(&self) -> Option<String> {
        self.state.leader_id.clone()
    }

    pub fn is_leader(&self) -> bool {
        self.state.is_leader
    }

    pub fn get_term(&self) -> u64 {
        self.state.term
    }

    pub fn replicate(&mut self, entry: LogEntry) -> bool {
        if !self.state.is_leader {
            return false;
        }

        for peer in self.state.peers.values_mut() {
            if peer.next_index <= self.state.log.len() as u64 {
                let entries: Vec<LogEntry> = self.state.log.iter()
                    .skip((peer.next_index - 1) as usize)
                    .cloned()
                    .collect();

                let prev_entry = self.state.log.get((peer.next_index - 1) as usize);
                let prev_index = peer.next_index - 1;
                let prev_term = prev_entry.map(|e| e.term).unwrap_or(0);

                let _ = self.handle_append_entries(
                    self.state.term,
                    self.node_id.clone(),
                    prev_index,
                    prev_term,
                    entries,
                    self.state.commit_index,
                );
            }
        }
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub heartbeat_interval_ms: u64,
    pub election_timeout_min_ms: u64,
    pub election_timeout_max_ms: u64,
    pub max_inflight_requests: usize,
    pub retry_timeout_ms: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval_ms: 100,
            election_timeout_min_ms: 150,
            election_timeout_max_ms: 300,
            max_inflight_requests: 10,
            retry_timeout_ms: 1000,
        }
    }
}

pub struct NetworkPartitionHandler {
    partitions: HashMap<String, HashSet<String>>,
    packet_loss_rate: f32,
    latency_ms: u64,
}

impl NetworkPartitionHandler {
    pub fn new() -> Self {
        Self {
            partitions: HashMap::new(),
            packet_loss_rate: 0.0,
            latency_ms: 0,
        }
    }

    pub fn simulate_partition(&mut self, partition_id: &str, nodes: Vec<String>) {
        self.partitions.insert(partition_id.to_string(), nodes.into_iter().collect());
    }

    pub fn resolve_partition(&mut self, partition_id: &str) {
        self.partitions.remove(partition_id);
    }

    pub fn can_reach(&self, from: &str, to: &str) -> bool {
        for partition in self.partitions.values() {
            if partition.contains(from) && !partition.contains(to) {
                return false;
            }
            if partition.contains(to) && !partition.contains(from) {
                return false;
            }
        }
        true
    }

    pub fn set_packet_loss(&mut self, rate: f32) {
        self.packet_loss_rate = rate.clamp(0.0, 1.0);
    }

    pub fn should_drop(&self) -> bool {
        if self.packet_loss_rate > 0.0 {
            let mut rng = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&std::time::SystemTime::now(), &mut rng);
            let drop_rate = (rng.finish() as f32) / (u64::MAX as f32);
            return drop_rate < self.packet_loss_rate;
        }
        false
    }
}

impl Default for NetworkPartitionHandler {
    fn default() -> Self {
        Self::new()
    }
}