//! P-MCP Plugin System
//!
//! Extensible plugin architecture for:
// - Hardware drivers
// - Safety validators
// - Sensor integrations
// - Custom actuations
// - Protocol bridges

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use chrono::Utc;
use async_trait::async_trait;

/// Plugin Trait
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn plugin_type(&self) -> PluginType;
    fn initialize(&self) -> Result<(), PluginError>;
    fn shutdown(&self) -> Result<(), PluginError>;
}

/// Plugin Types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginType {
    Driver,
    Safety,
    Sensor,
    Actuation,
    Bridge,
    Custom,
}

/// Plugin Status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginStatus {
    Loaded,
    Initialized,
    Running,
    Stopped,
    Error,
}

/// Plugin Error
#[derive(Debug)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PluginError {}

impl PluginError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
        }
    }

    pub fn not_found(plugin: &str) -> Self {
        Self::new("NOT_FOUND", &format!("Plugin not found: {}", plugin))
    }

    pub fn initialization_failed(plugin: &str, reason: &str) -> Self {
        Self::new("INIT_FAILED", &format!("Plugin {} initialization failed: {}", plugin, reason))
    }
}

/// Plugin Descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDescriptor {
    pub name: String,
    pub version: String,
    pub plugin_type: PluginType,
    pub description: String,
    pub author: String,
    pub dependencies: Vec<String>,
    pub config_schema: Option<serde_json::Value>,
}

impl PluginDescriptor {
    pub fn new(name: &str, version: &str, plugin_type: PluginType) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            plugin_type,
            description: String::new(),
            author: String::new(),
            dependencies: Vec::new(),
            config_schema: None,
        }
    }
}

/// Plugin Instance
pub struct PluginInstance {
    pub descriptor: PluginDescriptor,
    pub status: PluginStatus,
    pub loaded_at: f64,
    pub plugin: Arc<dyn Plugin>,
}

impl std::fmt::Debug for PluginInstance {
    // dyn Plugin doesn't implement Debug -- hand-rolled, same rationale as
    // Route's Debug impl in api.rs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginInstance")
            .field("descriptor", &self.descriptor)
            .field("status", &self.status)
            .field("loaded_at", &self.loaded_at)
            .field("plugin", &"<dyn Plugin>")
            .finish()
    }
}

impl PluginInstance {
    pub fn new(descriptor: PluginDescriptor, plugin: Arc<dyn Plugin>) -> Self {
        Self {
            descriptor,
            status: PluginStatus::Loaded,
            loaded_at: Utc::now().timestamp() as f64,
            plugin,
        }
    }
}

