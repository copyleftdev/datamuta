# Datamuta streaming runtime

Build this standalone Cargo workspace with `cargo build --release --locked`.
The CLI supports schema configuration, sealed capture, offline plan inspection,
approved publication/resume, native status, receipt auditing and explicit
PostgreSQL destination-value verification.

The [capability guide](../../docs/book/capabilities.md) explains engine directions,
data transformations, shared recovery behavior and adapter extension requirements.

Start with the [PostgreSQL example](../../examples/postgres/README.md) and the
[operations guide](../../docs/book/operations.md). Read the
[recovery contract](../../docs/book/recovery.md) before retrying an uncertain attempt.

One bounded partition commits its effects, receipt and cursor atomically. The
sealed spool retains authenticated random-access partition metadata. Fast resume
checks the last native receipt without scanning completed payloads; explicit
historical receipt audit and full-value verification provide separate checks.

The crate includes laboratory MySQL, MongoDB and Firestore adapters. These do not
expand the documented production support envelope; see
[current limits](../../docs/book/limits.md). `verify` currently supports PostgreSQL
only. No private gym, dataset or hosted service is needed to build this crate.

Run ordinary tests with `cargo test --locked`. The public distribution includes
unit/conformance tests; private model campaigns and native experiment harnesses
are not bundled.
