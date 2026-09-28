use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::cmp::Ordering;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LWWRegister<T> {
    value: Option<T>,
    timestamp: u64,
    node_id: String,
}

impl<T: Clone> LWWRegister<T> {
    pub fn new(node_id: String) -> Self {
        Self {
            value: None,
            timestamp: 0,
            node_id,
        }
    }

    pub fn set(&mut self, value: T, timestamp: u64) {
        if timestamp > self.timestamp {
            self.value = Some(value);
            self.timestamp = timestamp;
        }
    }

    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    pub fn merge(&mut self, other: &LWWRegister<T>) {
        if other.timestamp > self.timestamp {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
            self.node_id = other.node_id.clone();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCounter {
    increments: HashMap<String, u64>,
}

impl GCounter {
    pub fn new() -> Self {
        Self {
            increments: HashMap::new(),
        }
    }

    pub fn increment(&mut self, node_id: &str) {
        let entry = self.increments.entry(node_id.to_string()).or_insert(0);
        *entry += 1;
    }

    pub fn get(&self) -> u64 {
        self.increments.values().sum()
    }

    pub fn merge(&mut self, other: &GCounter) {
        for (node, value) in &other.increments {
            let entry = self.increments.entry(node.clone()).or_insert(0);
            *entry = (*entry).max(*value);
        }
    }
}

impl Default for GCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PNCounter {
    positives: HashMap<String, u64>,
    negatives: HashMap<String, u64>,
}

impl PNCounter {
    pub fn new() -> Self {
        Self {
            positives: HashMap::new(),
            negatives: HashMap::new(),
        }
    }

    pub fn increment(&mut self, node_id: &str) {
        let entry = self.positives.entry(node_id.to_string()).or_insert(0);
        *entry += 1;
    }

    pub fn decrement(&mut self, node_id: &str) {
        let entry = self.negatives.entry(node_id.to_string()).or_insert(0);
        *entry += 1;
    }

    pub fn get(&self) -> i64 {
        let pos: u64 = self.positives.values().sum();
        let neg: u64 = self.negatives.values().sum();
        pos as i64 - neg as i64
    }

    pub fn merge(&mut self, other: &PNCounter) {
        for (node, value) in &other.positives {
            let entry = self.positives.entry(node.clone()).or_insert(0);
            *entry = (*entry).max(*value);
        }
        for (node, value) in &other.negatives {
            let entry = self.negatives.entry(node.clone()).or_insert(0);
            *entry = (*entry).max(*value);
        }
    }
}

impl Default for PNCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LWWMap<K: Eq + Hash + Clone, V> {
    entries: HashMap<K, LWWRegister<V>>,
}

impl<K: Eq + Hash + Clone + Serialize + for<'de> Deserialize<'de>, V: Clone> LWWMap<K, V> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn insert(&mut self, key: K, value: V, timestamp: u64, node_id: &str) {
        let entry = self.entries.entry(key).or_insert_with(|| LWWRegister::new(node_id.to_string()));
        entry.set(value, timestamp);
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key).and_then(|r| r.get())
    }

    pub fn remove(&mut self, key: &K) {
        self.entries.remove(key);
    }

    pub fn keys(&self) -> Vec<&K> {
        self.entries.keys().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn merge(&mut self, other: &LWWMap<K, V>) {
        for (key, other_entry) in &other.entries {
            let entry = self.entries.entry(key.clone()).or_insert_with(|| {
                LWWRegister::new(other_entry.node_id.clone())
            });
            entry.merge(other_entry);
        }
    }
}

impl<K: Eq + Hash + Clone, V> Default for LWWMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ORSet<T: Eq + Hash + Clone + Serialize> {
    added: HashMap<String, HashSet<T>>,
    removed: HashMap<String, HashSet<T>>,
}

impl<T: Eq + Hash + Clone + Serialize> ORSet<T> {
    pub fn new() -> Self {
        Self {
            added: HashMap::new(),
            removed: HashMap::new(),
        }
    }

    pub fn add(&mut self, element: T, node_id: &str) {
        let set = self.added.entry(node_id.to_string()).or_insert_with(HashSet::new);
        set.insert(element);
    }

    pub fn remove(&mut self, element: &T, node_id: &str) {
        let set = self.removed.entry(node_id.to_string()).or_insert_with(HashSet::new);
        set.insert(element.clone());
    }

    pub fn contains(&self, element: &T) -> bool {
        for (node_id, added) in &self.added {
            if added.contains(element) {
                let removed = self.removed.get(node_id);
                if removed.is_none() || !removed.unwrap().contains(element) {
                    return true;
                }
            }
        }
        false
    }

    pub fn elements(&self) -> Vec<T> {
        let mut result = HashSet::new();
        for (node_id, added) in &self.added {
            for elem in added {
                let removed = self.removed.get(node_id);
                if removed.is_none() || !removed.unwrap().contains(elem) {
                    result.insert(elem.clone());
                }
            }
        }
        result.into_iter().collect()
    }

    pub fn merge(&mut self, other: &ORSet<T>) {
        for (node_id, elements) in &other.added {
            let set = self.added.entry(node_id.clone()).or_insert_with(HashSet::new);
            set.extend(elements.clone());
        }
        for (node_id, elements) in &other.removed {
            let set = self.removed.entry(node_id.clone()).or_insert_with(HashSet::new);
            set.extend(elements.clone());
        }
    }
}

impl<T: Eq + Hash + Clone + Serialize> Default for ORSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RGA<T: Eq + Hash + Clone + Serialize> {
    entries: Vec<RGAEntry<T>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RGAEntry<T> {
    id: String,
    value: Option<T>,
    timestamp: u64,
    node_id: String,
    next: Option<String>,
}

impl<T: Eq + Hash + Clone + Serialize> RGA<T> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn insert_after(&mut self, after_id: &str, value: T, id: String, timestamp: u64, node_id: String) {
        let entry = RGAEntry {
            id: id.clone(),
            value: Some(value),
            timestamp,
            node_id,
            next: None,
        };

        if let Some(pos) = self.entries.iter().position(|e| e.id == after_id) {
            if let Some(next_id) = self.entries[pos].next.clone() {
                if let Some(next_pos) = self.entries.iter().position(|e| e.id == next_id) {
                    entry.next = Some(next_id);
                    self.entries[pos].next = Some(id);
                    self.entries.insert(next_pos, entry);
                    return;
                }
            }
            self.entries[pos].next = Some(id);
        }

        self.entries.push(entry);
    }

    pub fn remove(&mut self, id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.value = None;
        }
    }

    pub fn values(&self) -> Vec<T> {
        self.entries.iter()
            .filter_map(|e| e.value.clone())
            .collect()
    }

    pub fn merge(&mut self, other: &RGA<T>) {
        for entry in &other.entries {
            if !self.entries.iter().any(|e| e.id == entry.id) {
                let mut new_entry = entry.clone();
                if let Some(after_id) = self.find_insert_position(&entry.id, entry.timestamp, &entry.node_id) {
                    new_entry.next = Some(after_id.clone());
                    if let Some(pos) = self.entries.iter().position(|e| e.id == after_id) {
                        self.entries.insert(pos, new_entry);
                    } else {
                        self.entries.push(new_entry);
                    }
                } else {
                    self.entries.push(new_entry);
                }
            }
        }
    }

    fn find_insert_position(&self, id: &str, timestamp: u64, node_id: &str) -> Option<String> {
        let mut result = None;
        for entry in &self.entries {
            if entry.timestamp < timestamp || (entry.timestamp == timestamp && entry.node_id < node_id.to_string()) {
                result = Some(entry.id.clone());
            }
        }
        result
    }
}

impl<T: Eq + Hash + Clone + Serialize> Default for RGA<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoPhaseSet<T: Eq + Hash + Clone + Serialize> {
    added: ORSet<T>,
    removed: ORSet<T>,
}

impl<T: Eq + Hash + Clone + Serialize> TwoPhaseSet<T> {
    pub fn new() -> Self {
        Self {
            added: ORSet::new(),
            removed: ORSet::new(),
        }
    }

