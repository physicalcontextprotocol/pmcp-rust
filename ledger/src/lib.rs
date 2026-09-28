pub mod crdt;
pub mod state;
pub mod storage;
pub mod partition;
pub mod sync;
pub mod api;

pub use crdt::{RobotStateCRDT, DeltaMutator, VersionVector};
pub use state::{RobotState, ZoneState, FleetSnapshot};
pub use storage::{StorageBackend, RedisBackend, EtcdBackend, InMemoryBackend};
pub use partition::{ZonePartition, PartitionKey};
pub use sync::{SyncProtocol, GossipProtocol};
pub use api::{LedgerAPI, LedgerConfig};