/// Plugin Manager
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, PluginInstance>>>,
    hooks: Arc<RwLock<Vec<PluginHook>>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            hooks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn load(&self, mut instance: PluginInstance) -> Result<(), PluginError> {
        let name = instance.descriptor.name.clone();
        
        // Initialize plugin
        if let Err(e) = instance.plugin.initialize() {
            return Err(PluginError::initialization_failed(&name, &e.to_string()));
        }

        instance.status = PluginStatus::Initialized;

        // Register
        let mut plugins = self.plugins.write().await;
        plugins.insert(name.clone(), instance);

        // Call load hooks
        self.trigger_hook(PluginHook::PluginLoaded { name: name.clone() }).await;

        tracing::info!("[PluginManager] Loaded plugin: {}", name);
        Ok(())
    }

    pub async fn unload(&self, name: &str) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write().await;
        
        if let Some(instance) = plugins.remove(name) {
            instance.plugin.shutdown()?;
            self.trigger_hook(PluginHook::PluginUnloaded { name: name.to_string() }).await;
            tracing::info!("[PluginManager] Unloaded plugin: {}", name);
            Ok(())
        } else {
            Err(PluginError::not_found(name))
        }
    }

    pub async fn get(&self, name: &str) -> Option<Arc<dyn Plugin>> {
        let plugins = self.plugins.read().await;
        plugins.get(name).map(|i| i.plugin.clone())
    }

    pub async fn list(&self) -> Vec<PluginDescriptor> {
        let plugins = self.plugins.read().await;
        plugins.values().map(|i| i.descriptor.clone()).collect()
    }

    pub async fn list_by_type(&self, plugin_type: PluginType) -> Vec<PluginDescriptor> {
        let plugins = self.plugins.read().await;
        plugins.values()
            .filter(|i| i.descriptor.plugin_type == plugin_type)
            .map(|i| i.descriptor.clone())
            .collect()
    }

    pub async fn call_hook(&self, hook: PluginHook) {
        self.trigger_hook(hook).await;
    }

    /// Record a hook event.
    ///
    /// NOTE: this only appends to the event log (self.hooks: Vec<PluginHook>).
    /// PluginHookHandler is a declared trait with NO registry field and NO
    /// implementations anywhere in this codebase (confirmed) -- handler
    /// dispatch (notifying subscribers when a hook fires) was never wired
    /// up. The previous code here (`for h in hooks.iter() { h.execute(...) }`)
    /// didn't compile: it iterated over past PluginHook *events* and called
    /// .execute() on them, which is not what PluginHookHandler is for, and
    /// wouldn't have worked even if it compiled (PluginHook doesn't
    /// implement PluginHookHandler). Left as an event log until a real
    /// handler registry is added.
    async fn trigger_hook(&self, hook: PluginHook) {
        let mut hooks = self.hooks.write().await;
        hooks.push(hook);
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Plugin Hook
#[derive(Debug, Clone)]
pub enum PluginHook {
    PluginLoaded { name: String },
    PluginUnloaded { name: String },
    PreActuation { name: String, args: serde_json::Value },
    PostActuation { name: String, result: serde_json::Value },
    PreValidation { name: String, args: serde_json::Value },
    PostValidation { result: bool },
    SensorUpdate { name: String, value: serde_json::Value },
    LeaseRequested { robot_id: String, zone_id: String },
    LeaseGranted { lease_id: String },
}

#[async_trait]
pub trait PluginHookHandler: Send + Sync {
    async fn execute(&self, hook: &PluginHook) -> Result<(), PluginError>;
}

/// Plugin Hook Wrapper
struct PluginHookWrapper {
    name: String,
    handler: Arc<dyn PluginHookHandler>,
}

impl PluginHookWrapper {
    async fn execute(&self, hook: &PluginHook) -> Result<(), PluginError> {
        self.handler.execute(hook).await
    }
}

/// Plugin Registry (for discovery)
pub struct PluginRegistry {
    descriptors: Arc<RwLock<HashMap<String, PluginDescriptor>>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            descriptors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, descriptor: PluginDescriptor) {
        let mut descriptors = self.descriptors.write().await;
        descriptors.insert(descriptor.name.clone(), descriptor);
    }

    pub async fn unregister(&self, name: &str) {
        let mut descriptors = self.descriptors.write().await;
        descriptors.remove(name);
    }

    pub async fn get(&self, name: &str) -> Option<PluginDescriptor> {
        let descriptors = self.descriptors.read().await;
        descriptors.get(name).cloned()
    }

    pub async fn list(&self) -> Vec<PluginDescriptor> {
        let descriptors = self.descriptors.read().await;
        descriptors.values().cloned().collect()
    }

    pub async fn search(&self, query: &str) -> Vec<PluginDescriptor> {
        let descriptors = self.descriptors.read().await;
        let query_lower = query.to_lowercase();
        
        descriptors.values()
            .filter(|d| {
                d.name.to_lowercase().contains(&query_lower) ||
                d.description.to_lowercase().contains(&query_lower)
            })
            .cloned()
            .collect()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in Hardware Driver Plugin
pub struct UR5Driver {
    config: UR5Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UR5Config {
    pub ip_address: String,
    pub port: u16,
    pub workspace_limits: WorkspaceLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceLimits {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    pub z_min: f64,
    pub z_max: f64,
}

impl UR5Driver {
    pub fn new(config: UR5Config) -> Self {
        Self { config }
    }

    pub async fn connect(&self) -> Result<(), PluginError> {
        // In real implementation, would connect to UR5
        tracing::info!("[UR5Driver] Connecting to {}:{}", self.config.ip_address, self.config.port);
        Ok(())
    }

    pub async fn move_joint(&self, positions: &[f64]) -> Result<(), PluginError> {
        // In real implementation, would send move command
        tracing::debug!("[UR5Driver] Moving to joint positions: {:?}", positions);
        Ok(())
    }

    pub async fn move_pose(&self, pose: &[f64]) -> Result<(), PluginError> {
        tracing::debug!("[UR5Driver] Moving to pose: {:?}", pose);
        Ok(())
    }

    pub async fn get_state(&self) -> Result<serde_json::Value, PluginError> {
        Ok(serde_json::json!({
            "joint_positions": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "tool_position": [0.0, 0.0, 0.0],
            "connected": true,
        }))
    }
}

impl Plugin for UR5Driver {
    fn name(&self) -> &str { "ur5-driver" }
    fn version(&self) -> &str { "1.0.0" }
    fn plugin_type(&self) -> PluginType { PluginType::Driver }

    fn initialize(&self) -> Result<(), PluginError> {
        tracing::info!("[UR5Driver] Initializing");
        Ok(())
    }

    fn shutdown(&self) -> Result<(), PluginError> {
        tracing::info!("[UR5Driver] Shutting down");
        Ok(())
    }
}

/// OPC-UA Bridge Plugin
pub struct OpcuBridge {
    config: OpcuConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpcuConfig {
    pub endpoint: String,
    pub security_policy: String,
    pub auth_mode: String,
}

impl OpcuBridge {
    pub fn new(config: OpcuConfig) -> Self {
        Self { config }
    }

    pub async fn connect(&self) -> Result<(), PluginError> {
        tracing::info!("[OpcuBridge] Connecting to {}", self.config.endpoint);
        Ok(())
    }

    pub async fn read_node(&self, node_id: &str) -> Result<serde_json::Value, PluginError> {
        Ok(serde_json::json!({ "nodeId": node_id, "value": 0.0 }))
    }

    pub async fn write_node(&self, node_id: &str, value: serde_json::Value) -> Result<(), PluginError> {
        tracing::debug!("[OpcuBridge] Writing {} = {:?}", node_id, value);
        Ok(())
    }
}

impl Plugin for OpcuBridge {
    fn name(&self) -> &str { "opcua-bridge" }
    fn version(&self) -> &str { "1.0.0" }
    fn plugin_type(&self) -> PluginType { PluginType::Bridge }

    fn initialize(&self) -> Result<(), PluginError> { Ok(()) }
    fn shutdown(&self) -> Result<(), PluginError> { Ok(()) }
}

/// ROS2 Bridge Plugin
pub struct Ros2Bridge {
    config: Ros2Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ros2Config {
    pub domain_id: u8,
    pub node_name: String,
    pub namespace: String,
}

impl Ros2Bridge {
    pub fn new(config: Ros2Config) -> Self {
        Self { config }
    }

    pub async fn publish(&self, topic: &str, msg: serde_json::Value) -> Result<(), PluginError> {
        tracing::debug!("[Ros2Bridge] Publishing to {}: {:?}", topic, msg);
        Ok(())
    }

    pub async fn subscribe(&self, topic: &str) -> Result<(), PluginError> {
        tracing::info!("[Ros2Bridge] Subscribing to {}", topic);
        Ok(())
    }

    pub async fn call_service(&self, service: &str, request: serde_json::Value) -> Result<serde_json::Value, PluginError> {
        Ok(serde_json::json!({ "service": service, "response": {} }))
    }
}

impl Plugin for Ros2Bridge {
    fn name(&self) -> &str { "ros2-bridge" }
    fn version(&self) -> &str { "1.0.0" }
    fn plugin_type(&self) -> PluginType { PluginType::Bridge }

    fn initialize(&self) -> Result<(), PluginError> { Ok(()) }
    fn shutdown(&self) -> Result<(), PluginError> { Ok(()) }
}

/// Custom Actuation Plugin
pub struct CustomActuation {
    name: String,
    handler: Arc<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>,
}

impl CustomActuation {
    pub fn new(
        name: &str,
        handler: impl Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.to_string(),
            handler: Arc::new(handler),
        }
    }

    pub fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, String> {
        (self.handler)(args)
    }
}

impl Plugin for CustomActuation {
    fn name(&self) -> &str { &self.name }
    fn version(&self) -> &str { "1.0.0" }
    fn plugin_type(&self) -> PluginType { PluginType::Actuation }

    fn initialize(&self) -> Result<(), PluginError> { Ok(()) }
    fn shutdown(&self) -> Result<(), PluginError> { Ok(()) }
}

/// Plugin Loader
pub struct PluginLoader {
    registry: PluginRegistry,
}

impl PluginLoader {
    pub fn new() -> Self {
        Self {
            registry: PluginRegistry::new(),
        }
    }

    pub async fn load_from_file(&self, path: &str) -> Result<PluginInstance, PluginError> {
        // In real implementation, would dynamically load from file
        // For now, return a placeholder
        let descriptor = PluginDescriptor::new("dynamic-plugin", "1.0.0", PluginType::Custom);
        
        // Create a simple no-op plugin
        struct DummyPlugin;
        impl Plugin for DummyPlugin {
            fn name(&self) -> &str { "dummy" }
            fn version(&self) -> &str { "1.0.0" }
            fn plugin_type(&self) -> PluginType { PluginType::Custom }
            fn initialize(&self) -> Result<(), PluginError> { Ok(()) }
            fn shutdown(&self) -> Result<(), PluginError> { Ok(()) }
        }

        Ok(PluginInstance::new(descriptor, Arc::new(DummyPlugin)))
    }

    pub async fn load_from_config(&self, config: &serde_json::Value) -> Result<PluginInstance, PluginError> {
        let name = config.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let version = config.get("version").and_then(|v| v.as_str()).unwrap_or("1.0.0");
        let plugin_type = match config.get("type").and_then(|v| v.as_str()) {
            Some("driver") => PluginType::Driver,
            Some("safety") => PluginType::Safety,
            Some("sensor") => PluginType::Sensor,
            Some("actuation") => PluginType::Actuation,
            Some("bridge") => PluginType::Bridge,
            _ => PluginType::Custom,
        };

        let descriptor = PluginDescriptor {
            name: name.to_string(),
            version: version.to_string(),
            plugin_type,
            description: config.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            author: config.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            dependencies: config.get("dependencies")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            config_schema: config.get("config_schema").cloned(),
        };

        struct ConfigPlugin {
            name: String,
            version: String,
            plugin_type: PluginType,
        }
        impl Plugin for ConfigPlugin {
            fn name(&self) -> &str { &self.name }
            fn version(&self) -> &str { &self.version }
            fn plugin_type(&self) -> PluginType { self.plugin_type }
            fn initialize(&self) -> Result<(), PluginError> { Ok(()) }
            fn shutdown(&self) -> Result<(), PluginError> { Ok(()) }
        }
        let config_plugin = ConfigPlugin {
            name: descriptor.name.clone(),
            version: descriptor.version.clone(),
            plugin_type: descriptor.plugin_type,
        };

        Ok(PluginInstance::new(descriptor, Arc::new(config_plugin)))
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_descriptor() {
        let desc = PluginDescriptor::new("test-plugin", "1.0.0", PluginType::Custom);
        assert_eq!(desc.name, "test-plugin");
        assert_eq!(desc.version, "1.0.0");
    }

    #[tokio::test]
    async fn test_plugin_manager() {
        let manager = PluginManager::new();
        
        let desc = PluginDescriptor::new("test", "1.0.0", PluginType::Custom);
        struct TestPlugin;
        impl Plugin for TestPlugin {
            fn name(&self) -> &str { "test" }
            fn version(&self) -> &str { "1.0.0" }
            fn plugin_type(&self) -> PluginType { PluginType::Custom }
            fn initialize(&self) -> Result<(), PluginError> { Ok(()) }
            fn shutdown(&self) -> Result<(), PluginError> { Ok(()) }
        }
        
        let instance = PluginInstance::new(desc, Arc::new(TestPlugin));
        manager.load(instance).await.ok();
        
        let plugins = manager.list().await;
        assert_eq!(plugins.len(), 1);
    }

    #[tokio::test]
    async fn test_plugin_registry() {
        let registry = PluginRegistry::new();
        
        let desc = PluginDescriptor::new("test-plugin", "1.0.0", PluginType::Driver);
        registry.register(desc).await;
        
        let found = registry.get("test-plugin").await;
        assert!(found.is_some());
    }
}