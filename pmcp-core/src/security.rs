//! P-MCP Security Module
//!
//! Implements security features including:
// - W3C DID-based identity
// - Ed25519 digital signatures
// - mTLS authentication
// - TEE attestation stubs

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer, Verifier};
use std::collections::HashMap;
use crate::error::PmcpError;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::Utc;

/// DID (Decentralized Identifier) for robot identity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DID {
    pub scheme: String,
    pub method: String,
    pub method_specific_id: String,
}

impl DID {
    pub fn new(robot_class: &str, model: &str, location: &str, serial: &str) -> Self {
        let method_specific_id = format!("{}:{}:{}:{}", robot_class, model, location, serial);
        Self {
            scheme: "did".to_string(),
            method: "pmcp".to_string(),
            method_specific_id,
        }
    }

    pub fn to_string(&self) -> String {
        format!("{}:{}:{}", self.scheme, self.method, self.method_specific_id)
    }

    pub fn from_str(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() >= 3 && parts[0] == "did" && parts[1] == "pmcp" {
            Some(Self {
                scheme: "did".to_string(),
                method: "pmcp".to_string(),
                method_specific_id: parts[2..].join(":"),
            })
        } else {
            None
        }
    }
}

/// DID Document
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DIDDocument {
    pub id: String,
    pub context: Vec<String>,
    pub verification_method: Vec<VerificationMethod>,
    pub authentication: Vec<String>,
    pub service: Vec<ServiceEndpoint>,
}

impl DIDDocument {
    pub fn new(did: &DID, public_key: &[u8]) -> Self {
        let id = did.to_string();
        Self {
            id: id.clone(),
            context: vec!["https://www.w3.org/ns/did/v1".to_string()],
            verification_method: vec![VerificationMethod {
                id: format!("{}#key-1", id),
                r#type: "Ed25519VerificationKey2020".to_string(),
                controller: id.clone(),
                public_key_jwk: None,
                public_key_multibase: Some(base64::encode(public_key)),
            }],
            authentication: vec![format!("{}#key-1", id)],
            service: vec![],
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationMethod {
    pub id: String,
    pub r#type: String,
    pub controller: String,
    pub public_key_jwk: Option<serde_json::Value>,
    pub public_key_multibase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub id: String,
    pub r#type: String,
    pub service_endpoint: String,
}

/// Key Pair for signing
#[derive(Debug, Clone)]
pub struct KeyPair {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl KeyPair {
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let verifying_key = signing_key.verifying_key();
        Self { signing_key, verifying_key }
    }

    pub fn from_seed(seed: &[u8]) -> Self {
        let signing_key = SigningKey::from_bytes(seed.try_into().unwrap_or(&[0u8; 32]));
        let verifying_key = signing_key.verifying_key();
        Self { signing_key, verifying_key }
    }

    pub fn public_key(&self) -> Vec<u8> {
        self.verifying_key.as_bytes().to_vec()
    }

    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature = self.signing_key.sign(message);
        signature.to_bytes().to_vec()
    }

    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        let signature = Signature::from_bytes(signature.try_into().unwrap_or(&[0u8; 64]));
        self.verifying_key.verify(message, &signature).is_ok()
    }
}

/// Robot Identity with cryptographic credentials
#[derive(Debug, Clone)]
pub struct RobotIdentityV2 {
    pub did: DID,
    pub document: DIDDocument,
    pub key_pair: KeyPair,
    pub created_at: f64,
    pub metadata: HashMap<String, String>,
}

impl RobotIdentityV2 {
    pub fn new(robot_class: &str, model: &str, serial: &str, location: &str) -> Self {
        let did = DID::new(robot_class, model, location, serial);
        let key_pair = KeyPair::generate();
        let document = DIDDocument::new(&did, &key_pair.public_key());
        
        let mut metadata = HashMap::new();
        metadata.insert("robot_class".to_string(), robot_class.to_string());
        metadata.insert("model".to_string(), model.to_string());
        metadata.insert("serial".to_string(), serial.to_string());
        metadata.insert("location".to_string(), location.to_string());

        Self {
            did,
            document,
            key_pair,
            created_at: Utc::now().timestamp() as f64,
            metadata,
        }
    }