    pub fn add(&mut self, element: T, node_id: &str) {
        if !self.removed.contains(&element) {
            self.added.add(element, node_id);
        }
    }

    pub fn remove(&mut self, element: &T, node_id: &str) {
        self.removed.remove(element, node_id);
        self.added.remove(element, node_id);
    }

    pub fn contains(&self, element: &T) -> bool {
        self.added.contains(element) && !self.removed.contains(element)
    }

    pub fn elements(&self) -> Vec<T> {
        self.added.elements()
            .into_iter()
            .filter(|e| !self.removed.contains(e))
            .collect()
    }

    pub fn merge(&mut self, other: &TwoPhaseSet<T>) {
        self.added.merge(&other.added);
        self.removed.merge(&other.removed);
    }
}

impl<T: Eq + Hash + Clone + Serialize> Default for TwoPhaseSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCounterMap {
    counters: HashMap<String, GCounter>,
}

impl GCounterMap {
    pub fn new() -> Self {
        Self {
            counters: HashMap::new(),
        }
    }

    pub fn increment(&mut self, key: &str, node_id: &str) {
        let counter = self.counters.entry(key.to_string()).or_insert_with(GCounter::new);
        counter.increment(node_id);
    }

    pub fn get(&self, key: &str) -> u64 {
        self.counters.get(key).map(|c| c.get()).unwrap_or(0)
    }

