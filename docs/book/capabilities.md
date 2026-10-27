# Capabilities and extension points

Datamuta separates the meaning of a migration from the database mechanism that
commits it. A typed mapping describes the intended transformation; an approved
plan binds that mapping to endpoints, implementation and snapshot; a native
adapter publishes bounded partitions with durable progress. This lets several
engines share the same inspection, approval and recovery workflow.

The distribution contains an experimental runtime, not an any-database support
guarantee. The PostgreSQL example is the complete documented operator path.
Other adapters are implemented laboratory profiles with additional setup and
verification work. None of these profiles establishes production qualification.

## Engines and directions

| Engine | Capture source | Publication target | Connection and data boundary |
| --- | --- | --- | --- |
| PostgreSQL | Yes | Yes | Admitted PostgreSQL 17 flat tables; disposable loopback or explicit verified TLS/SCRAM profile. |
| MySQL | No | Yes, laboratory | Loopback MySQL 8.4; restricted InnoDB tables and disposable authentication. |
| MongoDB | No | Yes, laboratory | Loopback MongoDB 8.0 replica set; strict flat-document validators and transactional publication. |
| Firestore | Yes, laboratory | Yes, laboratory | Local emulator, fixed demo project, selected flat collection and prescribed document identities. |
| Parquet, other columnar formats, Firebase Realtime Database, other engines | No | No | No adapter for these systems is included in this CLI. |

The engine names in configuration are `postgres`, `mysql`, `mongo`, and
`firestore`. `source_engine` accepts only `postgres` or `firestore`.
`target_engine` accepts all four. Engine dispatch is compiled into Rust; these
are not dynamically loaded plugins.

PostgreSQL can therefore feed relational or flat document targets through the
shared publisher. Firestore can also supply the typed snapshot, making
Firestore → PostgreSQL and PostgreSQL → Firestore distinct migration directions.
Having two directions does **not** provide simultaneous synchronization, conflict
resolution, change capture, or automatic round-trip fidelity. Each direction
needs its own admitted schemas, mapping, metadata domain, snapshot and approval.
Available source and target enums do not qualify every possible pairing.

`configure` discovers PostgreSQL schemas at both ends. For another engine, an
integrator must construct the version-2 `Config` with explicit typed schemas,
mapping, endpoints and limits, and provision the adapter's expected native
metadata. There is no included turnkey setup example for those lab profiles.
Once configured and admitted, they use `capture`, `inspect`, `run`, `status`,
and `audit`. The CLI's full-value `verify` accepts **PostgreSQL destinations**;
other targets need their own native value comparator. Receipt audit alone is
not a substitute.

## What happens to actual data

The shared flat contract carries booleans, signed 32-bit and 64-bit integers,
UTF-8 text, and explicit nullable values. It supports complete field renaming
and lossless `i32` → `i64` widening. Single and composite integer identities
preserve component order. It rejects missing or duplicate assignments, nullable
values mapped into required fields, unsupported conversions and identity drift.

For example, the [included migration](example.md) expresses this contract:

| Source field | Target field | Meaning preserved |
| --- | --- | --- |
| `order_id: i32` | `order_key: i64` | Same integer identity, widened without rounding. |
| `cents: i64` | `value: i64` | Same signed amount, including negative and zero values. |
| `note: utf8?` | `memo: utf8?` | Text remains text; null remains null. |
| `active: bool` | `enabled: bool` | Same boolean value. |

The same typed record can be represented by SQL columns or flat document fields.
MongoDB adds its adapter-defined `_id` from the ordered integer key tuple;
Firestore uses a prescribed encoding for the document name. Those identity rules
do not accept arbitrary existing document IDs. Nested objects, joins,
denormalization, decimals, timestamps, arrays, binary values, filtering, field
dropping and business-unit conversion are outside this mapping contract.

## What stays consistent across adapters

1. **Inspect before effects.** The plan captures source and target contracts,
   mapping, endpoint and engine identities, implementation fingerprint and sealed
   snapshot identity. Publication requires approval of that exact plan hash.
2. **Separate capture from publication.** After successful capture, publication
   reads the sealed disk spool and target; the original source need not stay
   connected. Capture itself has source-specific consistency requirements.
3. **Bound each unit of work.** The stream runtime reads bounded partitions with
   authenticated metadata. Limits are explicit; total spool capacity is still
   capped. See [Current limits](limits.md).
4. **Recover from native evidence.** Each streaming adapter commits partition
   effects, its receipt and cursor together. After an uncertain outcome, a new
   invocation reconciles native progress rather than blindly repeating writes.
5. **Separate progress from correctness.** Fast status/resume checks the last
   commit. `audit` checks receipt history. Full native value comparison checks
   what is actually stored. These answer different questions.

PostgreSQL and MySQL use database transactions; MongoDB uses replica-set
transactions; Firestore uses an atomic Commit with a checkpoint precondition.
The adapter must satisfy the shared contract through its own storage semantics.
A descriptor or a successful API call alone does not prove those semantics.

## Reusable Rust layers

| Component | Reusable capability | Boundary |
| --- | --- | --- |
| `datamuta-plan` | Typed schema validation, deterministic mapping compilation, bounded candidate enumeration and JEV choice-response admission. | The CLI uses explicit mappings. The library does not call JEV or provide a hosted planner. |
| `datamuta-kernel` | Custom `MigrationUnit` types, target capability descriptors, plan-bound receipts and checkpointed execution through `Target<U>`. | This executor validates a bounded materialized snapshot; it is not the disk-streaming executor. |
| `datamuta-postgres` | Native schema discovery, connection policy and PostgreSQL admission checks. | Admits a deliberately restricted schema and server profile. |
| `datamuta-stream` | Disk capture, sealed partition metadata, exact-plan approval, publication/resume and adapter dispatch. | Currently a single-job partition protocol, not a distributed scheduler. |

Candidate enumeration can help a planner compare structurally valid mappings;
field meanings still require explicit evidence and review. Structural validity
does not establish that two fields have the same business meaning. The JEV
selection library can refuse an ambiguous response; it is not automatic semantic
discovery in the shipped CLI.

The lower-level kernel allows application-defined units beyond flat rows, provided
they supply deterministic bytes and contiguous ordinals. That extension point does
not give the streaming CLI nested-document or file-format support automatically.
Its `Target<U>` trait and the stream publisher are separate integration surfaces.

## Adding an engine

A new streaming adapter needs source capture and/or target publication behavior,
explicit endpoint and schema admission, deterministic identity handling, and
bounded encoding. A target must atomically bind effects to durable receipts and
progress, and resolve uncertain commits through that native evidence. Wire it
into configuration and dispatch, preserve implementation fingerprinting, and add
native value verification and an operator example.

Qualification should exercise loss of the source after capture, process death
on both sides of commit, completed replay, competing publishers, schema drift,
spool corruption and deliberate destination-value corruption. The last probe
must fail value verification even when receipt history is intact. Ordinary unit
tests or reuse of the shared publisher cannot qualify a new storage engine alone.

## Where the broader vision still needs work

Universal source discovery, arbitrary schema transformations, native columnar
adapters, live bidirectional synchronization, automatic quarantine/reprocessing,
application cutover, rollback and distributed petabyte scheduling are outside
this distribution. Invalid input is refused; there is no operator quarantine
queue in the CLI. Bounded memory and resumable publication are useful foundations,
but do not establish petabyte readiness.

The standalone runtime requires no hosted account or billing integration.
Resident services, hosted planning, billing integrations and the private test gym
are not shipped here. They should not be inferred from the library extension
points or the project's longer-term goal.
