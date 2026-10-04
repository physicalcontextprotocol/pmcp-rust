//! PCP Python Bindings
//!
//! PyO3-based Python bindings for the PCP Rust core

use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::sync::Arc;

use crate::types::*;
use crate::server::{PCPServer, PCPServerBuilder};
use crate::error::PcpErrorCode;

/// PCP Python Module
#[pymodule]
pub fn pcp_core(_py: Python, m: &PyModule) -> PyResult<()> {
    // Register types
    m.add_class::<PyActuationSpec>()?;
    m.add_class::<PySensorSpec>()?;
    m.add_class::<PyActuationResult>()?;
    m.add_class::<PySensorReading>()?;
    m.add_class::<PyLeaseGrant>()?;
    m.add_class::<PyServerStatus>()?;
    m.add_class::<PyPCPServer>()?;

    // Register constants
    m.add("PCP_VERSION", PCP_VERSION)?;
    m.add("MCP_VERSION", MCP_VERSION)?;

    Ok(())
}

// ============================================================================
// Python Wrapper Types
// ============================================================================

/// Python wrapper for ActuationSpec
#[pyclass]
struct PyActuationSpec {
    inner: ActuationSpec,
}

#[pymethods]
impl PyActuationSpec {
    #[new]
    fn new(
        name: String,
        description: String,
        parameters: Vec<PyActuationParameter>,
        robot_id: String,
        max_speed_m_s: Option<f64>,
        max_force_n: Option<f64>,
        max_energy_j: Option<f64>,
    ) -> Self {
        let params: Vec<ActuationParameter> = parameters
            .into_iter()
            .map(|p| p.into())
            .collect();

        Self {
            inner: ActuationSpec {
                name,
                description,
                parameters: params,
                robot_id,
                max_speed_m_s: max_speed_m_s.unwrap_or(1.0),
                max_force_n: max_force_n.unwrap_or(100.0),
                max_energy_j: max_energy_j.unwrap_or(500.0),
                ..Default::default()
            },
        }
    }

    fn to_mcp_tool(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            Ok(self.inner.to_mcp_tool().to_object(py))
        })
    }
}

/// Python wrapper for ActuationParameter
#[pyclass]
struct PyActuationParameter {
    name: String,
    param_type: String,
    description: String,
    required: bool,
    default: Option<serde_json::Value>,
}

#[pymethods]
impl PyActuationParameter {
    #[new]
    fn new(
        name: String,
        param_type: String,
        description: String,
        required: Option<bool>,
        default: Option<serde_json::Value>,
    ) -> Self {
        Self {
            name,
            param_type,
            description,
            required: required.unwrap_or(true),
            default,
        }
    }
}

impl From<PyActuationParameter> for ActuationParameter {
    fn from(p: PyActuationParameter) -> Self {
        ActuationParameter {
            name: p.name,
            param_type: p.param_type,
            description: p.description,
            required: p.required,
            default: p.default,
            ..Default::default()
        }
    }
}

/// Python wrapper for SensorSpec
#[pyclass]
struct PySensorSpec {
    inner: SensorSpec,
}

#[pymethods]
impl PySensorSpec {
    #[new]
    fn new(
        name: String,
        description: String,
        robot_id: String,
        sensor_type: String,
        unit: Option<String>,
        hz: Option<f64>,
    ) -> Self {
        let st = match sensor_type.as_str() {
            "joint_states" => SensorType::JointStates,
            "end_effector" => SensorType::EndEffector,
            "force_torque" => SensorType::ForceTorque,
            "camera_rgb" => SensorType::CameraRgb,
            "camera_depth" => SensorType::CameraDepth,
            "lidar" => SensorType::Lidar,
            "imu" => SensorType::Imu,
            "battery" => SensorType::Battery,
            "temperature" => SensorType::Temperature,
            "proximity" => SensorType::Proximity,
            "gps" => SensorType::Gps,
            "odometry" => SensorType::Odometry,
            "plant_health" => SensorType::PlantHealth,
            "energy_meter" => SensorType::EnergyMeter,
            _ => SensorType::Custom,
        };

        Self {
            inner: SensorSpec {
                name,
                description,
                robot_id,
                sensor_type: st,
                unit: unit.unwrap_or_default(),
                hz: hz.unwrap_or(10.0),
                ..Default::default()
            },
        }
    }

