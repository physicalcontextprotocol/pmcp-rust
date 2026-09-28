//! P-MCP Metrics and Monitoring
//!
//! Comprehensive metrics collection including:
// - Performance metrics
// - Resource utilization
// - Safety statistics
// - Custom business metrics

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc};
use serde::{Deserialize, Serialize};
use chrono::Utc;
use async_trait::async_trait;

/// Metric Types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
}

/// Metric Value
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetricValue {
    Counter(u64),
    Gauge(f64),
    Histogram(Vec<f64>),
    Summary(f64),
}

/// Base Metric
#[derive(Debug, Clone)]
pub struct Metric {
    pub name: String,
    pub metric_type: MetricType,
    pub value: MetricValue,
    pub labels: HashMap<String, String>,
    pub timestamp: f64,
}

impl Metric {
    pub fn new(name: &str, metric_type: MetricType) -> Self {
        Self {
            name: name.to_string(),
            metric_type,
            value: match metric_type {
                MetricType::Counter => MetricValue::Counter(0),
                MetricType::Gauge => MetricValue::Gauge(0.0),
                MetricType::Histogram => MetricValue::Histogram(Vec::new()),
                MetricType::Summary => MetricValue::Summary(0.0),
            },
            labels: HashMap::new(),
            timestamp: Utc::now().timestamp() as f64,
        }
    }

    pub fn with_label(mut self, key: &str, value: &str) -> Self {
        self.labels.insert(key.to_string(), value.to_string());
        self
    }

    pub fn with_labels(mut self, labels: HashMap<String, String>) -> Self {
        self.labels.extend(labels);
        self
    }

    pub fn increment(&mut self, amount: u64) {
        if let MetricValue::Counter(v) = &mut self.value {
            *v += amount;
        }
        self.timestamp = Utc::now().timestamp() as f64;
    }

    pub fn set_gauge(&mut self, value: f64) {
        if let MetricValue::Gauge(v) = &mut self.value {
            *v = value;
        }
        self.timestamp = Utc::now().timestamp() as f64;
    }

    pub fn observe(&mut self, value: f64) {
        match &mut self.value {
            MetricValue::Histogram(v) => v.push(value),
            MetricValue::Summary(v) => *v = value,
            _ => {}
        }
        self.timestamp = Utc::now().timestamp() as f64;
    }
}

/// Counter Metric
#[derive(Debug, Clone)]
pub struct Counter {
    inner: Metric,
}

impl Counter {
    pub fn new(name: &str) -> Self {
        Self {
            inner: Metric::new(name, MetricType::Counter),
        }
    }

    pub fn with_label(self, key: &str, value: &str) -> Self {
        Self { inner: self.inner.with_label(key, value) }
    }

    pub fn inc(&mut self) {
        self.inner.increment(1);
    }

    pub fn inc_by(&mut self, amount: u64) {
        self.inner.increment(amount);
    }

    pub fn get(&self) -> u64 {
        if let MetricValue::Counter(v) = &self.inner.value {
            *v
        } else {
            0
        }
    }

    pub fn to_metric(&self) -> Metric {
        self.inner.clone()
    }
}

/// Gauge Metric
#[derive(Debug, Clone)]
pub struct Gauge {
    inner: Metric,
}

impl Gauge {
    pub fn new(name: &str) -> Self {
        Self {
            inner: Metric::new(name, MetricType::Gauge),
        }
    }

    pub fn with_label(self, key: &str, value: &str) -> Self {
        Self { inner: self.inner.with_label(key, value) }
    }

    pub fn set(&mut self, value: f64) {
        self.inner.set_gauge(value);
    }

    pub fn inc(&mut self) {
        if let MetricValue::Gauge(v) = &mut self.inner.value {
            *v += 1.0;
        }
        self.inner.timestamp = Utc::now().timestamp() as f64;
    }

    pub fn dec(&mut self) {
        if let MetricValue::Gauge(v) = &mut self.inner.value {
            *v -= 1.0;
        }
        self.inner.timestamp = Utc::now().timestamp() as f64;
    }

    pub fn get(&self) -> f64 {
        if let MetricValue::Gauge(v) = &self.inner.value {
            *v
        } else {
            0.0
        }
    }

    pub fn to_metric(&self) -> Metric {
        self.inner.clone()
    }
}

/// Histogram Metric
#[derive(Debug, Clone)]
pub struct Histogram {
    inner: Metric,
    buckets: Vec<f64>,
}

impl Histogram {
    pub fn new(name: &str, buckets: Vec<f64>) -> Self {
        let mut inner = Metric::new(name, MetricType::Histogram);
        inner.value = MetricValue::Histogram(Vec::new());
        Self {
            inner,
            buckets,
        }
    }

