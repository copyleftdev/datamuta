# Three-row PostgreSQL migration

This example uses invented data and two disposable loopback PostgreSQL databases.
It requires Docker Compose and an installed `datamuta-stream`. Ports 55432 and
55433 must be free. The trust-authenticated, TLS-disabled endpoints are for this
local example only. See the book's connection reference for verified transport.

From `examples/postgres` in the repository:

```sh
docker compose --project-name datamuta-example up --detach --wait
```

Create a fresh private working directory and configure the explicit mapping:

```sh
umask 077
mkdir work
cd work
datamuta-stream configure \
  --source-endpoint ../source-endpoint.json --source-table orders \
  --target-endpoint ../target-endpoint.json --target-table purchases \
  --mapping ../mapping.json --out config.json
datamuta-stream capture --config config.json --spool snapshot --plan plan.json
datamuta-stream inspect --plan plan.json --spool snapshot
```

Review the two endpoints, four mappings and three captured rows. Replace `HASH`
with the inspected `plan_sha256` before publishing:

```sh
datamuta-stream run --plan plan.json --spool snapshot --approved-plan-sha256 HASH
datamuta-stream status --plan plan.json --spool snapshot
datamuta-stream verify --plan plan.json --spool snapshot --out verification.json
```

Expect `status: "verified"`, `rows_verified: 3`, and `fields_verified: 12` in the
verification report. Inspect actual target data from the example directory:

```sh
cd ..
docker compose --project-name datamuta-example exec -T target \
  psql -U datamuta -d datamuta -c 'TABLE dm_target.purchases;'
```

The target should contain keys 1, 2 and 3, values 12500, -750 and 0, a null memo on
key 2, and the preserved Unicode text on key 3. The identity column is bigint.
The SQL files initialize fresh databases; they are not recovery scripts.

After inspecting the result, remove only these disposable services:

```sh
docker compose --project-name datamuta-example down --volumes
```

Keep the local `work/` directory for review or remove it yourself after retiring
this example. For a new run, recreate the containers and use a new working
directory. An old plan belongs to its original native receipt domain.
