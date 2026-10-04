//! PCP Rate Limiter & Scheduler
//!
//! Token-bucket + sliding-window rate limiters for actuation frequency control,
//! API rate limiting, and energy budget enforcement.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::error::{PcpError, PcpErrorCode};

// ─────────────────────────────────────────────────────────────────────────────
//  TOKEN BUCKET
// ─────────────────────────────────────────────────────────────────────────────

/// Classic token-bucket rate limiter.
pub struct TokenBucket {
    capacity: f64,       // max tokens
    tokens: f64,         // current tokens
    refill_rate: f64,    // tokens per second
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Try to consume `n` tokens. Returns true if successful.
    pub fn try_consume(&mut self, n: f64) -> bool {
        self.refill();
        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }

    /// Return the current token count after refill.
    pub fn tokens(&mut self) -> f64 {
        self.refill();
        self.tokens
    }

    /// Time until `n` tokens become available, in milliseconds.
    pub fn wait_ms(&mut self, n: f64) -> u64 {
        self.refill();
        if self.tokens >= n {
            return 0;
        }
        let deficit = n - self.tokens;
        let wait_secs = deficit / self.refill_rate;
        (wait_secs * 1000.0) as u64
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  SLIDING WINDOW COUNTER
// ─────────────────────────────────────────────────────────────────────────────

/// Sliding-window rate limiter (tracks exact timestamps within window).
pub struct SlidingWindowCounter {
    window_ms: u64,
    max_requests: usize,
    timestamps: std::collections::VecDeque<Instant>,
}

impl SlidingWindowCounter {
    pub fn new(window_ms: u64, max_requests: usize) -> Self {
        Self {
            window_ms,
            max_requests,
            timestamps: std::collections::VecDeque::new(),
        }
    }

    pub fn check_and_record(&mut self) -> bool {
        let now = Instant::now();
        let cutoff = now - Duration::from_millis(self.window_ms);

        // Evict expired timestamps
        while self.timestamps.front().map(|&t| t < cutoff).unwrap_or(false) {
            self.timestamps.pop_front();
        }

        if self.timestamps.len() >= self.max_requests {
            return false;
        }
        self.timestamps.push_back(now);
        true
    }

    pub fn current_count(&mut self) -> usize {
        let now = Instant::now();
        let cutoff = now - Duration::from_millis(self.window_ms);
        while self.timestamps.front().map(|&t| t < cutoff).unwrap_or(false) {
            self.timestamps.pop_front();
        }
        self.timestamps.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  ACTUATION RATE LIMITER
// ─────────────────────────────────────────────────────────────────────────────

/// Per-robot, per-actuation-type rate limiter.
pub struct ActuationRateLimiter {
    limiters: Arc<Mutex<HashMap<String, TokenBucket>>>,
    default_rate_hz: f64,
    default_burst: f64,
}

impl ActuationRateLimiter {
    pub fn new(default_rate_hz: f64, default_burst: f64) -> Self {
        Self {
            limiters: Arc::new(Mutex::new(HashMap::new())),
            default_rate_hz,
            default_burst,
        }
    }

    /// Register a custom rate for a specific robot+actuation pair.
    pub async fn register(
        &self,
        robot_id: &str,
        actuation: &str,
        rate_hz: f64,
        burst: f64,
    ) {
        let key = format!("{}:{}", robot_id, actuation);
        let mut map = self.limiters.lock().await;
        map.insert(key, TokenBucket::new(burst, rate_hz));
    }

    /// Check and consume one token. Returns Ok if permitted, Err if rate-limited.
    pub async fn check(&self, robot_id: &str, actuation: &str) -> Result<(), PcpError> {
        let key = format!("{}:{}", robot_id, actuation);
        let mut map = self.limiters.lock().await;
        let bucket = map.entry(key).or_insert_with(|| {
            TokenBucket::new(self.default_burst, self.default_rate_hz)
        });

        if bucket.try_consume(1.0) {
            Ok(())
        } else {
            let wait = bucket.wait_ms(1.0);
            warn!(
                "Rate limit exceeded for robot={} actuation={}, retry in {}ms",
                robot_id, actuation, wait
            );
            Err(PcpError::with_message(
                PcpErrorCode::RateLimited,
                format!("Rate limit exceeded; retry in {}ms", wait),
            ))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  ENERGY BUDGET TRACKER
// ─────────────────────────────────────────────────────────────────────────────

pub struct EnergyBudgetTracker {
    budgets: Arc<Mutex<HashMap<String, EnergyBucket>>>,
}

struct EnergyBucket {
    budget_joules: f64,
    used_joules: f64,
    refill_per_sec: f64,
    last_refill: Instant,
    cycle_start: Instant,
    cycle_duration: Duration,
}

impl EnergyBucket {
    fn new(budget_joules: f64, refill_per_sec: f64, cycle_secs: f64) -> Self {
        let now = Instant::now();
        Self {
            budget_joules,
            used_joules: 0.0,
            refill_per_sec,
            last_refill: now,
            cycle_start: now,
            cycle_duration: Duration::from_secs_f64(cycle_secs),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();

        // Cycle reset
        if now.duration_since(self.cycle_start) >= self.cycle_duration {
            self.used_joules = 0.0;
            self.cycle_start = now;
        }

        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let added = elapsed * self.refill_per_sec;
        self.used_joules = (self.used_joules - added).max(0.0);
        self.last_refill = now;
    }

    fn consume(&mut self, joules: f64) -> bool {
        self.refill();
        if self.used_joules + joules <= self.budget_joules {
            self.used_joules += joules;
            true
        } else {
            false
        }
    }

    fn remaining(&mut self) -> f64 {
        self.refill();
        (self.budget_joules - self.used_joules).max(0.0)
    }
}

impl EnergyBudgetTracker {
    pub fn new() -> Self {
        Self {
            budgets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_robot(
        &self,
        robot_id: &str,
        budget_joules: f64,
        refill_per_sec: f64,
        cycle_secs: f64,
    ) {
        let mut map = self.budgets.lock().await;
        map.insert(
            robot_id.to_string(),
            EnergyBucket::new(budget_joules, refill_per_sec, cycle_secs),
        );
    }

    pub async fn consume(
        &self,
        robot_id: &str,
        joules: f64,
    ) -> Result<f64, PcpError> {
        let mut map = self.budgets.lock().await;
        match map.get_mut(robot_id) {
            None => {
                // Unknown robot — allow by default
                Ok(f64::INFINITY)
            }
            Some(bucket) => {
                if bucket.consume(joules) {
                    Ok(bucket.remaining())
                } else {
                    Err(PcpError::with_message(
                        PcpErrorCode::EnergyBudget,
                        format!(
                            "Energy budget exceeded for robot={}: requested={:.1}J remaining={:.1}J",
                            robot_id,
                            joules,
                            bucket.remaining()
                        ),
                    ))
                }
            }
        }
    }

    pub async fn remaining(&self, robot_id: &str) -> Option<f64> {
        let mut map = self.budgets.lock().await;
        map.get_mut(robot_id).map(|b| b.remaining())
    }
}

impl Default for EnergyBudgetTracker {
    fn default() -> Self { Self::new() }
}

// ─────────────────────────────────────────────────────────────────────────────
//  PRIORITY SCHEDULER
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::BinaryHeap;
use std::cmp::Ordering;

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub priority: i32,
    pub robot_id: String,
    pub actuation: String,
    pub params: serde_json::Value,
    pub deadline_ms: u64,
    pub task_id: String,
}

// Reverse ordering so BinaryHeap becomes a min-heap on (priority, deadline)
impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool { self.task_id == other.task_id }
}
impl Eq for ScheduledTask {}
impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}
impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first; earlier deadline first as tiebreaker
        other.priority.cmp(&self.priority)
            .then(self.deadline_ms.cmp(&other.deadline_ms))
    }
}

pub struct PriorityScheduler {
    queue: Arc<Mutex<BinaryHeap<ScheduledTask>>>,
    max_queue_depth: usize,
}

impl PriorityScheduler {
    pub fn new(max_queue_depth: usize) -> Self {
        Self {
            queue: Arc::new(Mutex::new(BinaryHeap::new())),
            max_queue_depth,
        }
    }

    pub async fn enqueue(&self, task: ScheduledTask) -> Result<(), PcpError> {
        let mut q = self.queue.lock().await;
        if q.len() >= self.max_queue_depth {
            return Err(PcpError::with_message(
                PcpErrorCode::InternalError,
                "Scheduler queue full".to_string(),
            ));
        }
        q.push(task);
        Ok(())
    }

    pub async fn dequeue(&self) -> Option<ScheduledTask> {
        let mut q = self.queue.lock().await;
        q.pop()
    }

    pub async fn queue_depth(&self) -> usize {
        self.queue.lock().await.len()
    }

    pub async fn drain_expired(&self) -> Vec<ScheduledTask> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut q = self.queue.lock().await;
        let mut expired = vec![];
        let all: Vec<ScheduledTask> = q.drain().collect();
        for task in all {
            if task.deadline_ms < now_ms {
                expired.push(task);
            } else {
                q.push(task);
            }
        }
        expired
    }
}
