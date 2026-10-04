# Changelog — pmcp-rust

All notable changes to the Rust crates. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Two crates with independent versions: `physicalcontextprotocol` (protocol core) and
`physicalcontextprotocol-ledger` (CRDT ledger layer).

## [Unreleased]

### Changed
- **Protocol renamed to PCP.** The acronym is now **PCP** and the expanded
  name is **Physical Context Protocol** (was "Physical Model Context
  Protocol"). Public types follow: `PMCPServer` -> `PCPServer`,
  `PMCPServerBuilder` -> `PCPServerBuilder`, `PmcpError` -> `PcpError`,
  `PmcpErrorCode` -> `PcpErrorCode`. The JSON-RPC method prefix moves from
  `pmcp/` to `pcp/` (`pcp/estop`, `pcp/status`, `pcp/metrics`, ...), matching
  the Python and TypeScript SDKs. The PyO3 extension module is now
  `pcp_core` (was `pmcp_core`).
- **Crates renamed.** `pmcp-core` -> `physicalcontextprotocol`,
  `pmcp-ledger` -> `physicalcontextprotocol-ledger`. The library target is
  now `physicalcontextprotocol` (was `pmcp_core`), so imports become
  `use physicalcontextprotocol::...`. The ledger binary is still invoked as
  `pmcp-ledger`. Nothing was published under the old names, so no alias or
  deprecation shim is needed.

## [1.0.0] — 2026-09-28

First tagged public release. `pmcp-core` builds clean and its 43 unit
tests pass. `pmcp-ledger` does not compile.

### Fixed

- **`pyo3` misconfiguration in `pmcp-core/Cargo.toml`.** `pyo3` was
  declared as a top-level dependency with
  `features = ["extension-module"]`, but `src/lib.rs` gates
  `pub mod python;` behind `#[cfg(feature = "python")]` and the
  features table declared only `default = ["std"]` and `std = []`. So
  `python.rs` (~13 KB) was compiled but never linked in, and
  `extension-module` demanded a Python-interpreter link environment
  that a plain `cargo build` on a CI runner does not have. `pyo3` is now
  optional behind a proper `python = ["dep:pyo3", "pyo3/extension-module"]`
  feature. `cargo build --release` succeeds with no interpreter
  present.
- **Broken async `Clone` in the lease path** that was silently
  corrupting shared lease state across concurrent connections.
- **A missing lease-enforcement check in the actuation-call path
  itself** — the check existed elsewhere in the flow but not where the
  actuation actually happened.
- **`redis` feature typo in `pmcp-ledger/Cargo.toml`.** It requested
  `scripting`; the feature in redis 0.25 is `script`. Cargo refused to
  resolve the dependency at all, so the crate could not even reach the
  compilation stage. Fixed, and the crate now gets far enough to
  surface its real errors.

### Known limitations (documented, not fixed)

- **`pmcp-ledger` still does not compile — 19 type errors remain** in
  the CRDT layer after the dependency fix:
  5 × `no method 'increment' on u64`, 4 × mismatched types, 2 ×
  `no 'default' for ZoneBounds`, 2 × `RobotState: Default`
  unsatisfied, 1 × `await` outside async, 1 × `u64` primitive field
  access, 1 × private `RobotState` import, 1 × `no method 'merge' on
  u64`, 1 × `no method 'happens_before' on u64`, 1 × `NodeId: Default`
  unsatisfied. The pattern suggests `RobotState` has drifted between
  this crate and `pmcp-core` plus some missing `Default` derives. The
  ledger build is therefore **non-blocking in CI** — deliberately, and
  loudly commented as such rather than hidden behind `|| true`.
- **No workspace `Cargo.toml`.** `pmcp-core` and `ledger` are two
  independent crates with independent versions. Whether they become
  workspace members is still undecided.
- **`Cargo.lock` is committed in `pmcp-core` but not in `ledger`.**
  Normal for a library crate, but the asymmetry is worth resolving.
- **No integration test directories**, only embedded `#[cfg(test)]`
  modules.

### Changed

- Moved out of the monorepo into its own repository so the Rust SDK
  reads as a peer of the Python and TypeScript SDKs rather than a
  binding.
