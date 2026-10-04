//! PCP State Management
//!
//! Manages server state including:
//! - Robot state (position, velocity, joint angles)
//! - Session state
//! - Workflow state
//! - Configuration state

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use serde::{Deserialize, Serialize};
use chrono::Utc;

/// Robot State
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RobotState {
    pub robot_id: String,
    pub robot_class: String,
    pub position: Position,
    pub velocity: Velocity,
    pub joint_angles: Vec<f64>,
    pub joint_velocities: Vec<f64>,
    pub joint_torques: Vec<f64>,
    pub gripper_state: GripperState,
    pub timestamp: f64,
    pub status: RobotStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub roll: f64,
    pub pitch: f64,
    pub yaw: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Velocity {
    pub linear: f64,
    pub angular: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GripperState {
    pub position: f64,
    pub force: f64,
    pub is_grasping: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RobotStatus {
    Idle,
    Moving,
    Grasping,
    Releasing,
    Error,
    Estopped,
    Calibration,
}

impl Default for RobotState {
    fn default() -> Self {
        Self {
            robot_id: String::new(),
            robot_class: "arm".to_string(),
            position: Position::default(),
            velocity: Velocity::default(),
            joint_angles: vec![0.0; 6],
            joint_velocities: vec![0.0; 6],
            joint_torques: vec![0.0; 6],
            gripper_state: GripperState::default(),
            timestamp: Utc::now().timestamp() as f64,
            status: RobotStatus::Idle,
        }
    }
}

impl Default for Position {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            roll: 0.0,
            pitch: 0.0,
            yaw: 0.0,
        }
    }
}

impl Default for Velocity {
    fn default() -> Self {
        Self {
            linear: 0.0,
            angular: 0.0,
        }
    }
}

impl Default for GripperState {
    fn default() -> Self {
        Self {
            position: 0.0,
            force: 0.0,
            is_grasping: false,
        }
    }
}

impl RobotState {
    pub fn new(robot_id: &str, robot_class: &str, num_joints: usize) -> Self {
        Self {
            robot_id: robot_id.to_string(),
            robot_class: robot_class.to_string(),
            joint_angles: vec![0.0; num_joints],
            joint_velocities: vec![0.0; num_joints],
            joint_torques: vec![0.0; num_joints],
            ..Default::default()
        }
    }

    pub fn update_position(&mut self, x: f64, y: f64, z: f64) {
        self.position.x = x;
        self.position.y = y;
        self.position.z = z;
        self.timestamp = Utc::now().timestamp() as f64;
    }

    pub fn update_joints(&mut self, angles: &[f64]) {
        self.joint_angles = angles.to_vec();
        self.timestamp = Utc::now().timestamp() as f64;
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "robotId": self.robot_id,
            "position": {
                "x": self.position.x,
                "y": self.position.y,
                "z": self.position.z,
                "roll": self.position.roll,
                "pitch": self.position.pitch,
                "yaw": self.position.yaw,
            },
            "jointAngles": self.joint_angles,
            "status": format!("{:?}", self.status),
            "timestamp": self.timestamp,
        })
    }
}

/// Session State
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub client_id: String,
    pub connected_at: f64,
    pub last_activity: f64,
    pub capabilities: Vec<String>,
    pub initialized: bool,
    pub metadata: HashMap<String, String>,
}

impl SessionState {
    pub fn new(session_id: &str, client_id: &str) -> Self {
        let now = Utc::now().timestamp() as f64;
        Self {
            session_id: session_id.to_string(),
            client_id: client_id.to_string(),
            connected_at: now,
            last_activity: now,
            capabilities: vec![],
            initialized: false,
            metadata: HashMap::new(),
        }
    }

    pub fn mark_activity(&mut self) {
        self.last_activity = Utc::now().timestamp() as f64;
    }

    pub fn is_active(&self, timeout_secs: f64) -> bool {
        let now = Utc::now().timestamp() as f64;
        now - self.last_activity < timeout_secs
    }

    pub fn set_initialized(&mut self) {
        self.initialized = true;
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "sessionId": self.session_id,
            "clientId": self.client_id,
            "connectedAt": self.connected_at,
            "lastActivity": self.last_activity,
            "initialized": self.initialized,
            "metadata": self.metadata,
        })
    }
}

