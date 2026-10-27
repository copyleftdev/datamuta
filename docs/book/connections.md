# PostgreSQL connections

Namespaces start with `dm_`, use lowercase ASCII letters, digits or underscores,
and fit 48 bytes. Source and target endpoints are configured independently.

## Disposable loopback example

```json
{"port":55432,"namespace":"dm_source","lab_only":true}
```

This connects to numeric loopback, database/user `datamuta`, with TLS disabled.
It is only appropriate for disposable local instances. The adapter does not prove
that an endpoint is disposable. The bundled Compose example binds only loopback.

## Verified transport

```json
{
  "port":5432,
  "namespace":"dm_source",
  "lab_only":false,
  "verified":{
    "host":"db.example.com",
    "database":"orders",
    "user":"migration_worker",
    "ca_der":"/absolute/path/organization-ca.der",
    "ca_sha256":"0000000000000000000000000000000000000000000000000000000000000000",
    "password_env":"DATAMUTA_SOURCE_PASSWORD"
  }
}
```

Replace all example values, including the zero digest, before use. The DER file
contains one explicit trust anchor, at most 64 KiB. Its SHA-256 must match the
configured digest. Certificate and hostname verification and SCRAM channel binding
are required. This policy does not silently downgrade to plaintext or trust auth.

The password environment name must start with `DATAMUTA_`. Supply a nonempty UTF-8
password through the process environment; do not store it in endpoint JSON. Password
values, variable names and CA file locations are excluded from endpoint identity;
host, port, database, role, namespace and CA digest are included. A matching endpoint
is not proof that a restored server has the same native history.

Verified admission requires PostgreSQL major 17, a writable primary, fsync and
full-page writes, synchronous commit, TLS and a non-superuser/non-BYPASSRLS role.
The profile remains experimental; these checks do not certify storage durability
or failover behavior. Connection setup has a ten-second async deadline, statements
ten seconds and lock waits eight seconds.

## Provisioning and privileges

The operator provisions source and destination tables and a fresh streaming
metadata domain. See the executable example's `source.sql` and `target.sql`.
Use `stream_receipts` and `stream_cursor`, not the separate frozen-snapshot library's
`receipts` and `progress` tables.

Allow database CONNECT and schema USAGE, source SELECT/UPDATE, and
SELECT/INSERT/UPDATE on the target and both streaming metadata tables. Source
UPDATE privilege permits the SHARE lock; capture does not change source records.
No migration command creates tables or grants privileges.

The admitted data model is flat boolean, integer, bigint and text fields, with
supported primary keys and nullability. RLS, partitioning, triggers, defaults,
generated/identity columns and unsupported constraints are refused. Keep native
metadata exclusively owned by the migration protocol throughout the job.