    pub fn with_label(self, key: &str, value: &str) -> Self {
        Self { inner: self.inner.with_label(key, value), buckets: self.buckets }
    }

    pub fn observe(&mut self, value: f64) {
        self.inner.observe(value);
    }

    pub fn get_percentile(&self, p: f64) -> f64 {
        // Standard linear-interpolation percentile (matches numpy's default
        // 'linear' method), not a crude nearest-rank/ceiling approach. The
        // previous ceil((p/100)*n) implementation systematically
        // overestimated percentiles for small sample sizes -- e.g. p50 of
        // [0.3, 0.7] should be 0.5 (the interpolated median), not 0.7. For
        // an observability system reporting P50/P95/P99 latencies, that's
        // a real accuracy bug, not just a style preference.
        if let MetricValue::Histogram(values) = &self.inner.value {
            if values.is_empty() {
                return 0.0;
            }
            let mut sorted = values.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = sorted.len();
            if n == 1 {
                return sorted[0];
            }
            let rank = (p / 100.0).clamp(0.0, 1.0) * (n - 1) as f64;
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            let frac = rank - lo as f64;
            sorted[lo] + (sorted[hi] - sorted[lo]) * frac
        } else {
            0.0
        }
    }

    pub fn to_metric(&self) -> Metric {
        self.inner.clone()
    }
}

/// Timer - helper for measuring duration
pub struct Timer {
    start: Instant,
    histogram: Option<Histogram>,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            histogram: None,
        }
    }

    pub fn with_histogram(mut self, h: Histogram) -> Self {
        self.histogram = Some(h);
        self
    }

    pub fn stop(&mut self) -> Duration {
        let elapsed = self.start.elapsed();
        if let Some(h) = &mut self.histogram {
            h.observe(elapsed.as_secs_f64());
        }
        elapsed
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

/// Server Metrics
#[derive(Debug, Clone)]
pub struct ServerMetrics {
    pub requests_total: Counter,
    pub requests_in_flight: Gauge,
    pub request_duration: Histogram,
    pub errors_total: Counter,
    pub bytes_received: Counter,
    pub bytes_sent: Counter,
    pub active_sessions: Gauge,
    pub active_robots: Gauge,
    pub estop_activations: Counter,
    pub shadow_blocks: Counter,
    pub constitution_blocks: Counter,
    pub lease_requests: Counter,
    pub lease_grants: Counter,
    pub lease_denials: Counter,
}

impl ServerMetrics {
    pub fn new() -> Self {
        Self {
            requests_total: Counter::new("pmcp_requests_total")
                .with_label("method", "all"),
            requests_in_flight: Gauge::new("pmcp_requests_in_flight"),
            request_duration: Histogram::new("pmcp_request_duration_seconds", vec![
                0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0,
            ]),
            errors_total: Counter::new("pmcp_errors_total")
                .with_label("type", "all"),
            bytes_received: Counter::new("pmcp_bytes_received"),
            bytes_sent: Counter::new("pmcp_bytes_sent"),
            active_sessions: Gauge::new("pmcp_active_sessions"),
            active_robots: Gauge::new("pmcp_active_robots"),
            estop_activations: Counter::new("pmcp_estop_activations_total"),
            shadow_blocks: Counter::new("pmcp_shadow_blocks_total"),
            constitution_blocks: Counter::new("pmcp_constitution_blocks_total"),
            lease_requests: Counter::new("pmcp_lease_requests_total"),
            lease_grants: Counter::new("pmcp_lease_grants_total"),
            lease_denials: Counter::new("pmcp_lease_denials_total"),
        }
    }

    pub fn record_request(&mut self, method: &str, duration: Duration, success: bool) {
        self.requests_total.inc();
        self.request_duration.observe(duration.as_secs_f64());
        
        if !success {
            self.errors_total.clone().with_label("type", method).inc();
        }
    }

    pub fn record_shadow_block(&mut self) {
        self.shadow_blocks.inc();
    }

    pub fn record_constitution_block(&mut self) {
        self.constitution_blocks.inc();
    }

    pub fn record_estop(&mut self) {
        self.estop_activations.inc();
    }

    pub fn record_lease(&mut self, granted: bool) {
        self.lease_requests.inc();
        if granted {
            self.lease_grants.inc();
        } else {
            self.lease_denials.inc();
        }
    }
}

impl Default for ServerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics Registry
pub struct MetricsRegistry {
    metrics: Arc<RwLock<HashMap<String, Metric>>>,
    exporters: Vec<Box<dyn MetricsExporter>>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(HashMap::new())),
            exporters: Vec::new(),
        }
    }

    pub async fn register(&self, metric: Metric) {
        let mut metrics = self.metrics.write().await;
        metrics.insert(metric.name.clone(), metric);
    }

    pub async fn get(&self, name: &str) -> Option<Metric> {
        let metrics = self.metrics.read().await;
        metrics.get(name).cloned()
    }

    pub async fn list(&self) -> Vec<Metric> {
        let metrics = self.metrics.read().await;
        metrics.values().cloned().collect()
    }

    pub async fn increment(&self, name: &str, amount: u64) {
        let mut metrics = self.metrics.write().await;
        if let Some(metric) = metrics.get_mut(name) {
            metric.increment(amount);
        }
    }

    pub async fn set_gauge(&self, name: &str, value: f64) {
        let mut metrics = self.metrics.write().await;
        if let Some(metric) = metrics.get_mut(name) {
            metric.set_gauge(value);
        }
    }

    pub async fn observe(&self, name: &str, value: f64) {
        let mut metrics = self.metrics.write().await;
        if let Some(metric) = metrics.get_mut(name) {
            metric.observe(value);
        }
    }

    pub fn add_exporter(&mut self, exporter: impl MetricsExporter + 'static) {
        self.exporters.push(Box::new(exporter));
    }

    pub async fn export(&self) -> Vec<serde_json::Value> {
        let metrics = self.metrics.read().await;
        let mut results = Vec::new();

        for exporter in &self.exporters {
            results.push(exporter.export(metrics.values().cloned().collect()));
        }

        results
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics Exporter Trait
pub trait MetricsExporter: Send + Sync {
    fn export(&self, metrics: Vec<Metric>) -> serde_json::Value;
}

/// Prometheus Exporter
pub struct PrometheusExporter {
    prefix: String,
}

impl PrometheusExporter {
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
        }
    }
}

