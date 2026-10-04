//! PCP Telemetry & Observability
//!
//! OpenTelemetry-compatible spans, metrics export, structured events,
//! and Prometheus-format scrape endpoint.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info};
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
//  SPAN / TRACE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanContext {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub sampled: bool,
}

impl SpanContext {
    pub fn new_root() -> Self {
        Self {
            trace_id: generate_id(16),
            span_id: generate_id(8),
            parent_span_id: None,
            sampled: true,
        }
    }

    pub fn child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: generate_id(8),
            parent_span_id: Some(self.span_id.clone()),
            sampled: self.sampled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub ctx: SpanContext,
    pub name: String,
    pub service: String,
    pub start_us: u64,
    pub end_us: Option<u64>,
    pub attributes: HashMap<String, serde_json::Value>,
    pub events: Vec<SpanEvent>,
    pub status: SpanStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpanStatus {
    Unset,
    Ok,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanEvent {
    pub timestamp_us: u64,
    pub name: String,
    pub attributes: HashMap<String, serde_json::Value>,
}

impl Span {
    pub fn new(name: impl Into<String>, service: impl Into<String>, ctx: SpanContext) -> Self {
        Self {
            ctx,
            name: name.into(),
            service: service.into(),
            start_us: now_us(),
            end_us: None,
            attributes: HashMap::new(),
            events: vec![],
            status: SpanStatus::Unset,
        }
    }

    pub fn set_attribute(&mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) {
        self.attributes.insert(key.into(), value.into());
    }

    pub fn add_event(&mut self, name: impl Into<String>, attrs: HashMap<String, serde_json::Value>) {
        self.events.push(SpanEvent {
            timestamp_us: now_us(),
            name: name.into(),
            attributes: attrs,
        });
    }

    pub fn finish(&mut self) {
        self.end_us = Some(now_us());
        if matches!(self.status, SpanStatus::Unset) {
            self.status = SpanStatus::Ok;
        }
    }

    pub fn finish_with_error(&mut self, msg: impl Into<String>) {
        self.end_us = Some(now_us());
        self.status = SpanStatus::Error(msg.into());
    }

    pub fn duration_us(&self) -> u64 {
        self.end_us.unwrap_or_else(now_us) - self.start_us
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  METRICS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetricValue {
    Counter(u64),
    Gauge(f64),
    Histogram { count: u64, sum: f64, buckets: Vec<(f64, u64)> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub name: String,
    pub labels: HashMap<String, String>,
    pub value: MetricValue,
    pub timestamp_ms: u64,
    pub help: String,
}

pub struct MetricsRegistry {
    counters: Arc<RwLock<HashMap<String, (u64, HashMap<String, String>, String)>>>,
    gauges: Arc<RwLock<HashMap<String, (f64, HashMap<String, String>, String)>>>,
    histograms: Arc<RwLock<HashMap<String, HistogramState>>>,
}

struct HistogramState {
    boundaries: Vec<f64>,
    counts: Vec<u64>,
    total_count: u64,
    total_sum: f64,
    labels: HashMap<String, String>,
    help: String,
}

impl HistogramState {
    fn new(boundaries: Vec<f64>, labels: HashMap<String, String>, help: String) -> Self {
        let n = boundaries.len() + 1;
        Self {
            boundaries,
            counts: vec![0; n],
            total_count: 0,
            total_sum: 0.0,
            labels,
            help,
        }
    }

    fn observe(&mut self, value: f64) {
        self.total_count += 1;
        self.total_sum += value;
        for (i, &bound) in self.boundaries.iter().enumerate() {
            if value <= bound {
                self.counts[i] += 1;
                return;
            }
        }
        // +Inf bucket
        *self.counts.last_mut().unwrap() += 1;
    }

    fn to_metric(&self, name: &str, timestamp_ms: u64) -> MetricPoint {
        let mut buckets: Vec<(f64, u64)> = self.boundaries.iter().zip(self.counts.iter())
            .map(|(&b, &c)| (b, c))
            .collect();
        buckets.push((f64::INFINITY, *self.counts.last().unwrap_or(&0)));

        MetricPoint {
            name: name.to_string(),
            labels: self.labels.clone(),
            value: MetricValue::Histogram {
                count: self.total_count,
                sum: self.total_sum,
                buckets,
            },
            timestamp_ms,
            help: self.help.clone(),
        }
    }
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            counters: Arc::new(RwLock::new(HashMap::new())),
            gauges: Arc::new(RwLock::new(HashMap::new())),
            histograms: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn counter_inc(&self, name: &str, labels: HashMap<String, String>, help: &str, by: u64) {
        let mut map = self.counters.write().await;
        let entry = map.entry(make_key(name, &labels))
            .or_insert((0, labels, help.to_string()));
        entry.0 += by;
    }

    pub async fn gauge_set(&self, name: &str, labels: HashMap<String, String>, help: &str, value: f64) {
        let mut map = self.gauges.write().await;
        map.insert(make_key(name, &labels), (value, labels, help.to_string()));
    }

    pub async fn gauge_add(&self, name: &str, labels: HashMap<String, String>, help: &str, delta: f64) {
        let mut map = self.gauges.write().await;
        let entry = map.entry(make_key(name, &labels))
            .or_insert((0.0, labels, help.to_string()));
        entry.0 += delta;
    }

    pub async fn histogram_observe(
        &self,
        name: &str,
        labels: HashMap<String, String>,
        help: &str,
        boundaries: &[f64],
        value: f64,
    ) {
        let mut map = self.histograms.write().await;
        let h = map.entry(make_key(name, &labels))
            .or_insert_with(|| HistogramState::new(
                boundaries.to_vec(),
                labels,
                help.to_string(),
            ));
        h.observe(value);
    }

    /// Export all metrics as a list of MetricPoints.
    pub async fn collect(&self) -> Vec<MetricPoint> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut points = vec![];

        for (key, (val, labels, help)) in self.counters.read().await.iter() {
            let name = key.split('{').next().unwrap_or(key).to_string();
            points.push(MetricPoint {
                name,
                labels: labels.clone(),
                value: MetricValue::Counter(*val),
                timestamp_ms: ts,
                help: help.clone(),
            });
        }

        for (key, (val, labels, help)) in self.gauges.read().await.iter() {
            let name = key.split('{').next().unwrap_or(key).to_string();
            points.push(MetricPoint {
                name,
                labels: labels.clone(),
                value: MetricValue::Gauge(*val),
                timestamp_ms: ts,
                help: help.clone(),
            });
        }

        for (key, h) in self.histograms.read().await.iter() {
            let name = key.split('{').next().unwrap_or(key).to_string();
            points.push(h.to_metric(&name, ts));
        }

        points
    }

    /// Render metrics in Prometheus text format.
    pub async fn prometheus_text(&self) -> String {
        let mut out = String::with_capacity(4096);
        for point in self.collect().await {
            let labels = if point.labels.is_empty() {
                String::new()
            } else {
                let pairs: Vec<String> = point.labels.iter()
                    .map(|(k, v)| format!("{}=\"{}\"", k, v))
                    .collect();
                format!("{{{}}}", pairs.join(","))
            };

            if !point.help.is_empty() {
                out.push_str(&format!("# HELP {} {}\n", point.name, point.help));
            }

            match &point.value {
                MetricValue::Counter(v) => {
                    out.push_str(&format!("# TYPE {} counter\n", point.name));
                    out.push_str(&format!("{}{} {}\n", point.name, labels, v));
                }
                MetricValue::Gauge(v) => {
                    out.push_str(&format!("# TYPE {} gauge\n", point.name));
                    out.push_str(&format!("{}{} {}\n", point.name, labels, v));
                }
                MetricValue::Histogram { count, sum, buckets } => {
                    out.push_str(&format!("# TYPE {} histogram\n", point.name));
                    for (le, c) in buckets {
                        let le_str = if le.is_infinite() { "+Inf".to_string() } else { le.to_string() };
                        out.push_str(&format!(
                            "{}_bucket{{le=\"{}\"{}}} {}\n",
                            point.name,
                            le_str,
                            if labels.is_empty() { "" } else { &labels[1..labels.len()-1] },
                            c
                        ));
                    }
                    out.push_str(&format!("{}_sum{} {}\n", point.name, labels, sum));
                    out.push_str(&format!("{}_count{} {}\n", point.name, labels, count));
                }
            }
            out.push('\n');
        }
        out
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self { Self::new() }
}

// ─────────────────────────────────────────────────────────────────────────────
//  STRUCTURED EVENT LOG
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventSeverity {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredEvent {
    pub id: String,
    pub timestamp_ms: u64,
    pub severity: EventSeverity,
    pub component: String,
    pub robot_id: Option<String>,
    pub message: String,
    pub payload: HashMap<String, serde_json::Value>,
    pub trace_id: Option<String>,
}

pub struct EventLog {
    events: Arc<Mutex<VecDeque<StructuredEvent>>>,
    max_size: usize,
    subscribers: Arc<Mutex<Vec<tokio::sync::mpsc::Sender<StructuredEvent>>>>,
}

impl EventLog {
    pub fn new(max_size: usize) -> Self {
        Self {
            events: Arc::new(Mutex::new(VecDeque::with_capacity(max_size))),
            max_size,
            subscribers: Arc::new(Mutex::new(vec![])),
        }
    }

    pub async fn emit(&self, event: StructuredEvent) {
        let mut events = self.events.lock().await;
        if events.len() >= self.max_size {
            events.pop_front();
        }
        events.push_back(event.clone());

        // Notify subscribers
        let mut subs = self.subscribers.lock().await;
        subs.retain(|tx| {
            tx.try_send(event.clone()).is_ok()
        });
    }

    pub async fn subscribe(&self) -> tokio::sync::mpsc::Receiver<StructuredEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        self.subscribers.lock().await.push(tx);
        rx
    }

    pub async fn query(
        &self,
        component: Option<&str>,
        robot_id: Option<&str>,
        min_severity: Option<u8>,
        limit: usize,
    ) -> Vec<StructuredEvent> {
        let events = self.events.lock().await;
        events.iter()
            .rev()
            .filter(|e| {
                component.map(|c| e.component == c).unwrap_or(true)
                    && robot_id.map(|r| e.robot_id.as_deref() == Some(r)).unwrap_or(true)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    pub async fn count(&self) -> usize {
        self.events.lock().await.len()
    }
}

impl Default for EventLog {
    fn default() -> Self { Self::new(100_000) }
}

// ─────────────────────────────────────────────────────────────────────────────
//  TELEMETRY PIPELINE
// ─────────────────────────────────────────────────────────────────────────────

pub struct TelemetryPipeline {
    pub metrics: Arc<MetricsRegistry>,
    pub events: Arc<EventLog>,
    span_buffer: Arc<Mutex<VecDeque<Span>>>,
    span_buffer_max: usize,
}

impl TelemetryPipeline {
    pub fn new(span_buffer_max: usize) -> Self {
        Self {
            metrics: Arc::new(MetricsRegistry::new()),
            events: Arc::new(EventLog::new(100_000)),
            span_buffer: Arc::new(Mutex::new(VecDeque::with_capacity(span_buffer_max))),
            span_buffer_max,
        }
    }

    pub async fn record_span(&self, span: Span) {
        let mut buf = self.span_buffer.lock().await;
        if buf.len() >= self.span_buffer_max {
            buf.pop_front();
        }
        buf.push_back(span);
    }

    pub async fn drain_spans(&self) -> Vec<Span> {
        let mut buf = self.span_buffer.lock().await;
        buf.drain(..).collect()
    }

    /// Record an actuation event with standard metrics.
    pub async fn record_actuation(
        &self,
        robot_id: &str,
        actuation: &str,
        success: bool,
        duration_ms: f64,
        energy_j: f64,
    ) {
        let labels: HashMap<String, String> = [
            ("robot_id".to_string(), robot_id.to_string()),
            ("actuation".to_string(), actuation.to_string()),
            ("success".to_string(), success.to_string()),
        ].into();

        self.metrics.counter_inc(
            "pcp_actuation_total",
            labels.clone(),
            "Total actuations executed",
            1,
        ).await;

        let boundaries = vec![1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0];
        self.metrics.histogram_observe(
            "pcp_actuation_duration_ms",
            labels.clone(),
            "Actuation duration in milliseconds",
            &boundaries,
            duration_ms,
        ).await;

        let energy_labels: HashMap<String, String> = [
            ("robot_id".to_string(), robot_id.to_string()),
        ].into();
        self.metrics.gauge_add(
            "pcp_energy_used_joules",
            energy_labels,
            "Cumulative energy used by robot",
            energy_j,
        ).await;
    }

    pub async fn record_safety_violation(
        &self,
        robot_id: &str,
        violation_type: &str,
        severity: u8,
    ) {
        let labels: HashMap<String, String> = [
            ("robot_id".to_string(), robot_id.to_string()),
            ("violation_type".to_string(), violation_type.to_string()),
        ].into();

        self.metrics.counter_inc(
            "pcp_safety_violations_total",
            labels,
            "Total safety violations detected",
            1,
        ).await;

        let sev = match severity {
            0..=2 => EventSeverity::Info,
            3..=5 => EventSeverity::Warning,
            6..=8 => EventSeverity::Error,
            _ => EventSeverity::Critical,
        };

        self.events.emit(StructuredEvent {
            id: generate_id(8),
            timestamp_ms: now_ms(),
            severity: sev,
            component: "safety".to_string(),
            robot_id: Some(robot_id.to_string()),
            message: format!("Safety violation: {}", violation_type),
            payload: [("violation_type".to_string(), serde_json::Value::String(violation_type.to_string()))].into(),
            trace_id: None,
        }).await;
    }
}

impl Default for TelemetryPipeline {
    fn default() -> Self { Self::new(10_000) }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HELPERS
// ─────────────────────────────────────────────────────────────────────────────

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn generate_id(len: usize) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    Instant::now().elapsed().subsec_nanos().hash(&mut h);
    format!("{:0>width$x}", h.finish(), width = len * 2)[..len * 2].to_string()
}

fn make_key(name: &str, labels: &HashMap<String, String>) -> String {
    if labels.is_empty() {
        return name.to_string();
    }
    let mut pairs: Vec<String> = labels.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    pairs.sort();
    format!("{}{{{}}}", name, pairs.join(","))
}
