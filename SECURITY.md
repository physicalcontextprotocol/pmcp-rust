# Security policy — pmcp-rust

The default policy for this organization lives in
[`pmcp-spec/SECURITY.md`](https://github.com/physicalcontextprotocol/pmcp-spec/blob/main/SECURITY.md)
and applies here in full. This file records what is specific to the
Rust crates.

## Reporting

Use **private vulnerability reporting**:
**Security → Report a vulnerability** on this repository, or
[open an org-level advisory](https://github.com/physicalcontextprotocol/security/advisories/new).

Do not open a public issue.

## In scope here

- Fencing-token or lease-staleness logic that accepts a stale token, in
  `pmcp-core/src/lease.rs` or `pmcp-core/src/consensus.rs`.
- Signature verification, `did:document`-style identity creation, or
  auth-token handling in `pmcp-core/src/security.rs` or
  `pmcp-core/src/identity.rs` that accepts a forged or replayed
  credential.
- A gate in the actuation path that can be bypassed.
- Memory-safety issues in the `pmcp-core` crate.
- Leaked secrets or credentials in this repository.

## Out of scope here

- `pmcp-ledger` does not currently compile (19 pre-existing type
  errors in the CRDT layer). Please report build breakage in
  `pmcp-ledger` as an issue, not a security advisory.
- Missing features relative to the specification. `pmcp-core` does not
  implement every part of the v0.5 spec; its README is explicit about
  its current scope.
- Dependency CVEs in transitive crates. Report those to the registry.

## Supported

`pmcp-core` v0.5 line, best-effort. `pmcp-ledger` is not supported
and not compiled by CI's blocking jobs.
