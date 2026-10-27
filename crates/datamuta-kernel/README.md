# Datamuta kernel

A Rust library for bounded migration plans, capability admission, deterministic
unit identity, receipt checks and checkpointed execution. It performs no database
connection or billing operation. Adapter implementations supply native behavior.

Build and test this standalone workspace with `cargo test --locked`.
The user-facing snapshot workflow is documented in the
[operations book](../../docs/book/operations.md).