/// Workflow State
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    pub workflow_id: String,
    pub name: String,
    pub status: WorkflowStatus,
    pub current_step: usize,
    pub total_steps: usize,
    pub started_at: f64,
    pub completed_at: Option<f64>,
    pub steps: Vec<WorkflowStep>,
    pub variables: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub step_id: usize,
    pub name: String,
    pub actuation: String,
    pub arguments: HashMap<String, serde_json::Value>,
    pub status: StepStatus,
    pub started_at: Option<f64>,
    pub completed_at: Option<f64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl WorkflowState {
    pub fn new(workflow_id: &str, name: &str, steps: Vec<WorkflowStep>) -> Self {
        let total_steps = steps.len();
        Self {
            workflow_id: workflow_id.to_string(),
            name: name.to_string(),
            status: WorkflowStatus::Pending,
            current_step: 0,
            total_steps,
            started_at: Utc::now().timestamp() as f64,
            completed_at: None,
            steps,
            variables: HashMap::new(),
        }
    }

    pub fn start(&mut self) {
        self.status = WorkflowStatus::Running;
        if !self.steps.is_empty() {
            self.steps[0].status = StepStatus::Running;
            self.steps[0].started_at = Some(Utc::now().timestamp() as f64);
        }
    }

    pub fn next_step(&mut self, result: serde_json::Value) {
        // Mark current step as completed
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::Completed;
            step.completed_at = Some(Utc::now().timestamp() as f64);
            step.result = Some(result);
        }

        // Move to next step
        self.current_step += 1;
        
        if self.current_step < self.steps.len() {
            // Start next step
            if let Some(step) = self.steps.get_mut(self.current_step) {
                step.status = StepStatus::Running;
                step.started_at = Some(Utc::now().timestamp() as f64);
            }
        } else {
            // All steps completed
            self.status = WorkflowStatus::Completed;
            self.completed_at = Some(Utc::now().timestamp() as f64);
        }
    }

    pub fn fail(&mut self, error: &str) {
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::Failed;
            step.error = Some(error.to_string());
        }
        self.status = WorkflowStatus::Failed;
    }

    pub fn cancel(&mut self) {
        self.status = WorkflowStatus::Cancelled;
        self.completed_at = Some(Utc::now().timestamp() as f64);
    }

    pub fn progress(&self) -> f64 {
        if self.total_steps == 0 {
            0.0
        } else {
            self.current_step as f64 / self.total_steps as f64
        }
    }
}

/// Configuration State
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigState {
    pub key: String,
    pub value: serde_json::Value,
    pub config_type: ConfigType,
    pub updated_at: f64,
    pub updated_by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigType {
    Robot,
    Safety,
    Network,
    Calibration,
    User,
}

impl ConfigState {
    pub fn new(key: &str, value: serde_json::Value, config_type: ConfigType, updated_by: &str) -> Self {
        Self {
            key: key.to_string(),
            value,
            config_type,
            updated_at: Utc::now().timestamp() as f64,
            updated_by: updated_by.to_string(),
        }
    }
}

/// State Manager - central state management
pub struct StateManager {
    robot_states: Arc<RwLock<HashMap<String, RobotState>>>,
    sessions: Arc<RwLock<HashMap<String, SessionState>>>,
    workflows: Arc<RwLock<HashMap<String, WorkflowState>>>,
    configs: Arc<RwLock<HashMap<String, ConfigState>>>,
    history: Arc<RwLock<Vec<StateEvent>>>,
}

impl StateManager {
    pub fn new() -> Self {
        Self {
            robot_states: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            workflows: Arc::new(RwLock::new(HashMap::new())),
            configs: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    // Robot State Management
    pub async fn register_robot(&self, state: RobotState) {
        let robot_id = state.robot_id.clone();
        let mut states = self.robot_states.write().await;
        states.insert(robot_id.clone(), state);
        drop(states);
        self.add_event(StateEvent::robot_registered(&robot_id)).await;
    }

    pub async fn get_robot_state(&self, robot_id: &str) -> Option<RobotState> {
        let states = self.robot_states.read().await;
        states.get(robot_id).cloned()
    }

    pub async fn update_robot_state(&self, robot_id: &str, state: RobotState) {
        let mut states = self.robot_states.write().await;
        states.insert(robot_id.to_string(), state);
    }

    pub async fn list_robots(&self) -> Vec<String> {
        let states = self.robot_states.read().await;
        states.keys().cloned().collect()
    }

    // Session State Management
    pub async fn create_session(&self, session_id: &str, client_id: &str) -> SessionState {
        let session = SessionState::new(session_id, client_id);
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.to_string(), session.clone());
        self.add_event(StateEvent::session_created(session_id)).await;
        session
    }

    pub async fn get_session(&self, session_id: &str) -> Option<SessionState> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    pub async fn update_session(&self, session_id: &str, update: impl FnOnce(&mut SessionState)) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            update(session);
        }
    }

    pub async fn remove_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        self.add_event(StateEvent::session_closed(session_id)).await;
    }

    // Workflow State Management
    pub async fn create_workflow(&self, workflow: WorkflowState) {
        let mut workflows = self.workflows.write().await;
        workflows.insert(workflow.workflow_id.clone(), workflow);
    }

    pub async fn get_workflow(&self, workflow_id: &str) -> Option<WorkflowState> {
        let workflows = self.workflows.read().await;
        workflows.get(workflow_id).cloned()
    }

    pub async fn update_workflow(&self, workflow_id: &str, update: impl FnOnce(&mut WorkflowState)) {
        let mut workflows = self.workflows.write().await;
        if let Some(workflow) = workflows.get_mut(workflow_id) {
            update(workflow);
        }
    }

    // Config State Management
    pub async fn set_config(&self, config: ConfigState) {
        let mut configs = self.configs.write().await;
        configs.insert(config.key.clone(), config);
    }

    pub async fn get_config(&self, key: &str) -> Option<ConfigState> {
        let configs = self.configs.read().await;
        configs.get(key).cloned()
    }

    pub async fn list_configs(&self, config_type: Option<ConfigType>) -> Vec<ConfigState> {
        let configs = self.configs.read().await;
        match config_type {
            Some(ct) => configs.values().filter(|c| c.config_type == ct).cloned().collect(),
            None => configs.values().cloned().collect(),
        }
    }

    // History
    pub async fn add_event(&self, event: StateEvent) {
        let mut history = self.history.write().await;
        history.push(event);
        
        // Keep only last 1000 events
        if history.len() > 1000 {
            let excess = history.len() - 1000;
            history.drain(0..excess);
        }
    }

    pub async fn get_history(&self, limit: usize) -> Vec<StateEvent> {
        let history = self.history.read().await;
        history.iter().rev().take(limit).cloned().collect()
    }

    // Snapshot
    pub async fn snapshot(&self) -> StateSnapshot {
        let robot_states = self.robot_states.read().await;
        let sessions = self.sessions.read().await;
        let workflows = self.workflows.read().await;
        let configs = self.configs.read().await;

        StateSnapshot {
            timestamp: Utc::now().timestamp() as f64,
            robot_count: robot_states.len(),
            session_count: sessions.len(),
            workflow_count: workflows.len(),
            config_count: configs.len(),
        }
    }
}

