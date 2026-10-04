//! PCP Safety Module
//!
//! Safety Constitution and Shadow Validation

use serde::{Deserialize, Serialize};
use crate::types::{ShadowPreview, ShadowStatus};
use chrono::Utc;
use sha2::{Sha256, Digest};

/// Safety Constitution - immutable hardware rules (ISO 10218 based)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConstitution {
    pub robot_id: String,
    pub rules: Vec<SafetyRule>,
    pub fingerprint: String,
    pub created_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub constraint_type: String,
    pub threshold: f64,
    pub enabled: bool,
}

impl SafetyConstitution {
    pub fn new(robot_id: impl Into<String>) -> Self {
        let robot_id = robot_id.into();
        let rules = Self::default_rules();
        let fingerprint = Self::compute_fingerprint(&rules);
        let created_at = Utc::now().timestamp() as f64;

        Self {
            robot_id,
            rules,
            fingerprint,
            created_at,
        }
    }

    fn default_rules() -> Vec<SafetyRule> {
        vec![
            SafetyRule {
                id: "CONST-01".to_string(),
                name: "Max Speed in Human Presence".to_string(),
                description: "TCP speed must not exceed 250mm/s when human is within safety zone".to_string(),
                constraint_type: "max_speed_m_s".to_string(),
                threshold: 0.25,
                enabled: true,
            },
            SafetyRule {
                id: "CONST-02".to_string(),
                name: "Max Force Limit".to_string(),
                description: "End-effector force must not exceed 150N in collaborative mode".to_string(),
                constraint_type: "max_force_n".to_string(),
                threshold: 150.0,
                enabled: true,
            },
            SafetyRule {
                id: "CONST-03".to_string(),
                name: "Forbidden Zone".to_string(),
                description: "Robot must never enter the forbidden zone (safety barrier)".to_string(),
                constraint_type: "forbidden_zone".to_string(),
                threshold: 0.0,
                enabled: true,
            },
            SafetyRule {
                id: "CONST-04".to_string(),
                name: "Human Clearance".to_string(),
                description: "Minimum distance from humans must be 500mm (ISO 10218)".to_string(),
                constraint_type: "min_human_clearance_m".to_string(),
                threshold: 0.5,
                enabled: true,
            },
            SafetyRule {
                id: "CONST-05".to_string(),
                name: "Emergency Stop".to_string(),
                description: "E-stop must always succeed and stop all motion within 100ms".to_string(),
                constraint_type: "estop_response_ms".to_string(),
                threshold: 100.0,
                enabled: true,
            },
            SafetyRule {
                id: "CONST-06".to_string(),
                name: "Shadow Validation Required".to_string(),
                description: "All actuations must pass shadow simulation before execution".to_string(),
                constraint_type: "shadow_required".to_string(),
                threshold: 1.0,
                enabled: true,
            },
        ]
    }

    fn compute_fingerprint(rules: &[SafetyRule]) -> String {
        let mut hasher = Sha256::new();
        for rule in rules {
            hasher.update(rule.id.as_bytes());
            hasher.update(rule.threshold.to_string().as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "robotId": self.robot_id,
            "fingerprint": self.fingerprint,
            "ruleCount": self.rules.len(),
            "rules": self.rules.iter().map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "name": r.name,
                    "enabled": r.enabled,
                })
            }).collect::<Vec<_>>(),
        })
    }

    pub fn validate(&self, params: &SafetyCheckParams) -> (bool, Vec<String>) {
        let mut violations = Vec::new();

        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }

            let valid = match rule.constraint_type.as_str() {
                "max_speed_m_s" => params.max_speed <= rule.threshold,
                "max_force_n" => params.max_force <= rule.threshold,
                "max_energy_j" => params.max_energy <= rule.threshold,
                "min_human_clearance_m" => params.human_clearance >= rule.threshold,
                "min_z_height" => params.target_z >= rule.threshold,
                _ => true,
            };

            if !valid {
                violations.push(format!("{}: {}", rule.id, rule.description));
            }
        }

        (violations.is_empty(), violations)
    }
}

/// Parameters for safety validation
#[derive(Debug, Clone, Default)]
pub struct SafetyCheckParams {
    pub max_speed: f64,
    pub max_force: f64,
    pub max_energy: f64,
    pub human_clearance: f64,
    pub target_z: f64,
    pub zone_id: Option<String>,
}

/// Safety Middleware - coordinates constitution and shadow validation
pub struct SafetyMiddleware {
    pub constitution: SafetyConstitution,
    pub shadow: ShadowSimulator,
    pub estop_active: bool,
}

impl SafetyMiddleware {
    pub fn new(robot_id: impl Into<String>) -> Self {
        Self {
            constitution: SafetyConstitution::new(robot_id),
            shadow: ShadowSimulator::new(),
            estop_active: false,
        }
    }