    pub fn total(&self) -> u64 {
        self.counters.values().map(|c| c.get()).sum()
    }

    pub fn merge(&mut self, other: &GCounterMap) {
        for (key, other_counter) in &other.counters {
            let counter = self.counters.entry(key.clone()).or_insert_with(GCounter::new);
            counter.merge(other_counter);
        }
    }
}

impl Default for GCounterMap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flag {
    value: bool,
    timestamp: u64,
    node_id: String,
}

impl Flag {
    pub fn new(node_id: String) -> Self {
        Self {
            value: false,
            timestamp: 0,
            node_id,
        }
    }

    pub fn enable(&mut self, timestamp: u64) {
        if timestamp > self.timestamp {
            self.value = true;
            self.timestamp = timestamp;
        }
    }

    pub fn disable(&mut self, timestamp: u64) {
        if timestamp > self.timestamp {
            self.value = false;
            self.timestamp = timestamp;
        }
    }

    pub fn get(&self) -> bool {
        self.value
    }

    pub fn merge(&mut self, other: &Flag) {
        if other.timestamp > self.timestamp {
            self.value = other.value;
            self.timestamp = other.timestamp;
            self.node_id = other.node_id.clone();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedRemoveSet<T: Eq + Hash + Clone + Serialize> {
    items: HashMap<T, HashSet<String>>,
    tombstones: HashMap<T, HashSet<String>>,
}

impl<T: Eq + Hash + Clone + Serialize> ObservedRemoveSet<T> {
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            tombstones: HashMap::new(),
        }
    }

    pub fn add(&mut self, element: T, node_id: &str) {
        let entry = self.items.entry(element.clone()).or_insert_with(HashSet::new);
        entry.insert(node_id.to_string());
        self.tombstones.remove(&element);
    }

    pub fn remove(&mut self, element: &T, node_id: &str) {
        let entry = self.tombstones.entry(element.clone()).or_insert_with(HashSet::new);
        entry.insert(node_id.to_string());
        self.items.remove(element);
    }

    pub fn contains(&self, element: &T) -> bool {
        self.items.contains_key(element)
    }

    pub fn elements(&self) -> Vec<T> {
        self.items.keys().cloned().collect()
    }

    pub fn merge(&mut self, other: &ObservedRemoveSet<T>) {
        for (element, nodes) in &other.items {
            let entry = self.items.entry(element.clone()).or_insert_with(HashSet::new);
            entry.extend(nodes.clone());
        }
        for (element, nodes) in &other.tombstones {
            let entry = self.tombstones.entry(element.clone()).or_insert_with(HashSet::new);
            entry.extend(nodes.clone());
            self.items.remove(element);
        }
    }
}

impl<T: Eq + Hash + Clone + Serialize> Default for ObservedRemoveSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub trait CRDT: Clone {
    fn merge(&mut self, other: &Self);
}

impl<T: Clone> CRDT for LWWRegister<T> where T: Clone {
    fn merge(&mut self, other: &Self) {
        LWWRegister::merge(self, other);
    }
}

impl CRDT for GCounter {
    fn merge(&mut self, other: &Self) {
        GCounter::merge(self, other);
    }
}

impl CRDT for PNCounter {
    fn merge(&mut self, other: &Self) {
        PNCounter::merge(self, other);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lww_register() {
        let mut reg = LWWRegister::new("node1".to_string());
        reg.set("value1", 100);
        assert_eq!(reg.get(), Some(&"value1".to_string()));

        let mut reg2 = LWWRegister::new("node2".to_string());
        reg2.set("value2", 200);
        reg.merge(&reg2);
        assert_eq!(reg.get(), Some(&"value2".to_string()));
    }

    #[test]
    fn test_gcounter() {
        let mut counter = GCounter::new();
        counter.increment("node1");
        counter.increment("node1");
        counter.increment("node2");
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn test_pncounter() {
        let mut counter = PNCounter::new();
        counter.increment("node1");
        counter.increment("node1");
        counter.decrement("node1");
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn test_orset() {
        let mut set = ORSet::new();
        set.add("item1".to_string(), "node1");
        set.add("item2".to_string(), "node1");
        assert!(set.contains(&"item1".to_string()));
        assert_eq!(set.elements().len(), 2);
    }
}