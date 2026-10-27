# Recover an interrupted CLI job

Retain the original plan, sealed spool, binary and native metadata. Determine
whether the old publisher or its native session is still active before admitting
a replacement. A lost response or local process exit is not evidence of rollback.

| Situation | Action |
| --- | --- |
| Configure failed | Correct endpoint, schema, mapping or output-path problems; use a fresh configuration output. |
| Capture failed or timed out | Preserve partial files; recapture to a new spool and new plan path. The source is needed again. |
| Capture sealed but plan saving failed | Preserve evidence; there is no supported CLI to reconstruct a plan from the spool alone. Use a new capture and plan. |
| Run failed, timed out or lost its response | Settle the old publisher, query native status and resume the same plan if status validates and progress is incomplete. |
| Progress is already complete | Proceed to full verification; do not recapture. |
| Verification failed or timed out | Diagnose the failure, preserve the report fragments and logs, and rerun verification to a fresh filename after correction. |
| Spool or native evidence is inconsistent, or the target was restored/rolled back | Stop blind retries and reconcile against a trusted recovery set. Database rollback/restore recovery is not qualified by this profile. |

## Resume publication

After the old publisher and its owned native session have settled:

```sh
datamuta-stream status --plan plan.json --spool snapshot
```

If status validates and `end_partition` is below the captured partition count:

```sh
datamuta-stream run --plan plan.json --spool snapshot --approved-plan-sha256 HASH
```

Use the original reviewed digest. Publication resumes from native committed
progress. Capture and verification have different recovery contracts: their scans
restart. Persistent errors or attempts that make no progress require diagnosis;
there is no automatic retry policy.

## Interpret refusals

| Message | Meaning |
| --- | --- |
| `exact stream plan approval required` | The supplied digest does not approve this plan. Reinspect the intended job. |
| `stream implementation/version changed` | The binary no longer matches the plan. Use its retained release; do not rewrite receipts. |
| `last atomic commit certificate missing or changed` | The cursor, previous receipt and authenticated metadata disagree. Preserve evidence. |
| `historical receipt missing or changed` | The older receipt prefix is inconsistent. Fast status cannot clear this failure. |
| `partition payload hash mismatch` / `partition Merkle proof mismatch` | The spool differs from the approved data. Never regenerate expected hashes to make it pass. |
| `data verification requires a fully published snapshot` | Publication is incomplete. Reconcile and resume the admitted job. |
| Destination value/identity mismatch, missing rows or extra rows | Readback differs from the approved transformation. Investigate data, mapping and external writes; verification does not repair rows. |
| `destination row exceeds verification wire budget` | The target contains a value exceeding the readback guard. The scan is refused, not verified. |
| `evidence output already exists` | Choose a fresh report filename and preserve earlier evidence. |

The CLI does not expose a live cancellation service in this distribution. Stopping
the CLI follows process-death recovery; earlier commits remain. Resolve owned
native sessions through the database operator and never terminate unrelated sessions.
A successful status query does not prove an old transaction cannot still commit.

## Credentials and capacity

Passwords are external environment references read on new connections. Host,
port, database, role, namespace and CA digest affect endpoint identity; changing
those fields is not a transparent repair of an active plan. Use the original
trusted connection policy rather than weakening TLS to bypass a failure.

On disk pressure, preserve the active spool and receipts. Free unrelated owned
space or expand capacity, then reconcile. Do not edit an approved budget or remove
completed partition files to force a run through. A failed capture still needs
new paths.

## Incident record

Record the job identifier, plan digest, binary hash, command, time, exit status,
stdout/stderr, native status and verification outcome. State whether capture had
sealed, which process stopped, what was observed about its native session, and
whether the destination received external writes. Preserve original evidence and
share only a separately redacted copy when needed.
