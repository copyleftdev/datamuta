# Datamuta plan compiler

A deterministic Rust compiler for admitted flat schemas and explicit field
mappings. Supported transformations are complete copies/renames and lossless
integer widening; unsupported conversions are refused. Compilation checks field,
identity, type, nullability and size constraints before producing mapped units.

This library does not discover a database or send data to a hosted service.
The command-line `configure` operation supplies native schemas through the
PostgreSQL adapter. Build and test with `cargo test --locked`.