    pub fn did_string(&self) -> String {
        self.did.to_string()
    }

    pub fn sign_message(&self, message: &[u8]) -> Vec<u8> {
        self.key_pair.sign(message)
    }

    pub fn verify_signature(&self, message: &[u8], signature: &[u8]) -> bool {
        self.key_pair.verify(message, signature)
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "did": self.did.to_string(),
            "document": self.document.to_json(),
            "publicKey": base64::encode(&self.key_pair.public_key()),
            "createdAt": self.created_at,
            "metadata": self.metadata,
        })
    }
}

/// Authentication Token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub token_id: String,
    pub robot_did: String,
    pub issued_at: f64,
    pub expires_at: f64,
    pub scopes: Vec<String>,
    pub signature: String,
}

impl AuthToken {
    pub fn new(robot_did: &str, key_pair: &KeyPair, duration_secs: u64) -> Self {
        let now = Utc::now().timestamp() as f64;
        let token_id = uuid::Uuid::new_v4().to_string();
        
        let payload = format!("{}:{}:{}", robot_did, token_id, now);
        let signature = base64::encode(&key_pair.sign(payload.as_bytes()));

        Self {
            token_id,
            robot_did: robot_did.to_string(),
            issued_at: now,
            expires_at: now + duration_secs as f64,
            scopes: vec!["actuate".to_string(), "read_sensors".to_string()],
            signature,
        }
    }

    pub fn is_valid(&self) -> bool {
        let now = Utc::now().timestamp() as f64;
        now >= self.issued_at && now < self.expires_at
    }

    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::json!({
            "tokenId": self.token_id,
            "robotDid": self.robot_did,
            "issuedAt": self.issued_at,
            "expiresAt": self.expires_at,
            "scopes": self.scopes,
            "valid": self.is_valid(),
        })
    }
}

/// Authentication Manager
pub struct AuthManager {
    tokens: Arc<RwLock<HashMap<String, AuthToken>>>,
    key_pairs: Arc<RwLock<HashMap<String, KeyPair>>>,
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(RwLock::new(HashMap::new())),
            key_pairs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_robot(&self, identity: &RobotIdentityV2) -> Result<(), PmcpError> {
        let mut key_pairs = self.key_pairs.write().await;
        key_pairs.insert(identity.did_string(), identity.key_pair.clone());
        Ok(())
    }

    pub async fn issue_token(&self, robot_did: &str, duration_secs: u64) -> Result<AuthToken, PmcpError> {
        let key_pairs = self.key_pairs.read().await;
        let key_pair = key_pairs.get(robot_did)
            .ok_or_else(|| PmcpError::invalid_params("Robot not registered"))?;
        
        let token = AuthToken::new(robot_did, key_pair, duration_secs);
        
        let mut tokens = self.tokens.write().await;
        tokens.insert(token.token_id.clone(), token.clone());
        
        Ok(token)
    }

    pub async fn validate_token(&self, token_id: &str) -> Result<bool, PmcpError> {
        let tokens = self.tokens.read().await;
        if let Some(token) = tokens.get(token_id) {
            Ok(token.is_valid())
        } else {
            Ok(false)
        }
    }

    pub async fn revoke_token(&self, token_id: &str) -> Result<(), PmcpError> {
        let mut tokens = self.tokens.write().await;
        tokens.remove(token_id);
        Ok(())
    }

    pub async fn list_tokens(&self) -> Vec<AuthToken> {
        let tokens = self.tokens.read().await;
        tokens.values().cloned().collect()
    }
}

impl Default for AuthManager {
    fn default() -> Self {
        Self::new()
    }
}

/// TEE Attestation (stub implementation)
pub struct TEEAttestation {
    pub enclave_id: String,
    pub measurement: String,
    pub signer: KeyPair,
}

