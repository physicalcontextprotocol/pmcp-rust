//! PCP DID (Decentralized Identity) and Cryptographic Security Module
//!
//! Implements:
//! - DID:PCP method (did:pcp:<robot-id>)
//! - Ed25519 key pairs and signatures
//! - mTLS certificate generation helpers
//! - JWT-style capability tokens
//! - ZK proof stubs (groth16 interface)

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer, Verifier};
use rand::rngs::OsRng;

use crate::error::{PcpError, PcpErrorCode};

// ─────────────────────────────────────────────────────────────────────────────
//  DID METHOD
// ─────────────────────────────────────────────────────────────────────────────

pub const DID_METHOD: &str = "pcp";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Did {
    pub method: String,
    pub id: String,
}

impl Did {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            method: DID_METHOD.to_string(),
            id: id.into(),
        }
    }

    pub fn parse(did_str: &str) -> Result<Self, PcpError> {
        let parts: Vec<&str> = did_str.splitn(3, ':').collect();
        if parts.len() != 3 || parts[0] != "did" {
            return Err(PcpError::with_message(
                PcpErrorCode::InvalidParams,
                format!("Invalid DID format: {}", did_str),
            ));
        }
        Ok(Self {
            method: parts[1].to_string(),
            id: parts[2].to_string(),
        })
    }

    pub fn to_string(&self) -> String {
        format!("did:{}:{}", self.method, self.id)
    }
}

