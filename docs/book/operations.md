# Run and verify a migration

This guide covers the standalone `datamuta-stream` CLI with PostgreSQL source and
destination tables. The separate resident, hosted services and private test gym
are outside this source distribution. Start with the
[synthetic PostgreSQL example](example.md) for a complete
local setup.

## Prepare a job

Build or install the CLI following the repository README. Retain the same release
for the job: its implementation fingerprint is part of the approved plan.
Create a fresh directory with `umask 077`; the captured spool contains source data.
Do not mix plans, spool files or native metadata from different jobs.

Prepare a nonempty source and an empty destination in the admitted flat schema.
Mappings support complete copies/renames and integer-to-bigint widening. Tables
with triggers, RLS, partitioning, defaults, generated columns or unsupported
constraints are refused. The CLI inserts records; it is not an upsert or merge.

Use separate source and target endpoint files, an explicit mapping and fresh
`stream_receipts`/`stream_cursor` tables as shown in the example. The older
frozen-snapshot library metadata is not interchangeable with streaming metadata.
See [connections](connections.md) for endpoint policy and privileges.

## Configure

```sh
datamuta-stream configure \
  --source-endpoint source-endpoint.json --source-table orders \
  --target-endpoint target-endpoint.json --target-table purchases \
  --mapping mapping.json --out config.json
```

This discovers native field schemas and validates the mapping. It does not grant
execution approval or guarantee that later metadata and publication admission will
succeed. Configuration output paths must be fresh.

The default spool budget is 256 MiB. `--spill-bytes N` accepts 8,192 through
4,294,967,296 bytes, including spool index/tree overhead. This is a logical
admission budget, not a disk reservation. Check available blocks and inodes locally,
and destination data, indexes and WAL capacity on the database hosts.

## Capture and inspect

Arrange a source write-blocking window:

```sh
datamuta-stream capture --config config.json --spool snapshot --plan plan.json
```

Continue only after a successful exit. An interrupted capture cannot be resumed;
use the [recovery guide](recovery.md) and new output paths. After success:

```sh
datamuta-stream inspect --plan plan.json --spool snapshot
```

Review the actual endpoints, mapping, captured row/partition counts and limits.
Record the reviewed `plan_sha256`. Inspection validates the plan and manifest
offline; it does not scan every payload. Once capture seals, execution does not
need the source database. Later source writes are not included.

## Publish and monitor

Replace `HASH` with the reviewed digest:

```sh
datamuta-stream run --plan plan.json --spool snapshot --approved-plan-sha256 HASH
```

Committed destination partitions become visible immediately. This is not an
atomic table replacement, and cancelling a process does not undo previous commits.
Use one designated publisher and prevent unrelated target writes or DDL until
verification finishes.

```sh
datamuta-stream status --plan plan.json --spool snapshot
```

`end_partition` counts the committed prefix; compare it with the plan's total
partition count. `partitions_applied` is zero in status and counts only the current
attempt in a run report. Progress by partition is not an exact row percentage.
Status authenticates the last commit, not every older receipt or destination value.
It also does not prove an old native session has stopped.

Commands have a 300-second asynchronous deadline. After a timeout or response loss,
settle the old attempt, inspect native status and explicitly resume the same plan.
Blocking filesystem operations are not hard real-time bounded. See
[current limits](limits.md) before selecting a workload.

## Verify and accept

After publication completes and the writer is stopped:

```sh
datamuta-stream verify --plan plan.json --spool snapshot --out verification-001.json
```

Require a zero exit code and a complete report with `status: "verified"`. Check
its plan digest and snapshot root against the reviewed plan, its verified row count
against capture, and its receipt-audit frontier against the full partition count.
The mere existence of a report file is not success; a write failure may leave a
partial file. Use a new output name for another attempt.

Verification audits all receipts, authenticates all spool payloads and compares
all transformed fields with PostgreSQL destination data in primary-key order.
Missing, extra or changed rows fail. SHARE locks block destination and metadata
writes during this explicit readback operation. Verification restarts its scan
if interrupted; it has no resumable audit cursor.

`audit` checks only receipt history. A successful execution or receipt audit does
not replace full verification. Verification establishes agreement with the approved
snapshot at the recorded native snapshot; the operator still reviews whether the
mapping expresses the intended business transformation.

## Retain the recovery set

Keep the matching binary and its hash, endpoint/configuration references, mapping,
plan, spool, command logs and verification report under the job's retention policy.
Document the source boundary, target, owner and application acceptance separately.
No command automatically performs application cutover or retires the source.

Do not delete completed partition files while recovery or verification is needed.
Do not reset native metadata to reuse a domain. There is no automatic spool
retirement policy. Redact credentials and source data before sharing diagnostics.

## Suggesting a mapping (optional)

When two PostgreSQL relations admit more than one structurally valid mapping, for
example two `bigint` amounts, `suggest-mapping` can draft one through a saved JEV
exchange. Write a short meaning for every source and target field in an evidence
file; field names alone are not evidence.

```json
{"source_meanings": {"order_id": "Order identifier", "cents": "Grand total in US cents", "tax_cents": "Sales tax in US cents"},
 "target_meanings": {"order_key": "Order identifier", "tax": "Sales tax in US cents", "total": "Grand total in US cents"},
 "context": "Billing migration"}
```

```sh
datamuta-stream suggest-mapping prepare \
  --source-endpoint src.json --source-table billing \
  --target-endpoint tgt.json --target-table invoices \
  --evidence evidence.json --request-out request.json --mapping-out mapping.json
# send request.json to JEV unchanged and save the reply as response.json
datamuta-stream suggest-mapping admit \
  --source-endpoint src.json --source-table billing \
  --target-endpoint tgt.json --target-table invoices \
  --evidence evidence.json --request request.json --response response.json \
  --mapping-out mapping.json --provenance-out mapping.provenance.json
```

To let Datamuta make the call, put your TypeSafe key in `DATAMUTA_JEV_API_KEY` and
use `ask`, which prepares, posts once to `https://api.typesafe.ai/v1/systemone`
(override with `--endpoint`; HTTPS only), saves the request and reply, and admits:

```sh
export DATAMUTA_JEV_API_KEY=...   # never stored by Datamuta
datamuta-stream suggest-mapping ask \
  --source-endpoint src.json --source-table billing \
  --target-endpoint tgt.json --target-table invoices \
  --evidence evidence.json --request-out request.json --response-out response.json \
  --mapping-out mapping.json --provenance-out mapping.provenance.json
```

`ask` never retries; a failed or refused call is reported and a new attempt is your
decision. If only one mapping is valid, `prepare` and `ask` write it to `--mapping-out` and no
request is sent. `admit` refuses a request whose schemas or evidence have changed, a
response from another model, malformed or oversized responses, and existing output
paths. A low-confidence or abstaining reply exits with status 3, writes no mapping
and prints the candidates. An accepted mapping is a draft: review it, then continue
with `configure`, `capture`, `inspect` and exact-hash approval as usual.