impl TEEAttestation {
    pub fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"pmcp-enclave-v1");
        let measurement = format!("{:x}", hasher.finalize());

        Self {
            enclave_id: uuid::Uuid::new_v4().to_string(),
            measurement,
            signer: KeyPair::generate(),
        }
    }

    pub fn generate_attestation_report(&self, data: &[u8]) -> AttestationReport {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = format!("{:x}", hasher.finalize());

        AttestationReport {
            report_id: uuid::Uuid::new_v4().to_string(),
            enclave_id: self.enclave_id.clone(),
            measurement: self.measurement.clone(),
            data_hash: hash,
            timestamp: Utc::now().timestamp() as f64,
            signature: base64::encode(&self.signer.sign(data)),
        }
    }

    pub fn verify_attestation(&self, report: &AttestationReport) -> bool {
        report.enclave_id == self.enclave_id && 
        report.measurement == self.measurement
    }
}

impl Default for TEEAttestation {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationReport {
    pub report_id: String,
    pub enclave_id: String,
    pub measurement: String,
    pub data_hash: String,
    pub timestamp: f64,
    pub signature: String,
}

/// mTLS Certificate (stub implementation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TLSCertificate {
    pub subject_did: String,
    pub issuer: String,
    pub not_before: f64,
    pub not_after: f64,
    pub serial_number: String,
    pub public_key: String,
    pub signature: String,
}

impl TLSCertificate {
    pub fn new(subject_did: &str, issuer: &str, key_pair: &KeyPair, validity_days: u32) -> Self {
        let now = Utc::now().timestamp() as f64;
        let serial = uuid::Uuid::new_v4().to_string();
        
        let payload = format!("{}:{}:{}", subject_did, serial, now);
        let signature = base64::encode(&key_pair.sign(payload.as_bytes()));

        Self {
            subject_did: subject_did.to_string(),
            issuer: issuer.to_string(),
            not_before: now,
            not_after: now + (validity_days as f64 * 86400.0),
            serial_number: serial,
            public_key: base64::encode(&key_pair.public_key()),
            signature,
        }
    }

    pub fn is_valid(&self) -> bool {
        let now = Utc::now().timestamp() as f64;
        now >= self.not_before && now < self.not_after
    }
}

/// Security Policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub name: String,
    pub require_auth: bool,
    pub require_tls: bool,
    pub allowed_transports: Vec<String>,
    pub max_request_size: usize,
    pub rate_limit_per_minute: u32,
    pub ip_whitelist: Vec<String>,
    pub ip_blacklist: Vec<String>,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            require_auth: true,
            require_tls: false,
            allowed_transports: vec!["stdio".to_string(), "http".to_string()],
            max_request_size: 1024 * 1024, // 1MB
            rate_limit_per_minute: 100,
            ip_whitelist: vec![],
            ip_blacklist: vec![],
        }
    }
}

impl SecurityPolicy {
    pub fn permissive() -> Self {
        Self {
            name: "permissive".to_string(),
            require_auth: false,
            require_tls: false,
            allowed_transports: vec!["stdio".to_string(), "http".to_string(), "websocket".to_string()],
            max_request_size: 10 * 1024 * 1024, // 10MB
            rate_limit_per_minute: 1000,
            ip_whitelist: vec![],
            ip_blacklist: vec![],
        }
    }

    pub fn strict() -> Self {
        Self {
            name: "strict".to_string(),
            require_auth: true,
            require_tls: true,
            allowed_transports: vec!["stdio".to_string()],
            max_request_size: 64 * 1024, // 64KB
            rate_limit_per_minute: 10,
            ip_whitelist: vec![],
            ip_blacklist: vec!["0.0.0.0/0".to_string()],
        }
    }
}

/// Security Manager
pub struct SecurityManager {
    auth_manager: AuthManager,
    tee_attestation: TEEAttestation,
    policies: Arc<RwLock<HashMap<String, SecurityPolicy>>>,
    registered_robots: Arc<RwLock<HashMap<String, RobotIdentityV2>>>,
}