impl std::fmt::Display for Did {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "did:{}:{}", self.method, self.id)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  DID DOCUMENT
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationMethod {
    pub id: String,
    pub controller: String,
    pub verification_type: String,  // "Ed25519VerificationKey2020"
    pub public_key_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    pub id: String,
    pub service_type: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidDocument {
    pub context: Vec<String>,
    pub id: String,
    pub verification_method: Vec<VerificationMethod>,
    pub authentication: Vec<String>,
    pub assertion_method: Vec<String>,
    pub service: Vec<Service>,
    pub created: String,
    pub updated: String,
}

impl DidDocument {
    pub fn new(did: &Did, public_key_hex: &str) -> Self {
        let now = iso8601_now();
        let vm_id = format!("{}#key-1", did);
        Self {
            context: vec![
                "https://www.w3.org/ns/did/v1".to_string(),
                "https://w3id.org/security/suites/ed25519-2020/v1".to_string(),
            ],
            id: did.to_string(),
            verification_method: vec![VerificationMethod {
                id: vm_id.clone(),
                controller: did.to_string(),
                verification_type: "Ed25519VerificationKey2020".to_string(),
                public_key_hex: public_key_hex.to_string(),
            }],
            authentication: vec![vm_id.clone()],
            assertion_method: vec![vm_id],
            service: vec![],
            created: now.clone(),
            updated: now,
        }
    }

    pub fn add_service(&mut self, service_type: &str, endpoint: &str) {
        let id = format!("{}#service-{}", self.id, self.service.len() + 1);
        self.service.push(Service {
            id,
            service_type: service_type.to_string(),
            endpoint: endpoint.to_string(),
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  KEY MATERIAL
// ─────────────────────────────────────────────────────────────────────────────

pub struct RobotIdentity {
    pub did: Did,
    pub keypair: SigningKey,
    pub document: DidDocument,
}

impl RobotIdentity {
    /// Generate a fresh identity for a robot.
    pub fn generate(robot_id: impl Into<String>) -> Self {
        let mut csprng = OsRng;
        let keypair = SigningKey::generate(&mut csprng);
        let pub_hex = hex::encode(keypair.verifying_key().as_bytes());
        let did = Did::new(format!("robot:{}", robot_id.into()));
        let document = DidDocument::new(&did, &pub_hex);
        Self { did, keypair, document }
    }

    /// Sign arbitrary bytes. Returns hex-encoded signature.
    pub fn sign(&self, payload: &[u8]) -> String {
        let sig: Signature = self.keypair.sign(payload);
        hex::encode(sig.to_bytes())
    }

    /// Verify a hex-encoded signature over payload with this identity's public key.
    pub fn verify(&self, payload: &[u8], sig_hex: &str) -> Result<(), PcpError> {
        let sig_bytes = hex::decode(sig_hex)
            .map_err(|e| PcpError::with_message(PcpErrorCode::InvalidParams, e.to_string()))?;
        let sig_arr: [u8; 64] = sig_bytes.try_into()
            .map_err(|_| PcpError::with_message(PcpErrorCode::InvalidParams, "Signature must be 64 bytes".to_string()))?;
        let sig = Signature::from_bytes(&sig_arr);
        self.keypair.verifying_key().verify(payload, &sig)
            .map_err(|_| PcpError::with_message(PcpErrorCode::SignatureInvalid, "Signature verification failed".to_string()))
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.keypair.verifying_key().as_bytes())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  CAPABILITY TOKEN  (JWT-like, signed with Ed25519)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    pub issuer: String,        // DID of issuer
    pub subject: String,       // DID of subject
    pub audience: String,      // DID or service
    pub capabilities: Vec<String>,  // e.g. ["actuation:move_to", "sensor:lidar"]
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: String,
    pub signature: String,     // hex-encoded Ed25519 signature over canonical JSON
}

impl CapabilityToken {
    pub fn issue(
        identity: &RobotIdentity,
        subject: &str,
        audience: &str,
        capabilities: Vec<String>,
        valid_secs: u64,
    ) -> Self {
        let now = now_secs();
        let nonce = hex::encode(rand_bytes(16));

        let mut token = Self {
            issuer: identity.did.to_string(),
            subject: subject.to_string(),
            audience: audience.to_string(),
            capabilities,
            issued_at: now,
            expires_at: now + valid_secs,
            nonce,
            signature: String::new(),
        };

        let payload = token.canonical_payload();
        token.signature = identity.sign(&payload);
        token
    }

    pub fn verify(&self, identity: &RobotIdentity) -> Result<(), PcpError> {
        if self.issuer != identity.did.to_string() {
            return Err(PcpError::with_message(
                PcpErrorCode::SignatureInvalid,
                "Token issuer mismatch".to_string(),
            ));
        }
        let now = now_secs();
        if now > self.expires_at {
            return Err(PcpError::with_message(
                PcpErrorCode::TokenExpired,
                "Capability token has expired".to_string(),
            ));
        }
        let payload = self.canonical_payload();
        identity.verify(&payload, &self.signature)
    }

    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap || c == "*")
    }

    fn canonical_payload(&self) -> Vec<u8> {
        let obj = serde_json::json!({
            "issuer": self.issuer,
            "subject": self.subject,
            "audience": self.audience,
            "capabilities": self.capabilities,
            "issued_at": self.issued_at,
            "expires_at": self.expires_at,
            "nonce": self.nonce,
        });
        let mut s = serde_json::to_string(&obj).unwrap_or_default();
        s.as_bytes().to_vec()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  DID REGISTRY (in-memory)
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use tokio::sync::RwLock;

pub struct DidRegistry {
    documents: Arc<RwLock<HashMap<String, DidDocument>>>,
}

impl DidRegistry {
    pub fn new() -> Self {
        Self {
            documents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, doc: DidDocument) {
        let id = doc.id.clone();
        self.documents.write().await.insert(id, doc);
    }

    pub async fn resolve(&self, did: &str) -> Option<DidDocument> {
        self.documents.read().await.get(did).cloned()
    }

    pub async fn deregister(&self, did: &str) {
        self.documents.write().await.remove(did);
    }

    pub async fn list(&self) -> Vec<String> {
        self.documents.read().await.keys().cloned().collect()
    }

    /// Verify a JWS-style detached signature against a DID.
    pub async fn verify_signature(
        &self,
        did: &str,
        payload: &[u8],
        sig_hex: &str,
    ) -> Result<(), PcpError> {
        let doc = self.resolve(did).await
            .ok_or_else(|| PcpError::with_message(
                PcpErrorCode::NotFound,
                format!("DID {} not found in registry", did),
            ))?;

        let vm = doc.verification_method.first()
            .ok_or_else(|| PcpError::with_message(
                PcpErrorCode::InvalidParams,
                "No verification method in DID document".to_string(),
            ))?;

        let pub_bytes = hex::decode(&vm.public_key_hex)
            .map_err(|e| PcpError::with_message(PcpErrorCode::InvalidParams, e.to_string()))?;
        let pub_arr: [u8; 32] = pub_bytes.try_into()
            .map_err(|_| PcpError::with_message(PcpErrorCode::InvalidParams, "Public key must be 32 bytes".to_string()))?;

        let public_key = VerifyingKey::from_bytes(&pub_arr)
            .map_err(|e| PcpError::with_message(PcpErrorCode::InvalidParams, e.to_string()))?;

        let sig_bytes = hex::decode(sig_hex)
            .map_err(|e| PcpError::with_message(PcpErrorCode::InvalidParams, e.to_string()))?;
        let sig_arr: [u8; 64] = sig_bytes.try_into()
            .map_err(|_| PcpError::with_message(PcpErrorCode::InvalidParams, "Signature must be 64 bytes".to_string()))?;

        let sig = Signature::from_bytes(&sig_arr);

        public_key.verify(payload, &sig)
            .map_err(|_| PcpError::with_message(
                PcpErrorCode::SignatureInvalid,
                "DID signature verification failed".to_string(),
            ))
    }
}

impl Default for DidRegistry {
    fn default() -> Self { Self::new() }
}

// ─────────────────────────────────────────────────────────────────────────────
//  ZK PROOF STUBS  (groth16/plonk interface)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkCircuit {
    pub name: String,
    pub constraints: Vec<String>,  // simplified representation
    pub public_inputs: Vec<String>,
    pub private_inputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkProof {
    pub circuit: String,
    pub proof_bytes: Vec<u8>,
    pub public_inputs: Vec<String>,
    pub timestamp: u64,
    pub prover_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkVerificationKey {
    pub circuit: String,
    pub vk_bytes: Vec<u8>,
}

pub struct ZkSafetyProver {
    pub circuit: ZkCircuit,
}

impl ZkSafetyProver {
    /// Stub: generate a proof that a trajectory satisfies safety constraints.
    /// Real implementation would use bellman/halo2.
    pub fn prove(
        &self,
        trajectory: &serde_json::Value,
        prover_did: &str,
    ) -> Result<ZkProof, PcpError> {
        // Compute a deterministic proof stub based on trajectory hash
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(trajectory).unwrap_or_default());
        hasher.update(prover_did.as_bytes());
        let digest = hasher.finalize();

        Ok(ZkProof {
            circuit: self.circuit.name.clone(),
            proof_bytes: digest.to_vec(),
            public_inputs: vec![
                format!("trajectory_hash:{}", hex::encode(&digest[..8])),
            ],
            timestamp: now_secs(),
            prover_did: prover_did.to_string(),
        })
    }

    /// Stub: verify a proof. Real implementation calls bellman/halo2 verify.
    pub fn verify(&self, proof: &ZkProof, vk: &ZkVerificationKey) -> bool {
        proof.circuit == self.circuit.name
            && proof.proof_bytes.len() == 32
            && !proof.prover_did.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  HELPERS
// ─────────────────────────────────────────────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn iso8601_now() -> String {
    let secs = now_secs();
    format!("{}Z", secs)  // simplified; real impl uses chrono
}

fn rand_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    OsRng.fill_bytes(&mut buf);
    buf
}
