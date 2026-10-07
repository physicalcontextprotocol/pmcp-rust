use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

use crate::crdt::{RobotState, Position3D, Orientation, Velocity3D};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneState {
    pub zone_id: String,
    pub robots: HashMap<String, RobotState>,
    pub last_updated: DateTime<Utc>,
    pub bounds: ZoneBounds,
}

impl ZoneState {
    pub fn new(zone_id: String, bounds: ZoneBounds) -> Self {
        Self {
            zone_id,
            robots: HashMap::new(),
            last_updated: Utc::now(),
            bounds,
        }
    }

    pub fn add_robot(&mut self, state: RobotState) {
        self.robots.insert(state.robot_id.clone(), state);
        self.last_updated = Utc::now();
    }

    pub fn remove_robot(&mut self, robot_id: &str) -> Option<RobotState> {
        let removed = self.robots.remove(robot_id);
        if removed.is_some() {
            self.last_updated = Utc::now();
        }
        removed
    }

    pub fn robot_count(&self) -> usize {
        self.robots.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneBounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
    pub min_z: f64,
    pub max_z: f64,
}

impl ZoneBounds {
    pub fn new(min_x: f64, max_x: f64, min_y: f64, max_y: f64, min_z: f64, max_z: f64) -> Self {
        Self { min_x, max_x, min_y, max_y, min_z, max_z }
    }

    pub fn contains(&self, position: &Position3D) -> bool {
        position.x >= self.min_x && position.x <= self.max_x &&
        position.y >= self.min_y && position.y <= self.max_y &&
        position.z >= self.min_z && position.z <= self.max_z
    }
}

impl Default for ZoneBounds {
    fn default() -> Self {
        Self {
            min_x: 0.0,
            max_x: 0.0,
            min_y: 0.0,
            max_y: 0.0,
            min_z: 0.0,
            max_z: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetSnapshot {
    pub timestamp: DateTime<utc::Utc>,
    pub zones: HashMap<String, ZoneState>,
    pub total_robots: usize,
    pub active_robots: usize,
}

impl FleetSnapshot {
    pub fn new() -> Self {
        Self {
            timestamp: Utc::now(),
            zones: HashMap::new(),
            total_robots: 0,
            active_robots: 0,
        }
    }

    pub fn from_zones(zones: Vec<ZoneState>) -> Self {
        let total_robots: usize = zones.iter().map(|z| z.robot_count()).sum();
        let mut zone_map = HashMap::new();
        for zone in zones {
            zone_map.insert(zone.zone_id.clone(), zone);
        }
        Self {
            timestamp: Utc::now(),
            zones: zone_map,
            total_robots,
            active_robots: total_robots,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RobotRegistration {
    pub robot_id: String,
    pub name: String,
    pub robot_type: String,
    pub capabilities: Vec<String>,
    pub initial_zone: String,
    pub metadata: HashMap<String, String>,
}

impl RobotRegistration {
    pub fn new(robot_id: String, name: String, robot_type: String) -> Self {
        Self {
            robot_id,
            name,
            robot_type,
            capabilities: Vec::new(),
            initial_zone: "default".to_string(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_capabilities(mut self, caps: Vec<String>) -> Self {
        self.capabilities = caps;
        self
    }

    pub fn with_zone(mut self, zone: String) -> Self {
        self.initial_zone = zone;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneTransfer {
    pub robot_id: String,
    pub from_zone: String,
    pub to_zone: String,
    pub timestamp: DateTime<Utc>,
    pub reason: String,
}

impl ZoneTransfer {
    pub fn new(robot_id: String, from_zone: String, to_zone: String, reason: String) -> Self {
        Self {
            robot_id,
            from_zone,
            to_zone,
            timestamp: Utc::now(),
            reason,
        }
    }
}

mod utc {
    pub use chrono::Utc;
}