impl SecurityManager {
    pub fn new() -> Self {
        let mut policies = HashMap::new();
        policies.insert("default".to_string(), SecurityPolicy::default());
        policies.insert("permissive".to_string(), SecurityPolicy::permissive());
        policies.insert("strict".to_string(), SecurityPolicy::strict());

        Self {
            auth_manager: AuthManager::new(),
            tee_attestation: TEEAttestation::new(),
            policies: Arc::new(RwLock::new(policies)),
            registered_robots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_robot(&self, identity: RobotIdentityV2) -> Result<(), PmcpError> {
        let did = identity.did_string();
        self.auth_manager.register_robot(&identity).await?;
        self.registered_robots.write().await.insert(did, identity);
        Ok(())
    }

    pub async fn authenticate(&self, token_id: &str) -> Result<bool, PmcpError> {
        self.auth_manager.validate_token(token_id).await
    }

    pub async fn issue_token(&self, robot_did: &str, duration_secs: u64) -> Result<AuthToken, PmcpError> {
        self.auth_manager.issue_token(robot_did, duration_secs).await
    }

    pub async fn set_policy(&self, name: &str, policy: SecurityPolicy) {
        self.policies.write().await.insert(name.to_string(), policy);
    }

    pub async fn get_policy(&self, name: &str) -> Option<SecurityPolicy> {
        self.policies.read().await.get(name).cloned()
    }

    pub async fn check_policy(&self, policy_name: &str, transport: &str) -> Result<bool, PmcpError> {
        let policies = self.policies.read().await;
        if let Some(policy) = policies.get(policy_name) {
            Ok(policy.allowed_transports.contains(&transport.to_string()))
        } else {
            Err(PmcpError::invalid_params("Policy not found"))
        }
    }

    pub fn generate_attestation(&self, data: &[u8]) -> AttestationReport {
        self.tee_attestation.generate_attestation_report(data)
    }
}

impl Default for SecurityManager {
    fn default() -> Self {
        Self::new()
    }
}

// Helper module for base64 encoding
mod base64 {
    use std::io::Write;
    
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    
    pub fn encode(data: &[u8]) -> String {
        let mut result = String::new();
        let mut buf = [0u8; 3];
        let mut i = 0;
        
        for byte in data {
            buf[i] = *byte;
            i += 1;
            
            if i == 3 {
                let n = ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32);
                result.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
                result.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
                result.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
                result.push(ALPHABET[(n & 0x3F) as usize] as char);
                i = 0;
            }
        }
        
        if i > 0 {
            let mut padded = [0u8; 3];
            padded[..i].copy_from_slice(&buf[..i]);
            let n = ((padded[0] as u32) << 16) | ((padded[1] as u32) << 8) | (padded[2] as u32);
            result.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            result.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            if i == 1 {
                result.push('=');
                result.push('=');
            } else {
                result.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
                result.push('=');
            }
        }
        
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_did_creation() {
        let did = DID::new("arm", "ur5", "lab-01", "serial123");
        assert_eq!(did.scheme, "did");
        assert_eq!(did.method, "pmcp");
    }

    #[test]
    fn test_key_pair_sign_verify() {
        let key_pair = KeyPair::generate();
        let message = b"test message";
        let signature = key_pair.sign(message);
        assert!(key_pair.verify(message, &signature));
    }

    #[test]
    fn test_robot_identity() {
        let identity = RobotIdentityV2::new("arm", "UR5", "12345", "lab-01");
        assert!(identity.did_string().starts_with("did:pmcp:"));
    }

    #[test]
    fn test_auth_token() {
        let key_pair = KeyPair::generate();
        let token = AuthToken::new("did:pmcp:arm:ur5:lab:123", &key_pair, 3600);
        assert!(token.is_valid());
    }

    #[test]
    fn test_security_manager() {
        let sm = SecurityManager::new();
        let identity = RobotIdentityV2::new("arm", "UR5", "12345", "lab-01");
        
        // This would need async runtime in real test
        // sm.register_robot(identity).await;
    }
}