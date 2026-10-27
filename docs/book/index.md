# Datamuta Operations

Capture a typed snapshot, review the mapping, publish resumable partitions and
verify the resulting PostgreSQL data.

Read [Capabilities and extension points](capabilities.md) for the relational and
document adapters, source/target directions, reusable Rust layers and their
current boundaries.

Begin with the [three-row example](example.md), then use
[Run and verify](operations.md) for your prepared tables. If an attempt fails,
consult [Recover a job](recovery.md) before retrying.

A request or process completing is not the entire acceptance decision. Confirm
native progress, require full verification, and review whether the approved
mapping expresses the intended business transformation.

The source distribution contains runtime code, ordinary unit tests and small
invented examples. Hosted services, private experiments and captured results are
not part of this repository. Read [Current limits](limits.md) before choosing a
workload.
