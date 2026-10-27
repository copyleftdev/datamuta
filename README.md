# Datamuta

Datamuta is an experimental Rust database migration runtime. It captures a typed
snapshot, applies an explicitly reviewed mapping, resumes from native commit
receipts, and verifies destination values against the captured data.

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
