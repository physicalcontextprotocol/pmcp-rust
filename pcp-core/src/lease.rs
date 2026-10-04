//! PCP Lease Management
//!
//! Temporal zone ownership. Competing requests for an occupied zone are
//! resolved by a simple first-price comparison (highest bid_energy_j wins;
//! the winner's own bid, not a second price, is what's compared) -- this
//! module's docstring previously and incorrectly called this "Vickrey-style",
//! which specifically means a second-price sealed-bid auction. No second-price
//! mechanism exists here; renamed to describe the actual mechanism honestly.
//!
//! Fencing tokens (Kleppmann 2016): a monotonically increasing counter is
//! issued on every grant and renewal, and MUST be presented on every
//! subsequent actuation call, not only at lease-request time. This closes
//! the classic paused-holder-plus-clock-skew failure mode: even if a
//! caller's original grant is still lexically valid (lease_id, TTL), a
//! stale fence token proves the caller's view of zone ownership is out of
//! date. Today this is a single-process monotonic counter per zone, which
//! is honest about current maturity (no Raft-backed multi-replica lease
//! authority yet); SAFETY_ARCHITECTURE.md's Raft term numbers are the
//! intended source once that ships.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::types::{LeaseGrant, LeaseRequest, LeaseState};
use chrono::Utc;

/// Lease Manager - handles temporal zone ownership
pub struct LeaseManager {
    leases: Arc<RwLock<HashMap<String, LeaseGrant>>>,
    fence_counters: Arc<RwLock<HashMap<String, u64>>>,
}

