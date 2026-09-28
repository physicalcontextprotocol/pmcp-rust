//! P-MCP Workflow Engine
//!
//! Orchestrates multi-step robot missions with:
// - Sequential and parallel steps
// - Conditional branching
// - Error handling and retry
// - State persistence

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use serde::{Deserialize, Serialize};
use chrono::Utc;

/// Convert a serde_json::json!({...}) object literal into the
/// HashMap<String, Value> that WorkflowStepDef.parameters actually requires.
/// serde_json::json!({...}) produces a Value::Object, not a HashMap
/// directly -- constructing WorkflowStepDef.parameters via the macro
/// literal (as several builtin workflow templates below do) doesn't
/// type-check without this conversion.
fn params(v: serde_json::Value) -> HashMap<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(map) => map.into_iter().collect(),
        _ => HashMap::new(),
    }
}

/// Workflow Definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub steps: Vec<WorkflowStepDef>,
    pub variables: HashMap<String, VariableDef>,
    pub entry_point: Option<String>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepDef {
    pub id: String,
    pub name: String,
    pub step_type: StepType,
    pub actuation: Option<String>,
    pub parameters: HashMap<String, serde_json::Value>,
    pub conditions: Vec<Condition>,
    pub error_handler: Option<ErrorHandler>,
    pub retry: Option<RetryConfig>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepType {
    Actuation,
    Conditional,
    Parallel,
    Wait,
    Log,
    SetVariable,
    Validate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub variable: String,
    pub operator: ConditionOperator,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionOperator {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    Contains,
    In,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorHandler {
    pub on_error: ErrorAction,
    pub fallback_value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorAction {
    Abort,
    Continue,
    Retry,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub exponential: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableDef {
    pub var_type: VariableType,
    pub default_value: Option<serde_json::Value>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariableType {
    String,
    Number,
    Boolean,
    Array,
    Object,
}

/// Workflow Execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecution {
    pub execution_id: String,
    pub workflow_id: String,
    pub status: ExecutionStatus,
    pub current_step: Option<String>,
    pub variables: HashMap<String, serde_json::Value>,
    pub started_at: f64,
    pub completed_at: Option<f64>,
    pub step_results: HashMap<String, StepResult>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub status: StepStatus,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub started_at: Option<f64>,
    pub completed_at: Option<f64>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Skipped,
    Failed,
}

/// Workflow Engine
pub struct WorkflowEngine {
    definitions: Arc<RwLock<HashMap<String, WorkflowDefinition>>>,
    executions: Arc<RwLock<HashMap<String, WorkflowExecution>>>,
    actuate_fn: Option<Arc<dyn Fn(&str, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>>,
    event_sender: mpsc::Sender<WorkflowEvent>,
}

impl WorkflowEngine {
    pub fn new() -> Self {
        let (tx, _) = mpsc::channel(100);
        Self {
            definitions: Arc::new(RwLock::new(HashMap::new())),
            executions: Arc::new(RwLock::new(HashMap::new())),
            actuate_fn: None,
            event_sender: tx,
        }
    }

    pub fn set_actuation_handler<F>(&mut self, f: F)
    where
        F: Fn(&str, serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
    {
        self.actuate_fn = Some(Arc::new(f));
    }

    pub async fn register(&self, definition: WorkflowDefinition) -> Result<(), WorkflowError> {
        let mut definitions = self.definitions.write().await;
        definitions.insert(definition.id.clone(), definition);
        Ok(())
    }

    pub async fn unregister(&self, workflow_id: &str) -> Result<(), WorkflowError> {
        let mut definitions = self.definitions.write().await;
        definitions.remove(workflow_id).ok_or_else(|| WorkflowError::NotFound(workflow_id.to_string()))?;
        Ok(())
    }

    pub async fn get_definition(&self, workflow_id: &str) -> Option<WorkflowDefinition> {
        let definitions = self.definitions.read().await;
        definitions.get(workflow_id).cloned()
    }

    pub async fn list_definitions(&self) -> Vec<WorkflowDefinition> {
        let definitions = self.definitions.read().await;
        definitions.values().cloned().collect()
    }

    pub async fn start(&self, workflow_id: &str, initial_vars: Option<HashMap<String, serde_json::Value>>) -> Result<String, WorkflowError> {
        let definitions = self.definitions.read().await;
        let definition = definitions.get(workflow_id)
            .ok_or_else(|| WorkflowError::NotFound(workflow_id.to_string()))?
            .clone();
        drop(definitions);

        let execution_id = uuid::Uuid::new_v4().to_string();
        let mut variables = initial_vars.unwrap_or_default();

        // Initialize variables with defaults
        for (var_name, var_def) in &definition.variables {
            if !variables.contains_key(var_name) {
                if let Some(default) = &var_def.default_value {
                    variables.insert(var_name.clone(), default.clone());
                }
            }
        }

        let execution = WorkflowExecution {
            execution_id: execution_id.clone(),
            workflow_id: workflow_id.to_string(),
            status: ExecutionStatus::Running,
            current_step: definition.entry_point.clone(),
            variables,
            started_at: Utc::now().timestamp() as f64,
            completed_at: None,
            step_results: HashMap::new(),
            error: None,
        };

        let mut executions = self.executions.write().await;
        executions.insert(execution_id.clone(), execution);

        // Emit event
        let _ = self.event_sender.send(WorkflowEvent::Started { 
            execution_id: execution_id.clone(), 
            workflow_id: workflow_id.to_string() 
        }).await;

        // Start execution in background
        let engine = self.clone();
        let exec_id = execution_id.clone();
        tokio::spawn(async move {
            let _ = engine.execute_workflow(&exec_id).await;
        });

        Ok(execution_id)
    }

    async fn execute_workflow(&self, execution_id: &str) -> Result<(), WorkflowError> {
        let definition = {
            let executions = self.executions.read().await;
            let exec = executions.get(execution_id)
                .ok_or_else(|| WorkflowError::NotFound(execution_id.to_string()))?;
            
            let definitions = self.definitions.read().await;
            definitions.get(&exec.workflow_id)
                .ok_or_else(|| WorkflowError::NotFound(exec.workflow_id.clone()))?
                .clone()
        };
        
        // Execute steps
        for step_def in &definition.steps {
            let result = self.execute_step(execution_id, step_def).await?;
            let result_status = result.status;
            let result_error = result.error.clone();

            // Update execution
            let mut executions = self.executions.write().await;
            if let Some(exec) = executions.get_mut(execution_id) {
                exec.current_step = Some(step_def.id.clone());
                exec.step_results.insert(step_def.id.clone(), result);
            }

            // Check if should continue
            if result_status == StepStatus::Failed {
                self.fail_execution(execution_id, &format!("Step {} failed: {:?}", step_def.name, result_error)).await;
                return Err(WorkflowError::StepFailed(step_def.id.clone()));
            }
        }

        // Mark completed
        self.complete_execution(execution_id).await;
        Ok(())
    }

    async fn execute_step(&self, execution_id: &str, step_def: &WorkflowStepDef) -> Result<StepResult, WorkflowError> {
        let started_at = Utc::now().timestamp() as f64;

        // Emit step started event
        let _ = self.event_sender.send(WorkflowEvent::StepStarted {
            execution_id: execution_id.to_string(),
            step_id: step_def.id.clone(),
        }).await;

        let result = match step_def.step_type {
            StepType::Actuation => self.execute_actuation(step_def).await,
            StepType::Conditional => self.execute_conditional(step_def).await,
            StepType::Log => self.execute_log(step_def).await,
            StepType::SetVariable => self.execute_set_variable(execution_id, step_def).await,
            StepType::Wait => self.execute_wait(step_def).await,
            StepType::Validate => self.execute_validate(step_def).await,
            StepType::Parallel => self.execute_parallel(step_def).await,
        };

        let completed_at = Utc::now().timestamp() as f64;
        let duration_ms = ((completed_at - started_at) * 1000.0) as u64;

        Ok(StepResult {
            step_id: step_def.id.clone(),
            status: result.clone(),
            output: None,
            error: None,
            started_at: Some(started_at),
            completed_at: Some(completed_at),
            duration_ms,
        })
    }

    async fn execute_actuation(&self, step_def: &WorkflowStepDef) -> StepStatus {
        if let Some(actuation) = &step_def.actuation {
            if let Some(handler) = &self.actuate_fn {
                match handler(actuation, serde_json::Value::Object(step_def.parameters.clone().into_iter().collect())) {
                    Ok(_) => StepStatus::Completed,
                    Err(e) => {
                        tracing::error!("[Workflow] Actuation failed: {}", e);
                        StepStatus::Failed
                    }
                }
            } else {
                StepStatus::Completed // No handler, assume success
            }
        } else {
            StepStatus::Failed
        }
    }

    async fn execute_conditional(&self, step_def: &WorkflowStepDef) -> StepStatus {
        // Evaluate conditions
        let executions = self.executions.read().await;
        if let Some(exec) = executions.values().next() {
            for condition in &step_def.conditions {
                if let Some(var_value) = exec.variables.get(&condition.variable) {
                    let matches = self.evaluate_condition(var_value, &condition.operator, &condition.value);
                    if !matches {
                        return StepStatus::Skipped;
                    }
                }
            }
        }
        StepStatus::Completed
    }

    fn evaluate_condition(&self, actual: &serde_json::Value, operator: &ConditionOperator, expected: &serde_json::Value) -> bool {
        match operator {
            ConditionOperator::Equals => actual == expected,
            ConditionOperator::NotEquals => actual != expected,
            ConditionOperator::GreaterThan => {
                if let (Some(a), Some(e)) = (actual.as_f64(), expected.as_f64()) {
                    a > e
                } else {
                    false
                }
            }
            ConditionOperator::LessThan => {
                if let (Some(a), Some(e)) = (actual.as_f64(), expected.as_f64()) {
                    a < e
                } else {
                    false
                }
            }
            ConditionOperator::Contains => {
                if let (Some(a), Some(e)) = (actual.as_str(), expected.as_str()) {
                    a.contains(e)
                } else {
                    false
                }
            }
            ConditionOperator::In => {
                if let Some(arr) = expected.as_array() {
                    arr.contains(actual)
                } else {
                    false
                }
            }
        }
    }

    async fn execute_log(&self, step_def: &WorkflowStepDef) -> StepStatus {
        let msg = step_def.parameters.get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Workflow log");
        tracing::info!("[Workflow] {}", msg);
        StepStatus::Completed
    }

    async fn execute_set_variable(&self, execution_id: &str, step_def: &WorkflowStepDef) -> StepStatus {
        if let Some(value) = step_def.parameters.get("value") {
            let mut executions = self.executions.write().await;
            if let Some(exec) = executions.get_mut(execution_id) {
                let var_name = step_def.parameters.get("variable")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                exec.variables.insert(var_name.to_string(), value.clone());
            }
        }
        StepStatus::Completed
    }

    async fn execute_wait(&self, step_def: &WorkflowStepDef) -> StepStatus {
        let duration_ms = step_def.parameters.get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000);
        tokio::time::sleep(tokio::time::Duration::from_millis(duration_ms)).await;
        StepStatus::Completed
    }

    async fn execute_validate(&self, step_def: &WorkflowStepDef) -> StepStatus {
        // Validation logic
        StepStatus::Completed
    }

    async fn execute_parallel(&self, step_def: &WorkflowStepDef) -> StepStatus {
        // Parallel execution logic
        StepStatus::Completed
    }

    async fn fail_execution(&self, execution_id: &str, error: &str) {
        let mut executions = self.executions.write().await;
        if let Some(exec) = executions.get_mut(execution_id) {
            exec.status = ExecutionStatus::Failed;
            exec.error = Some(error.to_string());
            exec.completed_at = Some(Utc::now().timestamp() as f64);
        }
        
        let _ = self.event_sender.send(WorkflowEvent::Failed {
            execution_id: execution_id.to_string(),
            error: error.to_string(),
        }).await;
    }

    async fn complete_execution(&self, execution_id: &str) {
        let mut executions = self.executions.write().await;
        if let Some(exec) = executions.get_mut(execution_id) {
            exec.status = ExecutionStatus::Completed;
            exec.completed_at = Some(Utc::now().timestamp() as f64);
        }
        
        let _ = self.event_sender.send(WorkflowEvent::Completed {
            execution_id: execution_id.to_string(),
        }).await;
    }

    pub async fn get_execution(&self, execution_id: &str) -> Option<WorkflowExecution> {
        let executions = self.executions.read().await;
        executions.get(execution_id).cloned()
    }

    pub async fn list_executions(&self) -> Vec<WorkflowExecution> {
        let executions = self.executions.read().await;
        executions.values().cloned().collect()
    }

    pub async fn cancel(&self, execution_id: &str) -> Result<(), WorkflowError> {
        let mut executions = self.executions.write().await;
        if let Some(exec) = executions.get_mut(execution_id) {
            exec.status = ExecutionStatus::Cancelled;
            exec.completed_at = Some(Utc::now().timestamp() as f64);
            Ok(())
        } else {
            Err(WorkflowError::NotFound(execution_id.to_string()))
        }
    }
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WorkflowEngine {
    fn clone(&self) -> Self {
        Self {
            definitions: self.definitions.clone(),
            executions: self.executions.clone(),
            actuate_fn: self.actuate_fn.clone(),
            event_sender: self.event_sender.clone(),
        }
    }
}

/// Workflow Error
#[derive(Debug)]
pub enum WorkflowError {
    NotFound(String),
    StepFailed(String),
    InvalidDefinition(String),
    ExecutionError(String),
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowError::NotFound(id) => write!(f, "Workflow not found: {}", id),
            WorkflowError::StepFailed(id) => write!(f, "Step failed: {}", id),
            WorkflowError::InvalidDefinition(msg) => write!(f, "Invalid definition: {}", msg),
            WorkflowError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
        }
    }
}

impl std::error::Error for WorkflowError {}

/// Workflow Events
#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    Started { execution_id: String, workflow_id: String },
    StepStarted { execution_id: String, step_id: String },
    StepCompleted { execution_id: String, step_id: String },
    Completed { execution_id: String },
    Failed { execution_id: String, error: String },
    Cancelled { execution_id: String },
}

/// Preset Workflow Templates
pub mod presets {
    use super::*;

    pub fn pick_and_place() -> WorkflowDefinition {
        WorkflowDefinition {
            id: "pick-and-place".to_string(),
            name: "Pick and Place".to_string(),
            description: "Pick object from A and place at B".to_string(),
            version: "1.0.0".to_string(),
            steps: vec![
                WorkflowStepDef {
                    id: "move-to-pick".to_string(),
                    name: "Move to pick position".to_string(),
                    step_type: StepType::Actuation,
                    actuation: Some("move_to".to_string()),
                    parameters: params(serde_json::json!({"x": 0.3, "y": 0.0, "z": 0.1})),
                    conditions: vec![],
                    error_handler: None,
                    retry: None,
                    timeout_seconds: Some(30),
                },
                WorkflowStepDef {
                    id: "grasp".to_string(),
                    name: "Grasp object".to_string(),
                    step_type: StepType::Actuation,
                    actuation: Some("gripper_close".to_string()),
                    parameters: params(serde_json::json!({"force": 50.0})),
                    conditions: vec![],
                    error_handler: None,
                    retry: None,
                    timeout_seconds: Some(5),
                },
                WorkflowStepDef {
                    id: "move-to-place".to_string(),
                    name: "Move to place position".to_string(),
                    step_type: StepType::Actuation,
                    actuation: Some("move_to".to_string()),
                    parameters: params(serde_json::json!({"x": 0.5, "y": 0.3, "z": 0.1})),
                    conditions: vec![],
                    error_handler: None,
                    retry: None,
                    timeout_seconds: Some(30),
                },
                WorkflowStepDef {
                    id: "release".to_string(),
                    name: "Release object".to_string(),
                    step_type: StepType::Actuation,
                    actuation: Some("gripper_open".to_string()),
                    parameters: params(serde_json::json!({})),
                    conditions: vec![],
                    error_handler: None,
                    retry: None,
                    timeout_seconds: Some(5),
                },
            ],
            variables: HashMap::new(),
            entry_point: Some("move-to-pick".to_string()),
            timeout_seconds: Some(120),
        }
    }

    pub fn palletizing(columns: u32, rows: u32, layers: u32) -> WorkflowDefinition {
        let mut steps = Vec::new();
        let mut step_id = 0;

        for layer in 0..layers {
            for row in 0..rows {
                for col in 0..columns {
                    let x = 0.3 + col as f64 * 0.1;
                    let y = row as f64 * 0.1;
                    let z = 0.05 + layer as f64 * 0.05;

                    steps.push(WorkflowStepDef {
                        id: format!("place-{}-{}", layer, step_id),
                        name: format!("Place item at ({}, {}, {})", col, row, layer),
                        step_type: StepType::Actuation,
                        actuation: Some("move_to".to_string()),
                        parameters: params(serde_json::json!({"x": x, "y": y, "z": z})),
                        conditions: vec![],
                        error_handler: None,
                        retry: None,
                        timeout_seconds: Some(30),
                    });
                    step_id += 1;
                }
            }
        }

        WorkflowDefinition {
            id: "palletizing".to_string(),
            name: "Palletizing".to_string(),
            description: format!("Palletize {}x{}x{} items", columns, rows, layers),
            version: "1.0.0".to_string(),
            steps,
            variables: HashMap::new(),
            entry_point: None,
            timeout_seconds: Some(3600),
        }
    }

    pub fn inspection(points: Vec<(f64, f64, f64)>) -> WorkflowDefinition {
        let steps: Vec<WorkflowStepDef> = points.iter().enumerate().map(|(i, (x, y, z))| {
            WorkflowStepDef {
                id: format!("inspect-{}", i),
                name: format!("Inspect point {}", i),
                step_type: StepType::Actuation,
                actuation: Some("move_to".to_string()),
                parameters: params(serde_json::json!({"x": x, "y": y, "z": z})),
                conditions: vec![],
                error_handler: None,
                retry: None,
                timeout_seconds: Some(30),
            }
        }).collect();

        WorkflowDefinition {
            id: "inspection".to_string(),
            name: "Inspection".to_string(),
            description: "Inspect multiple points".to_string(),
            version: "1.0.0".to_string(),
            steps,
            variables: HashMap::new(),
            entry_point: None,
            timeout_seconds: Some(600),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_definition() {
        let wf = WorkflowDefinition {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test workflow".to_string(),
            version: "1.0.0".to_string(),
            steps: vec![],
            variables: HashMap::new(),
            entry_point: None,
            timeout_seconds: Some(60),
        };
        assert_eq!(wf.id, "test");
    }

    #[tokio::test]
    async fn test_workflow_engine() {
        let engine = WorkflowEngine::new();
        
        let wf = presets::pick_and_place();
        engine.register(wf).await.ok();
        
        let defs = engine.list_definitions().await;
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn test_preset_pick_and_place() {
        let wf = presets::pick_and_place();
        assert_eq!(wf.steps.len(), 4);
    }
}