impl MetricsExporter for PrometheusExporter {
    fn export(&self, metrics: Vec<Metric>) -> serde_json::Value {
        let mut output = String::new();

        for metric in metrics {
            let metric_name = format!("{}_{}", self.prefix, metric.name);
            
            // Add labels
            let labels: Vec<String> = metric.labels.iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            let label_str = if labels.is_empty() {
                String::new()
            } else {
                format!("{{{}}}", labels.join(","))
            };

            match &metric.value {
                MetricValue::Counter(v) => {
                    output.push_str(&format!("# TYPE {} counter\n", metric_name));
                    output.push_str(&format!("{}{} {}\n", metric_name, label_str, v));
                }
                MetricValue::Gauge(v) => {
                    output.push_str(&format!("# TYPE {} gauge\n", metric_name));
                    output.push_str(&format!("{}{} {}\n", metric_name, label_str, v));
                }
                MetricValue::Histogram(values) => {
                    output.push_str(&format!("# TYPE {} histogram\n", metric_name));
                    for v in values {
                        output.push_str(&format!("{}{} {}\n", metric_name, label_str, v));
                    }
                }
                MetricValue::Summary(v) => {
                    output.push_str(&format!("# TYPE {} summary\n", metric_name));
                    output.push_str(&format!("{}{} {}\n", metric_name, label_str, v));
                }
            }
        }

        serde_json::json!({ "format": "prometheus", "data": output })
    }
}

/// JSON Exporter
pub struct JsonExporter;

