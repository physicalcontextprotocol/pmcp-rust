//! P-MCP Raft-based Consensus Engine
//!
//! Provides distributed consensus for fleet-wide safety decisions,
//! lease arbitration, and constitution change proposals.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock, Mutex};
use tokio::time::interval;
use uuid::Uuid;
use tracing::{debug, error, info, warn};

use crate::error::{PmcpError, PmcpErrorCode};

// ─────────────────────────────────────────────────────────────────────────────
//  RAFT ROLES
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaftRole {
    Follower,
    Candidate,
    Leader,
}

impl std::fmt::Display for RaftRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RaftRole::Follower => write!(f, "follower"),
            RaftRole::Candidate => write!(f, "candidate"),
            RaftRole::Leader => write!(f, "leader"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  LOG ENTRIES
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogCommand {
    NoOp,
    SetLeaseOwner { zone: String, owner: String, expires_ms: u64 },
    RevokeLeaseOwner { zone: String },
    UpdateConstitution { rule_id: String, rule_body: String, signature: Vec<u8> },
    RegisterRobot { robot_id: String, capabilities: Vec<String> },
    DeregisterRobot { robot_id: String },
    SetFleetParameter { key: String, value: serde_json::Value },
    ApproveMultisigAction { action_id: String, signers: Vec<String> },
    RecordSafetyEvent { event_id: String, severity: u8, payload: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub index: u64,
    pub term: u64,
    pub command: LogCommand,
    pub timestamp_ms: u64,
    pub proposed_by: String,
}

impl LogEntry {
    pub fn new(index: u64, term: u64, command: LogCommand, proposed_by: impl Into<String>) -> Self {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            index,
            term,
            command,
            timestamp_ms,
            proposed_by: proposed_by.into(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  RAFT MESSAGES
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVote {
    pub term: u64,
    pub candidate_id: String,
    pub last_log_index: u64,
    pub last_log_term: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVoteResponse {
    pub term: u64,
    pub vote_granted: bool,
    pub voter_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntries {
    pub term: u64,
    pub leader_id: String,
    pub prev_log_index: u64,
    pub prev_log_term: u64,
    pub entries: Vec<LogEntry>,
    pub leader_commit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
    pub term: u64,
    pub success: bool,
    pub follower_id: String,
    pub match_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaftMessage {
    RequestVote(RequestVote),
    RequestVoteResponse(RequestVoteResponse),
    AppendEntries(AppendEntries),
    AppendEntriesResponse(AppendEntriesResponse),
    ClientProposal { command: LogCommand, client_id: String, request_id: String },
    Snapshot { last_index: u64, last_term: u64, data: Vec<u8> },
}

// ─────────────────────────────────────────────────────────────────────────────
//  STATE MACHINE
// ─────────────────────────────────────────────────────────────────────────────

pub trait StateMachine: Send + Sync {
    fn apply(&mut self, command: &LogCommand) -> Result<serde_json::Value, PmcpError>;
    fn snapshot(&self) -> Vec<u8>;
    fn restore(&mut self, snapshot: &[u8]) -> Result<(), PmcpError>;
}

pub struct FleetStateMachine {
    leases: HashMap<String, (String, u64)>,  // zone -> (owner, expires_ms)
    robots: HashMap<String, Vec<String>>,     // robot_id -> capabilities
    parameters: HashMap<String, serde_json::Value>,
    constitution_rules: HashMap<String, (String, Vec<u8>)>, // rule_id -> (body, sig)
    approved_actions: HashSet<String>,
    safety_events: VecDeque<serde_json::Value>,
}

impl FleetStateMachine {
    pub fn new() -> Self {
        Self {
            leases: HashMap::new(),
            robots: HashMap::new(),
            parameters: HashMap::new(),
            constitution_rules: HashMap::new(),
            approved_actions: HashSet::new(),
            safety_events: VecDeque::with_capacity(10_000),
        }
    }

    pub fn lease_owner(&self, zone: &str) -> Option<&str> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.leases.get(zone).and_then(|(owner, exp)| {
            if *exp > now { Some(owner.as_str()) } else { None }
        })
    }

    pub fn all_robots(&self) -> Vec<String> {
        self.robots.keys().cloned().collect()
    }
}

impl Default for FleetStateMachine {
    fn default() -> Self { Self::new() }
}

impl StateMachine for FleetStateMachine {
    fn apply(&mut self, command: &LogCommand) -> Result<serde_json::Value, PmcpError> {
        match command {
            LogCommand::NoOp => Ok(serde_json::Value::Null),

            LogCommand::SetLeaseOwner { zone, owner, expires_ms } => {
                self.leases.insert(zone.clone(), (owner.clone(), *expires_ms));
                Ok(serde_json::json!({
                    "zone": zone,
                    "owner": owner,
                    "expires_ms": expires_ms
                }))
            }

            LogCommand::RevokeLeaseOwner { zone } => {
                self.leases.remove(zone);
                Ok(serde_json::json!({ "zone": zone, "revoked": true }))
            }

            LogCommand::UpdateConstitution { rule_id, rule_body, signature } => {
                self.constitution_rules.insert(
                    rule_id.clone(),
                    (rule_body.clone(), signature.clone()),
                );
                Ok(serde_json::json!({ "rule_id": rule_id, "updated": true }))
            }

            LogCommand::RegisterRobot { robot_id, capabilities } => {
                self.robots.insert(robot_id.clone(), capabilities.clone());
                Ok(serde_json::json!({ "robot_id": robot_id, "registered": true }))
            }

            LogCommand::DeregisterRobot { robot_id } => {
                self.robots.remove(robot_id);
                Ok(serde_json::json!({ "robot_id": robot_id, "deregistered": true }))
            }

            LogCommand::SetFleetParameter { key, value } => {
                self.parameters.insert(key.clone(), value.clone());
                Ok(serde_json::json!({ "key": key, "set": true }))
            }

            LogCommand::ApproveMultisigAction { action_id, signers } => {
                self.approved_actions.insert(action_id.clone());
                Ok(serde_json::json!({
                    "action_id": action_id,
                    "signers": signers,
                    "approved": true
                }))
            }

            LogCommand::RecordSafetyEvent { event_id, severity, payload } => {
                let event = serde_json::json!({
                    "event_id": event_id,
                    "severity": severity,
                    "payload": payload
                });
                if self.safety_events.len() >= 10_000 {
                    self.safety_events.pop_front();
                }
                self.safety_events.push_back(event.clone());
                Ok(event)
            }
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        let state = serde_json::json!({
            "leases": self.leases,
            "robots": self.robots,
            "parameters": self.parameters,
        });
        serde_json::to_vec(&state).unwrap_or_default()
    }

    fn restore(&mut self, snapshot: &[u8]) -> Result<(), PmcpError> {
        let state: serde_json::Value = serde_json::from_slice(snapshot)
            .map_err(|e| PmcpError::with_message(PmcpErrorCode::InternalError, e.to_string()))?;

        if let Some(leases) = state.get("leases").and_then(|v| v.as_object()) {
            for (zone, val) in leases {
                if let (Some(owner), Some(exp)) = (
                    val.get(0).and_then(|v| v.as_str()),
                    val.get(1).and_then(|v| v.as_u64()),
                ) {
                    self.leases.insert(zone.clone(), (owner.to_string(), exp));
                }
            }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  RAFT NODE
// ─────────────────────────────────────────────────────────────────────────────

pub struct RaftConfig {
    pub node_id: String,
    pub peers: Vec<String>,
    pub election_timeout_min_ms: u64,
    pub election_timeout_max_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub max_log_entries_per_rpc: usize,
    pub snapshot_threshold: u64,
}

impl Default for RaftConfig {
    fn default() -> Self {
        Self {
            node_id: Uuid::new_v4().to_string(),
            peers: vec![],
            election_timeout_min_ms: 150,
            election_timeout_max_ms: 300,
            heartbeat_interval_ms: 50,
            max_log_entries_per_rpc: 100,
            snapshot_threshold: 10_000,
        }
    }
}

pub struct RaftNode<S: StateMachine> {
    config: RaftConfig,
    role: RaftRole,
    current_term: u64,
    voted_for: Option<String>,
    log: Vec<LogEntry>,
    commit_index: u64,
    last_applied: u64,
    // Leader state
    next_index: HashMap<String, u64>,
    match_index: HashMap<String, u64>,
    // Candidate state
    votes_received: HashSet<String>,
    // Timing
    last_heartbeat: Instant,
    election_deadline: Instant,
    // Messaging
    outbox: mpsc::Sender<(String, RaftMessage)>,
    inbox: mpsc::Receiver<(String, RaftMessage)>,
    // Application
    state_machine: S,
    // Pending client proposals (id -> waker)
    pending: HashMap<String, tokio::sync::oneshot::Sender<Result<serde_json::Value, PmcpError>>>,
}

impl<S: StateMachine> RaftNode<S> {
    pub fn new(
        config: RaftConfig,
        state_machine: S,
        outbox: mpsc::Sender<(String, RaftMessage)>,
        inbox: mpsc::Receiver<(String, RaftMessage)>,
    ) -> Self {
        let now = Instant::now();
        let timeout = Duration::from_millis(
            config.election_timeout_min_ms
                + (rand_u64() % (config.election_timeout_max_ms - config.election_timeout_min_ms))
        );
        Self {
            config,
            role: RaftRole::Follower,
            current_term: 0,
            voted_for: None,
            log: vec![LogEntry::new(0, 0, LogCommand::NoOp, "")],
            commit_index: 0,
            last_applied: 0,
            next_index: HashMap::new(),
            match_index: HashMap::new(),
            votes_received: HashSet::new(),
            last_heartbeat: now,
            election_deadline: now + timeout,
            outbox,
            inbox,
            state_machine,
            pending: HashMap::new(),
        }
    }

    pub async fn run(&mut self) {
        let mut heartbeat_interval = interval(Duration::from_millis(
            self.config.heartbeat_interval_ms
        ));

        loop {
            tokio::select! {
                _ = heartbeat_interval.tick() => {
                    if self.role == RaftRole::Leader {
                        self.send_heartbeats().await;
                    } else if Instant::now() >= self.election_deadline {
                        self.start_election().await;
                    }
                    self.apply_committed();
                }
                Some((from, msg)) = self.inbox.recv() => {
                    self.handle_message(from, msg).await;
                    self.apply_committed();
                }
            }
        }
    }

    async fn start_election(&mut self) {
        self.current_term += 1;
        self.role = RaftRole::Candidate;
        self.voted_for = Some(self.config.node_id.clone());
        self.votes_received.clear();
        self.votes_received.insert(self.config.node_id.clone());

        info!(
            "Node {} starting election for term {}",
            self.config.node_id, self.current_term
        );

        let last_index = self.log.len() as u64 - 1;
        let last_term = self.log.last().map(|e| e.term).unwrap_or(0);

        let rv = RaftMessage::RequestVote(RequestVote {
            term: self.current_term,
            candidate_id: self.config.node_id.clone(),
            last_log_index: last_index,
            last_log_term: last_term,
        });

        for peer in &self.config.peers.clone() {
            let _ = self.outbox.send((peer.clone(), rv.clone())).await;
        }

        self.reset_election_timer();
    }

    async fn send_heartbeats(&mut self) {
        for peer in self.config.peers.clone() {
            let next = self.next_index.get(&peer).copied().unwrap_or(self.log.len() as u64);
            let prev_index = next.saturating_sub(1);
            let prev_term = if prev_index == 0 {
                0
            } else {
                self.log.get(prev_index as usize).map(|e| e.term).unwrap_or(0)
            };

            let entries: Vec<LogEntry> = self.log
                .iter()
                .skip(next as usize)
                .take(self.config.max_log_entries_per_rpc)
                .cloned()
                .collect();

            let ae = RaftMessage::AppendEntries(AppendEntries {
                term: self.current_term,
                leader_id: self.config.node_id.clone(),
                prev_log_index: prev_index,
                prev_log_term: prev_term,
                entries,
                leader_commit: self.commit_index,
            });

            let _ = self.outbox.send((peer.clone(), ae)).await;
        }
    }

    async fn handle_message(&mut self, from: String, msg: RaftMessage) {
        match msg {
            RaftMessage::RequestVote(rv) => self.handle_request_vote(from, rv).await,
            RaftMessage::RequestVoteResponse(rvr) => self.handle_vote_response(from, rvr).await,
            RaftMessage::AppendEntries(ae) => self.handle_append_entries(from, ae).await,
            RaftMessage::AppendEntriesResponse(aer) => self.handle_ae_response(from, aer).await,
            RaftMessage::ClientProposal { command, client_id: _, request_id } => {
                if self.role == RaftRole::Leader {
                    let index = self.log.len() as u64;
                    let entry = LogEntry::new(index, self.current_term, command, &self.config.node_id);
                    self.log.push(entry);
                    self.match_index.insert(self.config.node_id.clone(), index);
                    info!("Leader appended entry at index {}", index);
                } else {
                    warn!("Received proposal but not leader (request_id={})", request_id);
                }
            }
            RaftMessage::Snapshot { last_index, last_term, data } => {
                if let Ok(()) = self.state_machine.restore(&data) {
                    let truncate_at = last_index as usize;
                    if truncate_at < self.log.len() {
                        self.log = self.log.split_off(truncate_at);
                    }
                    self.commit_index = last_index;
                    self.last_applied = last_index;
                    info!("Restored snapshot at index={}, term={}", last_index, last_term);
                }
            }
        }
    }

    async fn handle_request_vote(&mut self, from: String, rv: RequestVote) {
        if rv.term > self.current_term {
            self.become_follower(rv.term);
        }

        let vote_granted = rv.term >= self.current_term
            && (self.voted_for.is_none()
                || self.voted_for.as_deref() == Some(&rv.candidate_id))
            && self.is_log_up_to_date(rv.last_log_index, rv.last_log_term);

        if vote_granted {
            self.voted_for = Some(rv.candidate_id.clone());
            self.reset_election_timer();
        }

        let resp = RaftMessage::RequestVoteResponse(RequestVoteResponse {
            term: self.current_term,
            vote_granted,
            voter_id: self.config.node_id.clone(),
        });
        let _ = self.outbox.send((from, resp)).await;
    }

    async fn handle_vote_response(&mut self, _from: String, rvr: RequestVoteResponse) {
        if rvr.term > self.current_term {
            self.become_follower(rvr.term);
            return;
        }
        if self.role != RaftRole::Candidate || rvr.term != self.current_term {
            return;
        }
        if rvr.vote_granted {
            self.votes_received.insert(rvr.voter_id.clone());
            if self.votes_received.len() > (self.config.peers.len() + 1) / 2 {
                self.become_leader().await;
            }
        }
    }

    async fn handle_append_entries(&mut self, from: String, ae: AppendEntries) {
        if ae.term < self.current_term {
            let resp = RaftMessage::AppendEntriesResponse(AppendEntriesResponse {
                term: self.current_term,
                success: false,
                follower_id: self.config.node_id.clone(),
                match_index: 0,
            });
            let _ = self.outbox.send((from, resp)).await;
            return;
        }

        self.become_follower(ae.term);
        self.reset_election_timer();

        // Check prev log consistency
        let prev_ok = ae.prev_log_index == 0
            || (ae.prev_log_index < self.log.len() as u64
                && self.log[ae.prev_log_index as usize].term == ae.prev_log_term);

        if !prev_ok {
            let resp = RaftMessage::AppendEntriesResponse(AppendEntriesResponse {
                term: self.current_term,
                success: false,
                follower_id: self.config.node_id.clone(),
                match_index: 0,
            });
            let _ = self.outbox.send((from, resp)).await;
            return;
        }

        // Append entries, resolving conflicts
        let mut insert_at = ae.prev_log_index as usize + 1;
        for entry in &ae.entries {
            if insert_at < self.log.len() {
                if self.log[insert_at].term != entry.term {
                    self.log.truncate(insert_at);
                    self.log.push(entry.clone());
                }
            } else {
                self.log.push(entry.clone());
            }
            insert_at += 1;
        }

        if ae.leader_commit > self.commit_index {
            self.commit_index = ae.leader_commit.min(self.log.len() as u64 - 1);
        }

        let match_index = self.log.len() as u64 - 1;
        let resp = RaftMessage::AppendEntriesResponse(AppendEntriesResponse {
            term: self.current_term,
            success: true,
            follower_id: self.config.node_id.clone(),
            match_index,
        });
        let _ = self.outbox.send((from, resp)).await;
    }

    async fn handle_ae_response(&mut self, from: String, aer: AppendEntriesResponse) {
        if aer.term > self.current_term {
            self.become_follower(aer.term);
            return;
        }
        if self.role != RaftRole::Leader {
            return;
        }
        if aer.success {
            self.next_index.insert(from.clone(), aer.match_index + 1);
            self.match_index.insert(from.clone(), aer.match_index);
            self.advance_commit_index();
        } else {
            let next = self.next_index.get(&from).copied().unwrap_or(1);
            self.next_index.insert(from, next.saturating_sub(1).max(1));
        }
    }

    fn advance_commit_index(&mut self) {
        let n = self.log.len() as u64;
        for index in (self.commit_index + 1)..n {
            if self.log[index as usize].term != self.current_term {
                continue;
            }
            let count = 1 + self.match_index.values()
                .filter(|&&mi| mi >= index)
                .count();
            if count > (self.config.peers.len() + 1) / 2 {
                self.commit_index = index;
            }
        }
    }

    fn apply_committed(&mut self) {
        while self.last_applied < self.commit_index {
            self.last_applied += 1;
            let entry = self.log[self.last_applied as usize].clone();
            match self.state_machine.apply(&entry.command) {
                Ok(result) => {
                    debug!("Applied log index {} result={:?}", self.last_applied, result);
                }
                Err(e) => {
                    error!("Failed to apply log index {}: {:?}", self.last_applied, e);
                }
            }
        }
    }

    async fn become_leader(&mut self) {
        self.role = RaftRole::Leader;
        info!("Node {} became leader for term {}", self.config.node_id, self.current_term);

        let last_index = self.log.len() as u64;
        for peer in &self.config.peers.clone() {
            self.next_index.insert(peer.clone(), last_index);
            self.match_index.insert(peer.clone(), 0);
        }

        // Append a NoOp to commit pending entries from previous terms
        let noop = LogEntry::new(last_index, self.current_term, LogCommand::NoOp, &self.config.node_id);
        self.log.push(noop);
        self.send_heartbeats().await;
    }

    fn become_follower(&mut self, term: u64) {
        self.current_term = term;
        self.role = RaftRole::Follower;
        self.voted_for = None;
        self.reset_election_timer();
    }

    fn reset_election_timer(&mut self) {
        let timeout = Duration::from_millis(
            self.config.election_timeout_min_ms
                + (rand_u64() % (self.config.election_timeout_max_ms - self.config.election_timeout_min_ms))
        );
        self.election_deadline = Instant::now() + timeout;
    }

    fn is_log_up_to_date(&self, last_log_index: u64, last_log_term: u64) -> bool {
        let my_last_term = self.log.last().map(|e| e.term).unwrap_or(0);
        let my_last_index = self.log.len() as u64 - 1;
        if last_log_term != my_last_term {
            last_log_term > my_last_term
        } else {
            last_log_index >= my_last_index
        }
    }

    pub fn is_leader(&self) -> bool { self.role == RaftRole::Leader }
    pub fn current_term(&self) -> u64 { self.current_term }
    pub fn log_length(&self) -> u64 { self.log.len() as u64 }
    pub fn commit_index(&self) -> u64 { self.commit_index }
}

// Tiny PRNG helper (not cryptographic — just for timeouts)
fn rand_u64() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    Instant::now().elapsed().subsec_nanos().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    h.finish()
}

// ─────────────────────────────────────────────────────────────────────────────
//  CONSENSUS CLUSTER  (multi-node coordinator)
// ─────────────────────────────────────────────────────────────────────────────

pub struct ConsensusCluster {
    node_id: String,
    outbox: mpsc::Sender<(String, RaftMessage)>,
    proposal_tx: mpsc::Sender<LogCommand>,
}

impl ConsensusCluster {
    pub fn new(config: RaftConfig) -> (Self, impl std::future::Future<Output = ()>) {
        let (outbox_tx, _outbox_rx) = mpsc::channel(1024);
        let (inbox_tx, inbox_rx) = mpsc::channel(1024);
        let (proposal_tx, mut proposal_rx) = mpsc::channel::<LogCommand>(256);

        let node_id = config.node_id.clone();
        let sm = FleetStateMachine::new();
        let mut node = RaftNode::new(config, sm, outbox_tx.clone(), inbox_rx);

        let runner = async move {
            // Drain proposals into inbox
            let (internal_tx, mut internal_rx) = mpsc::channel(256);
            tokio::spawn(async move {
                while let Some(cmd) = proposal_rx.recv().await {
                    let _ = internal_tx.send((
                        "local".to_string(),
                        RaftMessage::ClientProposal {
                            command: cmd,
                            client_id: "local".to_string(),
                            request_id: Uuid::new_v4().to_string(),
                        },
                    )).await;
                }
            });
            node.run().await;
        };

        let cluster = Self {
            node_id,
            outbox: outbox_tx,
            proposal_tx,
        };
        (cluster, runner)
    }

    pub async fn propose(&self, command: LogCommand) -> Result<(), PmcpError> {
        self.proposal_tx.send(command).await
            .map_err(|e| PmcpError::with_message(PmcpErrorCode::InternalError, e.to_string()))
    }

    pub fn node_id(&self) -> &str { &self.node_id }
}
