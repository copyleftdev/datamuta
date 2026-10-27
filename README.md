# Datamuta

Datamuta is an experimental Rust database migration runtime. It captures a typed
snapshot, applies an explicitly reviewed mapping, resumes from native commit
receipts, and verifies destination values against the captured data.

The runtime separates typed planning from native publication, so the same sealed
snapshot workflow can serve relational and flat document targets. PostgreSQL is
the complete documented path; MySQL, MongoDB and Firestore are implemented lab
profiles. Firestore also has a source reader.

| Capability | Available today |
| --- | --- |
| Capture sources | PostgreSQL; Firestore emulator in the lab profile. |
| Publication targets | PostgreSQL; laboratory MySQL, MongoDB and Firestore adapters. |
| Transformations | Complete field copies/renames, integer widening, explicit null and identity preservation. |
| Recovery | Detached disk snapshots, exact-plan approval and native partition receipts/progress. |
| Verification | Shared receipt audit; full destination-value comparison for PostgreSQL. |
| Extension | Rust mapping libraries, generic kernel units/targets and compiled streaming adapters. |

Read [Capabilities and extension points](docs/book/capabilities.md) for engine
directions, actual data transformations, JEV selection boundaries and the work
required to add an adapter. Bidirectional snapshot routes do not imply live
synchronization; this release does not support arbitrary databases or schemas.

Start with the [PostgreSQL example](examples/postgres/README.md), then read the
[operations book](docs/book/operations.md) and [recovery runbook](docs/book/recovery.md).

## Build

Requires Rust 1.99.0. Each crate is a standalone Cargo workspace:

```sh
cargo build --release --locked --manifest-path crates/datamuta-stream/Cargo.toml
```

The default binary path is `crates/datamuta-stream/target/release/datamuta-stream`.
If `CARGO_TARGET_DIR` is set, use that directory instead. For a stable install path:

```sh
cargo install --locked --path crates/datamuta-stream --root .local/datamuta
export PATH="$PWD/.local/datamuta/bin:$PATH"
```

## Documentation

```sh
cargo install mdbook --version 0.5.2 --locked
mdbook build
mdbook serve --hostname 127.0.0.1 --port 3010
```

The book is standalone: it requires no experiment archive, captured data or private
preprocessor. Build output stays in `.local/book/`.

## Current boundary

The documented path is detached PostgreSQL → PostgreSQL migration using prepared,
flat tables. Source writes are blocked during capture. Destination partitions
become visible as they commit; this is not a single atomic table replacement.
There is no automatic CDC, application cutover or rollback. PostgreSQL full-value
verification is explicit and blocks destination writes while reading back data.

The runtime is experimental. The current 4 GiB spool admission ceiling is not a
production capacity guarantee. The repository contains implementation, ordinary
unit tests and synthetic examples; private experiments and their outputs are
not required to use or build it.