    /// Check an actuation through the safety pipeline (Constitution + Shadow).
    /// Lease enforcement happens in the caller against the real LeaseManager
    /// -- see methods.rs::handle_tools_call. This method previously accepted
    /// lease_token/zone_id parameters that were silently ignored (a no-op),
    /// which meant no actuation call was ever actually blocked by a missing
    /// or invalid lease. Removed rather than left as dead/misleading params.
    pub fn check(&self, name: &str, arguments: &serde_json::Value,
                 skip_shadow: bool) -> (bool, Option<ShadowPreview>, Vec<String>) {
        // 1. Check E-stop
        if self.estop_active {
            return (false, None, vec!["ESTOP active - all motion blocked".to_string()]);
        }

        // 2. Extract safety params from arguments
        let params = self.extract_params(name, arguments);

        // 4. Constitution check
        let (constitution_ok, violations) = self.constitution.validate(&params);
        if !constitution_ok {
            return (false, None, violations);
        }

        // 5. Shadow validation (if not skipped)
        if skip_shadow {
            let preview = ShadowPreview::safe(name, arguments.clone());
            return (true, Some(preview), vec![]);
        }

        let preview = self.shadow.preview(name, arguments);
        if preview.safe {
            (true, Some(preview), vec![])
        } else {
            let warnings = preview.warnings.clone();
            (false, Some(preview), warnings)
        }
    }

    fn extract_params(&self, _name: &str, args: &serde_json::Value) -> SafetyCheckParams {
        let mut params = SafetyCheckParams::default();

        if let Some(obj) = args.as_object() {
            if let Some(speed) = obj.get("speed").and_then(|v| v.as_f64()) {
                params.max_speed = speed;
            }
            if let Some(force) = obj.get("force").and_then(|v| v.as_f64()) {
                params.max_force = force;
            }
            if let Some(z) = obj.get("z").and_then(|v| v.as_f64()) {
                params.target_z = z;
            }
        }

        // Default values
        if params.max_speed == 0.0 {
            params.max_speed = 0.3; // default speed m/s
        }
        if params.max_force == 0.0 {
            params.max_force = 50.0; // default force N
        }
        params.human_clearance = 1.0; // Assume safe clearance

        params
    }

    pub fn set_estop(&mut self, active: bool) {
        self.estop_active = active;
        tracing::info!("[Safety] E-stop {}", if active { "ACTIVATED" } else { "RELEASED" });
    }
}

/// Shadow Simulator - geometric fallback collision detection
pub struct ShadowSimulator {
    workspace_bounds: Option<WorkspaceBounds>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceBounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
    pub min_z: f64,
    pub max_z: f64,
}

impl ShadowSimulator {
    pub fn new() -> Self {
        Self {
            workspace_bounds: Some(WorkspaceBounds {
                min_x: -1.0,
                max_x: 1.0,
                min_y: -1.0,
                max_y: 1.0,
                min_z: 0.0,
                max_z: 2.0,
            }),
        }
    }

    /// Run pre-flight shadow simulation
    pub fn preview(&self, name: &str, arguments: &serde_json::Value) -> ShadowPreview {
        let mut warnings = Vec::new();
        let mut status = ShadowStatus::Safe;
        let mut safe = true;

        // Extract target position
        let x = arguments.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = arguments.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let z = arguments.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let speed = arguments.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.3);

        // Check workspace bounds
        if let Some(bounds) = &self.workspace_bounds {
            if x < bounds.min_x || x > bounds.max_x ||
               y < bounds.min_y || y > bounds.max_y ||
               z < bounds.min_z || z > bounds.max_z {
                safe = false;
                status = ShadowStatus::WorkspaceViolation;
                warnings.push("Target position outside workspace bounds".to_string());
            }

            // Check floor guard
            if z < 0.0 {
                safe = false;
                status = ShadowStatus::WorkspaceViolation;
                warnings.push("Z target below floor (z < 0)".to_string());
            }
        }

        // Check speed limit (CONST-01)
        if speed > 0.25 {
            warnings.push(format!("Speed {} m/s exceeds collaborative limit (0.25 m/s)", speed));
        }

        let est_duration = 2.0; // estimated duration based on distance
        let est_energy = speed * 50.0; // simple energy estimate

        ShadowPreview {
            actuation_name: name.to_string(),
            arguments: arguments.clone(),
            status,
            safe,
            risk_score: if safe { 0.0 } else { 1.0 },
            sim_duration_s: 0.001, // geometric check is fast
            est_duration_s: est_duration,
            est_energy_j: est_energy,
            warnings,
            engine: "geometric".to_string(),
            collision_body: String::new(), // geometric validator doesn't compute this yet
            timestamp: Utc::now().timestamp() as f64,
        }
    }

    pub fn set_workspace(&mut self, bounds: WorkspaceBounds) {
        self.workspace_bounds = Some(bounds);
    }
}

impl Default for ShadowSimulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constitution_fingerprint() {
        let constitution = SafetyConstitution::new("test-robot");
        assert!(!constitution.fingerprint.is_empty());
    }

    #[test]
    fn test_shadow_preview_workspace_violation() {
        let sim = ShadowSimulator::new();
        let args = serde_json::json!({
            "x": 5.0,
            "y": 0.0,
            "z": 0.5,
            "speed": 0.1
        });
        let preview = sim.preview("move_to", &args);
        assert!(!preview.safe);
    }

    #[test]
    fn test_shadow_preview_safe() {
        let sim = ShadowSimulator::new();
        let args = serde_json::json!({
            "x": 0.0,
            "y": 0.0,
            "z": 0.5,
            "speed": 0.1
        });
        let preview = sim.preview("move_to", &args);
        assert!(preview.safe);
    }

    #[test]
    fn test_estop() {
        let mut safety = SafetyMiddleware::new("test-robot");
        safety.set_estop(true);
        let (ok, _, violations) = safety.check("move_to", &serde_json::json!({}), false);
        assert!(!ok);
        assert!(violations.iter().any(|v| v.contains("ESTOP")));
    }
}