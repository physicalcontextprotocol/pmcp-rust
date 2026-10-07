use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::crdt::{Position3D, NodeId};
use crate::state::ZoneBounds;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PartitionKey {
    pub zone: String,
    pub shard: u32,
}

impl PartitionKey {
    pub fn new(zone: String, shard: u32) -> Self {
        Self { zone, shard }
    }

    pub fn from_position(position: &Position3D, zone: &str, num_shards: u32) -> Self {
        let shard = ((position.x + position.y).abs() as u32) % num_shards;
        Self::new(zone.to_string(), shard)
    }

    pub fn to_string(&self) -> String {
        format!("{}:{}", self.zone, self.shard)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZonePartition {
    pub zone_id: String,
    pub bounds: ZoneBounds,
    pub shards: Vec<Shard>,
    pub num_shards: u32,
    pub robots: HashMap<String, u32>,
}

impl ZonePartition {
    pub fn new(zone_id: String, bounds: ZoneBounds, num_shards: u32) -> Self {
        let shards = (0..num_shards)
            .map(|i| Shard {
                shard_id: i,
                robot_ids: Vec::new(),
                version: 0,
            })
            .collect();

        Self {
            zone_id,
            bounds,
            shards,
            num_shards,
            robots: HashMap::new(),
        }
    }

    pub fn assign_robot(&mut self, robot_id: String, position: &Position3D) -> u32 {
        let shard = ((position.x + position.y).abs() as u32) % self.num_shards;
        self.shards[shard as usize].robot_ids.push(robot_id.clone());
        self.shards[shard as usize].version += 1;
        self.robots.insert(robot_id, shard);
        shard
    }

    pub fn remove_robot(&mut self, robot_id: &str) -> Option<u32> {
        if let Some(shard) = self.robots.remove(robot_id) {
            for s in &mut self.shards {
                if let Some(idx) = s.robot_ids.iter().position(|r| r == robot_id) {
                    s.robot_ids.remove(idx);
                    s.version += 1;
                    return Some(shard);
                }
            }
        }
        None
    }

    pub fn get_shard(&self, shard_id: u32) -> Option<&Shard> {
        self.shards.get(shard_id as usize)
    }

    pub fn rebalance(&mut self, target_distribution: &HashMap<String, f64>) {
        let total_robots: usize = self.robots.len();
        if total_robots == 0 {
            return;
        }

        let per_shard = (total_robots as f64 / self.num_shards as f64).ceil() as usize;

        for shard in &mut self.shards {
            if shard.robot_ids.len() > per_shard {
                let excess = shard.robot_ids.len() - per_shard;
                let to_move: Vec<String> = shard.robot_ids.drain(0..excess).collect();
                for robot_id in to_move {
                    self.robots.remove(&robot_id);
                }
            }
        }
    }

    pub fn get_robots_in_shard(&self, shard_id: u32) -> Option<&Vec<String>> {
        self.shards.get(shard_id as usize).map(|s| &s.robot_ids)
    }

    pub fn total_robots(&self) -> usize {
        self.robots.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    pub shard_id: u32,
    pub robot_ids: Vec<String>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionStats {
    pub zone_id: String,
    pub total_robots: usize,
    pub shards: Vec<ShardStats>,
    pub load_distribution: Vec<f64>,
    pub rebalancing_needed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardStats {
    pub shard_id: u32,
    pub robot_count: usize,
    pub load_factor: f64,
}

impl PartitionStats {
    pub fn analyze(partition: &ZonePartition) -> Self {
        let total_robots = partition.total_robots();
        let shards: Vec<ShardStats> = partition
            .shards
            .iter()
            .map(|s| ShardStats {
                shard_id: s.shard_id,
                robot_count: s.robot_ids.len(),
                load_factor: if total_robots > 0 {
                    s.robot_ids.len() as f64 / total_robots as f64
                } else {
                    0.0
                },
            })
            .collect();

        let load_distribution: Vec<f64> = shards.iter().map(|s| s.load_factor).collect();

        let mean = if !load_distribution.is_empty() {
            load_distribution.iter().sum::<f64>() / load_distribution.len() as f64
        } else {
            0.0
        };

        let variance: f64 = load_distribution
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>() / load_distribution.len() as f64;

        let rebalancing_needed = variance > 0.1;

        Self {
            zone_id: partition.zone_id.clone(),
            total_robots,
            shards,
            load_distribution,
            rebalancing_needed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_assignment() {
        let bounds = ZoneBounds::new(0.0, 100.0, 0.0, 100.0, 0.0, 10.0);
        let mut partition = ZonePartition::new("zone1".to_string(), bounds, 4);

        let pos1 = Position3D::new(10.0, 20.0, 0.0);
        let shard1 = partition.assign_robot("robot1".to_string(), &pos1);

        let pos2 = Position3D::new(30.0, 40.0, 0.0);
        let shard2 = partition.assign_robot("robot2".to_string(), &pos2);

        assert_eq!(partition.total_robots(), 2);
        assert!(shard1 < 4 && shard2 < 4);
    }
}