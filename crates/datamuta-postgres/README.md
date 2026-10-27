# Datamuta PostgreSQL adapter

Native discovery and typed mapping support for admitted PostgreSQL tables.
The library exposes disposable loopback and explicit verified TLS/SCRAM policies.
See [PostgreSQL connections](../../docs/book/connections.md) for the connection
contract and [the synthetic example](../../examples/postgres/README.md) for setup.

This crate also contains a bounded frozen-snapshot adapter used by library
consumers. Its `receipts`/`progress` metadata differs from the CLI streaming
executor's `stream_receipts`/`stream_cursor` metadata. Use the streaming example's
SQL for `datamuta-stream`; do not interchange the two layouts.

Build and test this standalone workspace with `cargo test --locked`.