    fn to_mcp_resource(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            Ok(self.inner.to_mcp_resource().to_object(py))
        })
    }

    fn uri(&self) -> String {
        self.inner.uri()
    }
}

/// Python wrapper for ActuationResult
#[pyclass]
struct PyActuationResult {
    inner: ActuationResult,
}

#[pymethods]
impl PyActuationResult {
    #[new]
    fn new(success: bool, output: Option<serde_json::Value>, error_message: Option<String>) -> Self {
        Self {
            inner: ActuationResult {
                success,
                output: output.unwrap_or(serde_json::Value::Null),
                error_message: error_message.unwrap_or_default(),
                timestamp: chrono::Utc::now().timestamp() as f64,
                ..Default::default()
            },
        }
    }

    fn to_mcp_content(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            Ok(self.inner.to_mcp_content().to_object(py))
        })
    }

    #[getter]
    fn success(&self) -> bool {
        self.inner.success
    }

    #[getter]
    fn output(&self) -> PyObject {
        Python::with_gil(|py| self.inner.output.to_object(py))
    }

    #[getter]
    fn error_message(&self) -> String {
        self.inner.error_message.clone()
    }
}

/// Python wrapper for SensorReading
#[pyclass]
struct PySensorReading {
    inner: SensorReading,
}

#[pymethods]
impl PySensorReading {
    #[new]
    fn new(sensor_name: String, robot_id: String, value: serde_json::Value, unit: Option<String>) -> Self {
        Self {
            inner: SensorReading {
                sensor_name,
                robot_id,
                value,
                unit: unit.unwrap_or_default(),
                timestamp: chrono::Utc::now().timestamp() as f64,
                ..Default::default()
            },
        }
    }

    fn to_mcp_content(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            Ok(self.inner.to_mcp_content().to_object(py))
        })
    }
}

/// Python wrapper for LeaseGrant
#[pyclass]
struct PyLeaseGrant {
    inner: LeaseGrant,
}

#[pymethods]
impl PyLeaseGrant {
    #[getter]
    fn lease_id(&self) -> String {
        self.inner.lease_id.clone()
    }

    #[getter]
    fn robot_id(&self) -> String {
        self.inner.robot_id.clone()
    }

    #[getter]
    fn zone_id(&self) -> String {
        self.inner.zone_id.clone()
    }

    #[getter]
    fn state(&self) -> String {
        format!("{:?}", self.inner.state)
    }

    #[getter]
    fn expires_at(&self) -> f64 {
        self.inner.expires_at
    }

    fn to_dict(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            Ok(self.inner.to_dict().to_object(py))
        })
    }
}

/// Python wrapper for ServerStatus
#[pyclass]
struct PyServerStatus {
    inner: ServerStatus,
}

#[pymethods]
impl PyServerStatus {
    #[getter]
    fn robot_id(&self) -> String {
        self.inner.robot_id.clone()
    }

    #[getter]
    fn pcp_version(&self) -> String {
        self.inner.pcp_version.clone()
    }

    #[getter]
    fn uptime_s(&self) -> f64 {
        self.inner.uptime_s
    }

    #[getter]
    fn call_count(&self) -> u64 {
        self.inner.call_count
    }

    #[getter]
    fn blocked_count(&self) -> u64 {
        self.inner.blocked_count
    }

    fn to_dict(&self) -> PyResult<PyObject> {
        Python::with_gil(|py| {
            let dict = PyDict::new(py);
            dict.set_item("robot_id", &self.inner.robot_id)?;
            dict.set_item("pcp_version", &self.inner.pcp_version)?;
            dict.set_item("uptime_s", self.inner.uptime_s)?;
            dict.set_item("call_count", self.inner.call_count)?;
            dict.set_item("blocked_count", self.inner.blocked_count)?;
            Ok(dict.to_object(py))
        })
    }
}

/// Python wrapper for PCPServer
#[pyclass]
struct PyPCPServer {
    server: Arc<PCPServer>,
}

#[pymethods]
impl PyPCPServer {
    /// Create a new PCPServer
    #[new]
    fn new(
        name: String,
        version: Option<String>,
        robot_id: Option<String>,
        robot_class: Option<String>,
        model: Option<String>,
        serial: Option<String>,
        location: Option<String>,
    ) -> PyResult<Self> {
        let server = PCPServer::new(
            name,
            version.unwrap_or_else(|| "1.0.0".to_string()),
            robot_id,
            &robot_class.unwrap_or_else(|| "arm".to_string()),
            &model.unwrap_or_else(|| "generic".to_string()),
            &serial.unwrap_or_else(|| "000000".to_string()),
            &location.unwrap_or_else(|| "lab-01".to_string()),
        );

        Ok(Self { server: Arc::new(server) })
    }

