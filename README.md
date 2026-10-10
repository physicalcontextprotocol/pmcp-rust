# pcp-rust

Rust implementation of the PCP protocol, split across two
independent crates:

- `pcp-core/` — protocol core: consensus (Raft), identity, safety,
  transport, networking, plugin system, rate limiting, telemetry, and
  optional Python bindings (`python.rs`).
- `ledger/` (`pcp-ledger`) — CRDT ledger layer: state, sync,
  partitioning, storage, RPC/API surface.

This is a **peer** implementation of the same wire format as
`pcp-python` and `pcp-typescript`, not a binding of either.

## Install

The core crate is published on crates.io as **`physicalcontextprotocol`**
(note: the package name differs from the `pcp-core/` directory name):

```bash
cargo add physicalcontextprotocol
```

The ledger crate is **not** published — it does not compile (see status below).

## Status at a glance

| Crate | Builds? | Tests | Supported? |
|---|---|---|---|
| `pcp-core` | yes | **43 pass, 0 fail** | best-effort |
| `pcp-ledger` | **no** — 19 compile errors | not runnable | no |

Both crates contain real implementations, not stubs. `pcp-core/src/lib.rs`
declares 18 public modules; `ledger/src/lib.rs` declares 6. Combined
that is roughly 350 KB of Rust source with ~66 embedded `#[test]`
unit tests.

## pcp-core — builds and tests clean

```bash
cd pcp-core
cargo build --release
cargo test                    # 43 passed; 0 failed
```

### The pyo3 misconfiguration is fixed

Earlier revisions of this README described a build break here, and
other documents in the project repeated it. It no longer applies, and
this section is kept so the stale claims elsewhere can be traced.

`pyo3` used to be a **top-level dependency** carrying
`features = ["extension-module"]`, while `src/lib.rs` gates
`pub mod python;` behind `#[cfg(feature = "python")]` and the features
table declared only `default = ["std"]` and `std = []`. So there was no
`python` feature, `python.rs` (~13 KB) was compiled but never linked
in, and `extension-module` demanded a Python-interpreter link
environment a plain `cargo build` does not have.

`pyo3` is now optional, behind a real feature:

```toml
[dependencies.pyo3]
version = "0.20"
optional = true

[features]
default = ["std"]
std = []
python = ["dep:pyo3", "pyo3/extension-module"]
```

A plain `cargo build` needs no interpreter, and `src/python.rs` only
compiles when a Python-extension build is actually requested. CI
enforces this: the `core-build` job greps `Cargo.toml` and fails if
`pyo3` is ever reintroduced as a non-optional dependency.

`crate-type = ["cdylib", "rlib"]` is fine — the `cdylib` only matters
for the `python` feature, and `rlib` covers every other consumer.

## pcp-ledger — does not compile

`cargo build` fails with 19 type errors in the CRDT layer:

| Count | Error |
|---|---|
| 5 | `E0599`: no method `increment` on `u64` |
| 4 | `E0308`: mismatched types |
| 2 | `E0599`: no `default` for `ZoneBounds` |
| 2 | `E0277`: `RobotState: Default` not satisfied |
| 1 | `E0728`: `await` outside an async function |
| 1 | `E0610`: `u64` is a primitive and has no fields |
| 1 | `E0603`: `RobotState` import is private |
| 1 | `E0599`: no method `merge` on `u64` |
| 1 | `E0599`: no method `happens_before` on `u64` |
| 1 | `E0277`: `NodeId: Default` not satisfied |

One prerequisite was fixed in this pass: `Cargo.toml` requested the
redis feature `scripting`, which does not exist in redis 0.25 (it is
`script`). Cargo refused to resolve the dependency at all, so the crate
never reached compilation. The pattern above suggests stale field
renames in `state.rs` / `crdt.rs` — both crates define a `RobotState`
and they have drifted — plus some missing `Default` derives.

CI treats the ledger build as **non-blocking**, with the failure
declared and explained rather than hidden behind `|| true`.

## Open structural questions

- **No workspace `Cargo.toml`.** `pcp-core` and `ledger` are two
  independent crates with independent versions and independent
  `Cargo.lock` conventions. Whether they should become members of one
  Cargo workspace is undecided.
- **`Cargo.lock` is committed in `pcp-core` but not in `ledger`.** That
  is the usual convention for library crates, but the asymmetry is
  worth resolving deliberately.
- **No integration test directories** in either crate, only embedded
  `#[cfg(test)]` modules.

## License

Apache 2.0 (see [`LICENSE`](LICENSE)).
