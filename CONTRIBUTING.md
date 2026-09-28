# Contributing to pmcp-rust

Two independent crates: `pmcp-core` (protocol core) and `pmcp-ledger`
(CRDT ledger layer).

The organization-wide contributor policy lives in
[`physicalcontextprotocol/.github`](https://github.com/physicalcontextprotocol/.github/blob/main/CONTRIBUTING.md).
This file covers what is specific to this repository.

## Running the checks

```bash
cd pmcp-core
cargo build --release
cargo test                      # 43 unit tests, all passing
```

`pmcp-ledger` **does not currently compile.** See below.

## pmcp-core status

Real code, builds clean, `cargo test` is green. The `pyo3`
misconfiguration that used to break a plain `cargo build` has been
fixed: `pyo3` is now an optional dependency behind a `python` feature, so
`src/python.rs` only compiles when a Python-extension build is actually
requested. (The repository README still describes the old broken state
in places — if you fix that, thank you.)

Note that `crate-type` includes `cdylib` as well as `rlib`. The
`cdylib` is only needed for the `python` feature; `rlib` covers every
other consumer.

## pmcp-ledger status — 19 compile errors

`pmcp-ledger` fails to build. This is a real, known, and currently
unfixed problem, and CI treats the ledger build as **non-blocking** for
exactly that reason.

One error was a straightforward typo and has been fixed in this pass:
`Cargo.toml` requested the redis feature `scripting`, which does not
exist in redis 0.25 (it is `script`). Cargo refused to resolve the
dependency at all, so nothing compiled. What remains are 19 type errors
in the CRDT layer:

| Count | Error |
|---|---|
| 5 | `E0599: no method named 'increment' found for type 'u64'` |
| 4 | `E0308: mismatched types` |
| 2 | `E0599: no associated function or constant named 'default' for 'ZoneBounds'` |
| 2 | `E0277: the trait bound 'RobotState: Default' is not satisfied` |
| 1 | `E0728: 'await' is only allowed inside 'async'` |
| 1 | `E0610: 'u64' is a primitive type and therefore doesn't have fields` |
| 1 | `E0603: struct import 'RobotState' is private` |
| 1 | `E0599: no method named 'merge' found for type 'u64'` |
| 1 | `E0599: no method named 'happens_before' found for type 'u64'` |
| 1 | `E0277: the trait bound 'NodeId: Default' is not satisfied` |

The shape of these suggests stale field renames in `state.rs` / `crdt.rs`
(both `pmcp-core` and `ledger` define a `RobotState`, and they have
drifted) plus a handful of missing `Default` derives. That is a
self-contained refactor and a genuinely useful contribution.

**Getting `pmcp-ledger` to compile is the highest-value open
contribution in this repository.** When you do, flip the ledger build in
`.github/workflows/ci.yml` from `continue-on-error: true` to blocking,
and say so in the PR.

## Open structural question

`pmcp-core` and `ledger` are two independent crates with independent
versions and no workspace `Cargo.toml`. Whether they should become
members of one Cargo workspace is still undecided. A PR proposing
either answer — with reasoning — is welcome.