impl MetricsExporter for JsonExporter {
    fn export(&self, metrics: Vec<Metric>) -> serde_json::Value {
        serde_json::json!({
            "format": "json",
            "timestamp": Utc::now().timestamp(),
            "metrics": metrics.iter().map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "type": format!("{:?}", m.metric_type),
                    "labels": m.labels,
                    "timestamp": m.timestamp,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

/// System Metrics Collector
pub struct SystemMetrics {
    cpu_usage: Gauge,
    memory_usage: Gauge,
    disk_usage: Gauge,
    network_rx: Counter,
    network_tx: Counter,
}

impl SystemMetrics {
    pub fn new() -> Self {
        Self {
            cpu_usage: Gauge::new("system_cpu_usage_percent"),
            memory_usage: Gauge::new("system_memory_usage_percent"),
            disk_usage: Gauge::new("system_disk_usage_percent"),
            network_rx: Counter::new("system_network_bytes_received"),
            network_tx: Counter::new("system_network_bytes_sent"),
        }
    }

    pub fn update(&mut self) {
        // In real implementation, would use sysinfo or similar
        self.cpu_usage.set(rand::random::<f64>() * 100.0);
        self.memory_usage.set(rand::random::<f64>() * 100.0);
        self.disk_usage.set(rand::random::<f64>() * 100.0);
    }
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Alert Manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub alert_id: String,
    pub name: String,
    pub severity: AlertSeverity,
    pub message: String,
    pub fired_at: f64,
    pub resolved_at: Option<f64>,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl Alert {
    pub fn new(name: &str, severity: AlertSeverity, message: &str) -> Self {
        Self {
            alert_id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            severity,
            message: message.to_string(),
            fired_at: Utc::now().timestamp() as f64,
            resolved_at: None,
            labels: HashMap::new(),
        }
    }

    pub fn resolve(&mut self) {
        self.resolved_at = Some(Utc::now().timestamp() as f64);
    }

    pub fn is_resolved(&self) -> bool {
        self.resolved_at.is_some()
    }
}

/// Alert Manager
pub struct AlertManager {
    alerts: Arc<RwLock<HashMap<String, Alert>>>,
    handlers: Vec<Box<dyn AlertHandler>>,
}

impl AlertManager {
    pub fn new() -> Self {
        Self {
            alerts: Arc::new(RwLock::new(HashMap::new())),
            handlers: Vec::new(),
        }
    }

    pub async fn fire(&self, alert: Alert) {
        let alert_id = alert.alert_id.clone();
        let mut alerts = self.alerts.write().await;
        alerts.insert(alert_id.clone(), alert);

        // Notify handlers
        for handler in &self.handlers {
            handler.handle(alerts.get(&alert_id).unwrap().clone()).await;
        }
    }

    pub async fn resolve(&self, alert_id: &str) {
        let mut alerts = self.alerts.write().await;
        if let Some(alert) = alerts.get_mut(alert_id) {
            alert.resolve();
        }
    }

    pub async fn list_active(&self) -> Vec<Alert> {
        let alerts = self.alerts.read().await;
        alerts.values().filter(|a| !a.is_resolved()).cloned().collect()
    }

    pub fn add_handler(&mut self, handler: impl AlertHandler + 'static) {
        self.handlers.push(Box::new(handler));
    }
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Alert Handler Trait
#[async_trait]
pub trait AlertHandler: Send + Sync {
    async fn handle(&self, alert: Alert);
}

/// Console Alert Handler
pub struct ConsoleAlertHandler;

#[async_trait]
impl AlertHandler for ConsoleAlertHandler {
    async fn handle(&self, alert: Alert) {
        let severity_str = format!("{:?}", alert.severity);
        tracing::warn!("[ALERT {}] {}: {}", severity_str, alert.name, alert.message);
    }
}

/// Health Check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub healthy: bool,
    pub checks: Vec<HealthCheck>,
    pub timestamp: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthCheckStatus,
    pub message: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthCheckStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Health Checker
pub struct HealthChecker {
    checks: Vec<Box<dyn Fn() -> HealthCheck + Send + Sync>>,
}

impl HealthChecker {
    pub fn new() -> Self {
        Self {
            checks: Vec::new(),
        }
    }

    pub fn add_check<F>(&mut self, name: &str, check: F)
    where
        F: Fn() -> HealthCheck + Send + Sync + 'static,
    {
        self.checks.push(Box::new(check));
    }

    pub fn check(&self) -> HealthStatus {
        let mut checks_result = Vec::new();
        let mut all_healthy = true;

        for check in &self.checks {
            let result = check();
            if result.status != HealthCheckStatus::Healthy {
                all_healthy = false;
            }
            checks_result.push(result);
        }

        HealthStatus {
            healthy: all_healthy,
            checks: checks_result,
            timestamp: Utc::now().timestamp() as f64,
        }
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter() {
        let mut counter = Counter::new("test_counter");
        counter.inc();
        counter.inc_by(5);
        assert_eq!(counter.get(), 6);
    }

    #[test]
    fn test_gauge() {
        let mut gauge = Gauge::new("test_gauge");
        gauge.set(42.0);
        gauge.inc();
        gauge.dec();
        assert_eq!(gauge.get(), 42.0);
    }

    #[test]
    fn test_histogram() {
        let mut histogram = Histogram::new("test_histogram", vec![0.1, 0.5, 1.0]);
        histogram.observe(0.3);
        histogram.observe(0.7);
        assert_eq!(histogram.get_percentile(50.0), 0.5);
    }

    #[test]
    fn test_timer() {
        let mut timer = Timer::new();
        std::thread::sleep(Duration::from_millis(10));
        let elapsed = timer.stop();
        assert!(elapsed.as_millis() >= 10);
    }

    #[test]
    fn test_alert() {
        let mut alert = Alert::new("Test Alert", AlertSeverity::Warning, "Test message");
        assert!(!alert.is_resolved());
        alert.resolve();
        assert!(alert.is_resolved());
    }

    #[tokio::test]
    async fn test_metrics_registry() {
        let registry = MetricsRegistry::new();
        
        let metric = Metric::new("test", MetricType::Counter);
        registry.register(metric).await;
        
        registry.increment("test", 5).await;
        
        let retrieved = registry.get("test").await;
        assert!(retrieved.is_some());
    }
}