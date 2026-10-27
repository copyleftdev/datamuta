# Current support and limits

The documented operator path is detached PostgreSQL → PostgreSQL migration.
MySQL, MongoDB and Firestore adapter modules are present in the source but remain
laboratory profiles; their presence is not a production-support promise.
`configure` and full destination `verify` currently cover PostgreSQL.

See [Capabilities and extension points](capabilities.md) for source and target
directions, the shared adapter contract and the distinction between library
extension points and runnable CLI features.

| Area | Current contract |
| --- | --- |
| Source capture | Nonempty flat snapshot. PostgreSQL blocks source writes until capture finishes; the Firestore lab reader pins a read time and requires structural changes to stay quiescent. |
| Mappings | Complete field copies/renames and lossless integer-to-bigint widening. |
| Source wire | Bounded pages and a server-side oversized-value guard. |
| Encoded record | At most 65,536 bytes, excluding framing newline. |
| Partition | At most 256 rows and 1 MiB encoded bytes; default byte target 128 KiB. |
| Spool | Default 256 MiB; configured range 8 KiB through 4 GiB, including index/tree overhead. No space reservation. |
| Partition count | At most 65,536. |
| Commit | One partition's effects, receipt and progress commit atomically. |
| Recovery | Last-commit validation and authenticated metadata; no completed payload-prefix scan for publication resume. |
| Receipt audit | Checks the completed historical receipt prefix; does not compare destination values. |
| Full verification | PostgreSQL value comparison against transformed authenticated spool; blocks target writes during its scan. |
| Deadline | 300 seconds per CLI command; not a hard real-time filesystem guarantee. |

Capture interrupted before completion starts over. Full verification starts over
if interrupted. Publication resumes from native progress. The tool does not
perform CDC, automatic cutover, rollback, native metadata repair or automatic
spool retirement. Arbitrary database rollback, malicious metadata edits, cloud
failover and petabyte operation are not qualified.

Private experiments and datasets are intentionally not distributed. This book
states implementation boundaries without requiring private benchmark artifacts.
Use the included synthetic example to check the documented workflow locally.