    /// Register an actuation
    fn register_actuation(&self, spec: PyActuationSpec) -> PyResult<()> {
        // In real implementation, would register
        Ok(())
    }

    /// Register a sensor
    fn register_sensor(&self, spec: PySensorSpec) -> PyResult<()> {
        // In real implementation, would register
        Ok(())
    }

    /// Handle a JSON-RPC message
    fn handle_message(&self, raw: String) -> PyResult<Option<String>> {
        let raw_json: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

        // In real implementation, would call server.handle_message
        // For now, return a placeholder
        Ok(Some(serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "result": {}
        }).to_string()))
    }

    /// Get server status
    fn status(&self) -> PyResult<PyServerStatus> {
        Ok(PyServerStatus {
            inner: ServerStatus {
                robot_id: "python-test".to_string(),
                pcp_version: PCP_VERSION.to_string(),
                uptime_s: 0.0,
                call_count: 0,
                blocked_count: 0,
                ..Default::default()
            },
        })
    }
}

/// Error codes
#[pyclass]
struct PyPcpErrorCode;

#[pymethods]
impl PyPcpErrorCode {
    #[classattr]
    fn PARSE_ERROR() -> i32 { PcpErrorCode::ParseError.code() }

    #[classattr]
    fn INVALID_REQUEST() -> i32 { PcpErrorCode::InvalidRequest.code() }

    #[classattr]
    fn METHOD_NOT_FOUND() -> i32 { PcpErrorCode::MethodNotFound.code() }

    #[classattr]
    fn INVALID_PARAMS() -> i32 { PcpErrorCode::InvalidParams.code() }

    #[classattr]
    fn INTERNAL_ERROR() -> i32 { PcpErrorCode::InternalError.code() }

    #[classattr]
    fn SHADOW_BLOCKED() -> i32 { PcpErrorCode::ShadowBlocked.code() }

    #[classattr]
    fn CONSTITUTION_BLOCKED() -> i32 { PcpErrorCode::ConstitutionBlocked.code() }

    #[classattr]
    fn LEASE_REQUIRED() -> i32 { PcpErrorCode::LeaseRequired.code() }

    #[classattr]
    fn ESTOP_ACTIVE() -> i32 { PcpErrorCode::EstopActive.code() }
}

// ============================================================================
// Convenience Functions for Python
// ============================================================================

/// Create a new actuation specification
#[pyfunction]
fn create_actuation_spec(
    name: String,
    description: String,
    parameters: Vec<PyActuationParameter>,
    robot_id: String,
    max_speed_m_s: Option<f64>,
) -> PyResult<PyActuationSpec> {
    Ok(PyActuationSpec::new(
        name,
        description,
        parameters,
        robot_id,
        max_speed_m_s,
        None,
        None,
    ))
}

/// Create a new sensor specification
#[pyfunction]
fn create_sensor_spec(
    name: String,
    description: String,
    robot_id: String,
    sensor_type: String,
    unit: Option<String>,
) -> PyResult<PySensorSpec> {
    Ok(PySensorSpec::new(
        name,
        description,
        robot_id,
        sensor_type,
        unit,
        None,
    ))
}

/// Create a successful actuation result
#[pyfunction]
fn success_result(output: Option<serde_json::Value>) -> PyResult<PyActuationResult> {
    Ok(PyActuationResult::new(true, output, None))
}

/// Create a failed actuation result
#[pyfunction]
fn failure_result(error: String) -> PyResult<PyActuationResult> {
    Ok(PyActuationResult::new(false, None, Some(error)))
}

/// Create a server status
#[pyfunction]
fn server_status(robot_id: String, uptime_s: f64, call_count: u64, blocked_count: u64) -> PyResult<PyServerStatus> {
    Ok(PyServerStatus {
        inner: ServerStatus {
            robot_id,
            pcp_version: PCP_VERSION.to_string(),
            uptime_s,
            call_count,
            blocked_count,
            ..Default::default()
        },
    })
}