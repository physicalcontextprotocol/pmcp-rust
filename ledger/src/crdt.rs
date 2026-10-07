use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use uuid::Uuid;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(id: String) -> Self {
        NodeId(id)
    }
}

impl Default for NodeId {
    /// A default node is a fresh replica identity, not a shared sentinel.
    fn default() -> Self {
        NodeId(Uuid::new_v4().to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionVector {
    dots: HashMap<NodeId, u64>,
}

impl VersionVector {
    pub fn new() -> Self {
        Self { dots: HashMap::new() }
    }

    pub fn increment(&mut self, node: &NodeId) {
        let count = self.dots.entry(node.clone()).or_insert(0);
        *count += 1;
    }

    pub fn merge(&mut self, other: &VersionVector) {
        for (node, count) in &other.dots {
            let current = self.dots.entry(node.clone()).or_insert(0);
            if *count > *current {
                *current = *count;
            }
        }
    }

    pub fn happens_before(&self, other: &VersionVector) -> bool {
        for (node, count) in &self.dots {
            if let Some(other_count) = other.dots.get(node) {
                if count > other_count {
                    return false;
                }
            }
        }
        for (node, other_count) in &other.dots {
            if !self.dots.contains_key(node) && *other_count > 0 {
                return false;
            }
        }
        true
    }

    pub fn concurrent(&self, other: &VersionVector) -> bool {
        !self.happens_before(other) && !other.happens_before(self)
    }

    pub fn get(&self, node: &NodeId) -> u64 {
        self.dots.get(node).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position3D {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Position3D {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn distance_to(&self, other: &Position3D) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Velocity3D {
    pub vx: f64,
    pub vy: f64,
    pub vz: f64,
    pub omega: f64,
}

impl Velocity3D {
    pub fn new(vx: f64, vy: f64, vz: f64, omega: f64) -> Self {
        Self { vx, vy, vz, omega }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Orientation {
    pub qx: f64,
    pub qy: f64,
    pub qz: f64,
    pub qw: f64,
}

impl Orientation {
    pub fn identity() -> Self {
        Self { qx: 0.0, qy: 0.0, qz: 0.0, qw: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RobotState {
    pub robot_id: String,
    pub position: Position3D,
    pub orientation: Orientation,
    pub velocity: Velocity3D,
    pub timestamp: DateTime<Utc>,
    pub zone: String,
    pub version: u64,
    pub node_id: NodeId,
    pub seq: u64,
    pub metadata: HashMap<String, String>,
}

impl RobotState {
    pub fn new(robot_id: String, node_id: NodeId) -> Self {
        Self {
            robot_id,
            position: Position3D::new(0.0, 0.0, 0.0),
            orientation: Orientation::identity(),
            velocity: Velocity3D::new(0.0, 0.0, 0.0, 0.0),
            timestamp: Utc::now(),
            zone: "default".to_string(),
            version: 0,
            node_id,
            seq: 0,
            metadata: HashMap::new(),
        }
    }

    pub fn with_position(mut self, x: f64, y: f64, z: f64) -> Self {
        self.position = Position3D::new(x, y, z);
        self
    }

    pub fn with_zone(mut self, zone: String) -> Self {
        self.zone = zone;
        self
    }

    pub fn with_metadata(mut self, key: String, value: String) -> Self {
        self.metadata.insert(key, value);
        self
    }

    fn precedes(&self, other: &RobotState) -> bool {
        (self.timestamp, &self.node_id) < (other.timestamp, &other.node_id)
    }

    pub fn merge(&mut self, other: &RobotState) {
        if other.precedes(self) {
            return;
        }

        self.position = other.position.clone();
        self.orientation = other.orientation.clone();
        self.velocity = other.velocity.clone();
        self.zone = other.zone.clone();
        self.metadata.clone_from(&other.metadata);
        self.timestamp = other.timestamp;
        self.node_id = other.node_id.clone();
        self.version = self.version.max(other.version);
        self.seq = self.seq.max(other.seq);
    }
}

impl Default for RobotState {
    fn default() -> Self {
        Self {
            robot_id: "default".to_string(),
            position: Position3D::new(0.0, 0.0, 0.0),
            orientation: Orientation::identity(),
            velocity: Velocity3D::new(0.0, 0.0, 0.0, 0.0),
            timestamp: Utc::now(),
            zone: "default".to_string(),
            version: 0,
            node_id: NodeId::default(),
            seq: 0,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeltaMutator {
    UpdatePosition {
        robot_id: String,
        position: Position3D,
        timestamp: DateTime<Utc>,
        zone: String,
    },
    UpdateVelocity {
        robot_id: String,
        velocity: Velocity3D,
        timestamp: DateTime<Utc>,
    },
    UpdateOrientation {
        robot_id: String,
        orientation: Orientation,
        timestamp: DateTime<Utc>,
    },
    UpdateZone {
        robot_id: String,
        zone: String,
        timestamp: DateTime<Utc>,
    },
    UpdateMetadata {
        robot_id: String,
        key: String,
        value: String,
        timestamp: DateTime<Utc>,
    },
    Delete {
        robot_id: String,
        timestamp: DateTime<Utc>,
    },
}

impl DeltaMutator {
    pub fn apply(&self, state: &mut HashMap<String, RobotState>, node_id: &NodeId) -> bool {
        match self {
            DeltaMutator::UpdatePosition { robot_id, position, timestamp, zone } => {
                let entry = state.entry(robot_id.clone()).or_insert_with(|| {
                    RobotState::new(robot_id.clone(), node_id.clone())
                });
                entry.position = position.clone();
                entry.zone = zone.clone();
                entry.timestamp = *timestamp;
                entry.version = entry.version.saturating_add(1);
                entry.seq += 1;
                true
            }
            DeltaMutator::UpdateVelocity { robot_id, velocity, timestamp } => {
                if let Some(entry) = state.get_mut(robot_id) {
                    entry.velocity = velocity.clone();
                    entry.timestamp = *timestamp;
                    entry.version = entry.version.saturating_add(1);
                    entry.seq += 1;
                    true
                } else {
                    false
                }
            }
            DeltaMutator::UpdateOrientation { robot_id, orientation, timestamp } => {
                if let Some(entry) = state.get_mut(robot_id) {
                    entry.orientation = orientation.clone();
                    entry.timestamp = *timestamp;
                    entry.version = entry.version.saturating_add(1);
                    entry.seq += 1;
                    true
                } else {
                    false
                }
            }
            DeltaMutator::UpdateZone { robot_id, zone, timestamp } => {
                if let Some(entry) = state.get_mut(robot_id) {
                    entry.zone = zone.clone();
                    entry.timestamp = *timestamp;
                    entry.version = entry.version.saturating_add(1);
                    entry.seq += 1;
                    true
                } else {
                    false
                }
            }
            DeltaMutator::UpdateMetadata { robot_id, key, value, timestamp } => {
                if let Some(entry) = state.get_mut(robot_id) {
                    entry.metadata.insert(key.clone(), value.clone());
                    entry.timestamp = *timestamp;
                    entry.version = entry.version.saturating_add(1);
                    entry.seq += 1;
                    true
                } else {
                    false
                }
            }
            DeltaMutator::Delete { robot_id, .. } => {
                state.remove(robot_id).is_some()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RobotStateCRDT {
    states: HashMap<String, RobotState>,
    version: VersionVector,
    node_id: NodeId,
}

impl RobotStateCRDT {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            states: HashMap::new(),
            version: VersionVector::new(),
            node_id,
        }
    }

    pub fn apply_delta(&mut self, delta: DeltaMutator) -> bool {
        let result = delta.apply(&mut self.states, &self.node_id);
        if result {
            self.version.increment(&self.node_id);
        }
        result
    }

    pub fn merge(&mut self, other: &RobotStateCRDT) {
        for (robot_id, other_state) in &other.states {
            let entry = self.states.entry(robot_id.clone()).or_insert_with(|| {
                RobotState::new(robot_id.clone(), self.node_id.clone())
            });
            entry.merge(other_state);
        }
        self.version.merge(&other.version);
    }

    pub fn get_state(&self, robot_id: &str) -> Option<&RobotState> {
        self.states.get(robot_id)
    }

    pub fn get_all_states(&self) -> &HashMap<String, RobotState> {
        &self.states
    }

    pub fn get_states_in_zone(&self, zone: &str) -> Vec<&RobotState> {
        self.states.values().filter(|s| s.zone == zone).collect()
    }

    pub fn version(&self) -> &VersionVector {
        &self.version
    }

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    pub fn count(&self) -> usize {
        self.states.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_vector() {
        let mut vv1 = VersionVector::new();
        let node1 = NodeId::new("node1".to_string());
        let node2 = NodeId::new("node2".to_string());

        vv1.increment(&node1);
        vv1.increment(&node1);

        let mut vv2 = VersionVector::new();
        vv2.increment(&node2);

        assert!(!vv1.happens_before(&vv2));
        assert!(!vv2.happens_before(&vv1));
    }

    #[test]
    fn test_crdt_merge() {
        let node1 = NodeId::new("node1".to_string());
        let node2 = NodeId::new("node2".to_string());

        let mut crdt1 = RobotStateCRDT::new(node1);
        crdt1.apply_delta(DeltaMutator::UpdatePosition {
            robot_id: "robot1".to_string(),
            position: Position3D::new(1.0, 2.0, 0.0),
            timestamp: Utc::now(),
            zone: "zone1".to_string(),
        });

        let mut crdt2 = RobotStateCRDT::new(node2);
        crdt2.apply_delta(DeltaMutator::UpdatePosition {
            robot_id: "robot1".to_string(),
            position: Position3D::new(3.0, 4.0, 0.0),
            timestamp: Utc::now(),
            zone: "zone1".to_string(),
        });

        crdt1.merge(&crdt2);

        assert_eq!(crdt1.count(), 1);
        let state = crdt1.get_state("robot1").unwrap();
        assert!(state.position.x == 1.0 || state.position.x == 3.0);
    }
}