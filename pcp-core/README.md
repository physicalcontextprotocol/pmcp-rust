# physicalcontextprotocol

Rust implementation of the **Physical Context Protocol** (PCP) — an
MCP-compatible standard protocol for robots and industrial systems.

Part of the [`physicalcontextprotocol`](https://github.com/physicalcontextprotocol)
organization. Previously published as `pcp-core`; renamed for consistency with
the Python (`physicalcontextprotocol` on PyPI) and TypeScript
(`physicalcontextprotocol` on npm) SDKs. Nothing was ever released under the
old name, so no compatibility shim is provided.

## Install

```bash
cargo add physicalcontextprotocol
```

## What the crate exports

```rust
use physicalcontextprotocol::{
    // server
    PCPServer,
    // transports
    Transport, StdioTransport, TcpServerTransport, TcpClientTransport, ConnectionPool,
    // consensus
    RaftNode, RaftConfig, FleetStateMachine, LogCommand, ConsensusCluster,
    // safety
    ActuationRateLimiter, EnergyBudgetTracker, PriorityScheduler,
    // identity
    Did, DidDocument, RobotIdentity, CapabilityToken, DidRegistry, ZkSafetyProver,
    // observability
    TelemetryPipeline, MetricsRegistry, EventLog, StructuredEvent,
    // errors
    PcpError, PcpErrorCode,
};
```

The crate provides a **server** implementation. For client-side work, use
`TcpClientTransport` (which implements the `Transport` trait) together with
`ConnectionPool`. There is no high-level `PCPClient` type.

## Optional features

- `python` — pyo3 bindings, producing a Python extension module. Building this
  feature requires the extension-module environment; the pure-Rust default
  build does not.

```bash
cargo build                 # pure Rust, supported path for SDK use
cargo build --features python
```

## Test

```bash
cargo test
```

## License

Apache-2.0. See [LICENSE](https://github.com/physicalcontextprotocol/pcp-rust/blob/main/LICENSE).

## Note on the sibling crate

`physicalcontextprotocol-ledger` (the 4D-CRDT digital-twin ledger) lives in the
same repository but is **not published**. It currently does not compile and is
versioned and released independently of this crate.