impl LeaseManager {
    pub fn new() -> Self {
        Self {
            leases: Arc::new(RwLock::new(HashMap::new())),
            fence_counters: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn next_fence_token(&self, zone_id: &str) -> u64 {
        let mut counters = self.fence_counters.write().await;
        let counter = counters.entry(zone_id.to_string()).or_insert(0);
        *counter += 1;
        *counter
    }

    /// Request a lease for a zone
    pub async fn request(&self, req: &LeaseRequest) -> LeaseGrant {
        let mut leases = self.leases.write().await;
        let zone_id = &req.zone_id;

        if let Some(existing) = leases.get(zone_id) {
            if existing.is_valid() {
                // Check if same robot is renewing
                if existing.robot_id == req.robot_id {
                    // Renew — bump the fence token too. A caller holding a
                    // reference to the pre-renewal grant must re-fetch the
                    // new token; a "renewal" the caller doesn't know about
                    // is exactly the stale-holder scenario fencing exists
                    // to catch.
                    let new_expiry = (Utc::now().timestamp() as f64) + (req.duration_ms as f64 / 1000.0);
                    let fence_token = self.next_fence_token(zone_id).await;
                    let renewed = LeaseGrant {
                        lease_id: existing.lease_id.clone(),
                        robot_id: existing.robot_id.clone(),
                        zone_id: existing.zone_id.clone(),
                        state: LeaseState::Active,
                        expires_at: new_expiry,
                        bid_energy_j: req.bid_energy_j,
                        fence_token,
                    };
                    leases.insert(zone_id.clone(), renewed.clone());
                    tracing::info!("[Lease] RENEWED zone={} robot={} fence={} ({})",
                        zone_id, req.robot_id, fence_token, req.duration_ms);
                    return renewed;
                }

                // Another robot holds the zone — highest bid wins (first-price:
                // the winner's own bid is what's compared, not a second price)
                if req.bid_energy_j <= existing.bid_energy_j {
                    tracing::info!("[Lease] DENIED zone={} robot={} (insufficient bid: {} <= {})",
                        zone_id, req.robot_id, req.bid_energy_j, existing.bid_energy_j);
                    return LeaseGrant::denied(&req.robot_id, zone_id);
                }

                // Higher bid wins - revoke existing
                tracing::info!("[Lease] OUTBID zone={} new={} old={} ({} > {})",
                    zone_id, req.robot_id, existing.robot_id,
                    req.bid_energy_j, existing.bid_energy_j);
            }
        }

        // Grant new lease
        let fence_token = self.next_fence_token(zone_id).await;
        let grant = LeaseGrant::new(&req.robot_id, &req.zone_id, req.duration_ms, req.bid_energy_j, fence_token);
        leases.insert(zone_id.clone(), grant.clone());
        tracing::info!("[Lease] ACTIVE zone={} robot={} fence={} ({})",
            zone_id, req.robot_id, fence_token, req.duration_ms);
        grant
    }

    /// Release a lease
    pub async fn release(&self, lease_id: &str) -> bool {
        let mut leases = self.leases.write().await;
        let mut found = false;
        let mut zone_to_remove: Option<String> = None;

        for (zone_id, grant) in leases.iter() {
            if grant.lease_id == lease_id {
                zone_to_remove = Some(zone_id.clone());
                found = true;
                break;
            }
        }
        if let Some(zone_id) = zone_to_remove {
            leases.remove(&zone_id);
            tracing::info!("[Lease] RELEASED {} zone={}", lease_id, zone_id);
        }
        found
    }

    /// Check if a lease is valid for a zone. fence_token, if provided, must
    /// match the zone's current fence token — a lexically valid, unexpired
    /// lease_token is not sufficient on its own (see module docs).
    pub async fn check(&self, lease_token: Option<&str>, zone_id: &str, fence_token: Option<u64>) -> (bool, String) {
        let leases = self.leases.read().await;
        let grant = leases.get(zone_id);

        if let Some(grant) = grant {
            if !grant.is_valid() {
                return (false, format!("Lease for zone={} has expired", zone_id));
            }
            if let Some(token) = lease_token {
                if token != &grant.lease_id {
                    return (false, format!("Lease token mismatch for zone={}", zone_id));
                }
            }
            if let Some(ft) = fence_token {
                if ft != grant.fence_token {
                    return (false, format!(
                        "Stale fence token for zone={}: presented {}, current is {}",
                        zone_id, ft, grant.fence_token
                    ));
                }
            }
            (true, String::new())
        } else {
            (false, format!("No lease held for zone={}", zone_id))
        }
    }

    /// Revoke a zone (safety event)
    pub async fn revoke_zone(&self, zone_id: &str) {
        let mut leases = self.leases.write().await;
        if leases.remove(zone_id).is_some() {
            tracing::warn!("[Lease] zone={} REVOKED (safety event)", zone_id);
        }
    }

    /// Get current leases
    pub async fn get_leases(&self) -> Vec<LeaseGrant> {
        let leases = self.leases.read().await;
        leases.values().cloned().collect()
    }

    /// Clean up expired leases
    pub async fn cleanup(&self) {
        let mut leases = self.leases.write().await;
        let now = Utc::now().timestamp() as f64;
        leases.retain(|_, grant| grant.expires_at > now);
    }
}

impl Default for LeaseManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lease_grant() {
        let mgr = LeaseManager::new();
        let req = LeaseRequest {
            robot_id: "robot1".to_string(),
            zone_id: "zone1".to_string(),
            duration_ms: 5000,
            bid_energy_j: 100.0,
            priority: 5,
        };

        let grant = mgr.request(&req).await;
        assert_eq!(grant.state, LeaseState::Active);
        assert_eq!(grant.robot_id, "robot1");
        assert!(grant.fence_token > 0);
    }

    #[tokio::test]
    async fn test_lease_deny() {
        let mgr = LeaseManager::new();

        // First robot gets the lease
        let req1 = LeaseRequest {
            robot_id: "robot1".to_string(),
            zone_id: "zone1".to_string(),
            duration_ms: 5000,
            bid_energy_j: 100.0,
            priority: 5,
        };
        mgr.request(&req1).await;

        // Second robot with lower bid is denied
        let req2 = LeaseRequest {
            robot_id: "robot2".to_string(),
            zone_id: "zone1".to_string(),
            duration_ms: 5000,
            bid_energy_j: 50.0,
            priority: 5,
        };
        let grant2 = mgr.request(&req2).await;
        assert_eq!(grant2.state, LeaseState::Denied);
    }

    #[tokio::test]
    async fn test_lease_outbid() {
        let mgr = LeaseManager::new();

        // First robot gets the lease
        let req1 = LeaseRequest {
            robot_id: "robot1".to_string(),
            zone_id: "zone1".to_string(),
            duration_ms: 5000,
            bid_energy_j: 100.0,
            priority: 5,
        };
        mgr.request(&req1).await;

        // Second robot with higher bid outbids
        let req2 = LeaseRequest {
            robot_id: "robot2".to_string(),
            zone_id: "zone1".to_string(),
            duration_ms: 5000,
            bid_energy_j: 150.0,
            priority: 5,
        };
        let grant2 = mgr.request(&req2).await;
        assert_eq!(grant2.state, LeaseState::Active);
        assert_eq!(grant2.robot_id, "robot2");
    }

    #[tokio::test]
    async fn test_lease_release() {
        let mgr = LeaseManager::new();
        let req = LeaseRequest {
            robot_id: "robot1".to_string(),
            zone_id: "zone1".to_string(),
            duration_ms: 5000,
            bid_energy_j: 100.0,
            priority: 5,
        };

        let grant = mgr.request(&req).await;
        let released = mgr.release(&grant.lease_id).await;
        assert!(released);

        // Check lease is gone
        let (valid, _) = mgr.check(None, "zone1", None).await;
        assert!(!valid);
    }

    #[tokio::test]
    async fn test_fence_token_monotonic_and_issued() {
        // Fence tokens are scoped per-zone (the guarantee is "this is the
        // current view of THIS zone's ownership", not a global ordering
        // across unrelated zones), so monotonicity is only a meaningful
        // property within the same zone.
        let mgr = LeaseManager::new();
        let req1 = LeaseRequest { robot_id: "r1".into(), zone_id: "z1".into(), duration_ms: 5000, bid_energy_j: 10.0, priority: 5 };
        let g1 = mgr.request(&req1).await;
        assert!(g1.fence_token > 0);

        // Different robot outbids for the SAME zone -> new grant, same
        // zone's counter continues incrementing.
        let req2 = LeaseRequest { robot_id: "r2".into(), zone_id: "z1".into(), duration_ms: 5000, bid_energy_j: 20.0, priority: 5 };
        let g2 = mgr.request(&req2).await;
        assert!(g2.fence_token > g1.fence_token);

        // A different, unrelated zone independently starts its own
        // per-zone counter at 1 -- this is correct, not a bug.
        let req3 = LeaseRequest { robot_id: "r3".into(), zone_id: "z2".into(), duration_ms: 5000, bid_energy_j: 10.0, priority: 5 };
        let g3 = mgr.request(&req3).await;
        assert!(g3.fence_token > 0);
    }

    #[tokio::test]
    async fn test_fence_token_bumped_on_renewal_and_rejects_stale() {
        // Kleppmann 2016: a renewal must invalidate a caller's stale view
        // of ownership, even though lease_id (and lexical validity) don't
        // change.
        let mgr = LeaseManager::new();
        let req = LeaseRequest { robot_id: "r1".into(), zone_id: "z1".into(), duration_ms: 5000, bid_energy_j: 10.0, priority: 5 };
        let g1 = mgr.request(&req).await;
        let stale_fence = g1.fence_token;

        let g2 = mgr.request(&req).await; // same robot, same zone -> renewal path
        assert_eq!(g1.lease_id, g2.lease_id);       // lease_id unchanged
        assert!(g2.fence_token > stale_fence);      // fence token bumped

        let (ok, _) = mgr.check(Some(&g2.lease_id), "z1", Some(g2.fence_token)).await;
        assert!(ok);

        let (ok, reason) = mgr.check(Some(&g2.lease_id), "z1", Some(stale_fence)).await;
        assert!(!ok);
        assert!(reason.contains("Stale fence token"));
    }
}