impl Default for StateManager {
    fn default() -> Self {
        Self::new()
    }
}

/// State Event for history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEvent {
    pub event_type: String,
    pub timestamp: f64,
    pub data: HashMap<String, String>,
}

impl StateEvent {
    pub fn new(event_type: &str) -> Self {
        Self {
            event_type: event_type.to_string(),
            timestamp: Utc::now().timestamp() as f64,
            data: HashMap::new(),
        }
    }

    pub fn with_data(mut self, key: &str, value: &str) -> Self {
        self.data.insert(key.to_string(), value.to_string());
        self
    }

    pub fn robot_registered(robot_id: &str) -> Self {
        Self::new("RobotRegistered").with_data("robotId", robot_id)
    }

    pub fn session_created(session_id: &str) -> Self {
        Self::new("SessionCreated").with_data("sessionId", session_id)
    }

    pub fn session_closed(session_id: &str) -> Self {
        Self::new("SessionClosed").with_data("sessionId", session_id)
    }
}

/// State Snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub timestamp: f64,
    pub robot_count: usize,
    pub session_count: usize,
    pub workflow_count: usize,
    pub config_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_robot_state() {
        let state = RobotState::new("robot-1", "arm", 6);
        assert_eq!(state.robot_id, "robot-1");
        assert_eq!(state.joint_angles.len(), 6);
    }

    #[test]
    fn test_session_state() {
        let session = SessionState::new("session-1", "client-1");
        assert!(!session.initialized);
        assert!(session.is_active(60.0));
    }

    #[test]
    fn test_workflow() {
        let steps = vec![
            WorkflowStep {
                step_id: 0,
                name: "Move".to_string(),
                actuation: "move_to".to_string(),
                arguments: HashMap::new(),
                status: StepStatus::Pending,
                started_at: None,
                completed_at: None,
                result: None,
                error: None,
            },
        ];
        let mut workflow = WorkflowState::new("wf-1", "test", steps);
        workflow.start();
        assert_eq!(workflow.status, WorkflowStatus::Running);
        assert_eq!(workflow.progress(), 0.0);
    }

    #[tokio::test]
    async fn test_state_manager() {
        let manager = StateManager::new();
        
        // Register robot
        let robot = RobotState::new("robot-1", "arm", 6);
        manager.register_robot(robot).await;
        
        let state = manager.get_robot_state("robot-1").await;
        assert!(state.is_some());
        
        let robots = manager.list_robots().await;
        assert_eq!(robots.len(), 1);
    }
}