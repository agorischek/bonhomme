# Production operations

These procedures assume [the production manifest](../deploy/bonhomme.production.toml), Postgres as
the durable backend, and secrets supplied by the deployment platform. Do not place database URLs or
API tokens in `bonhomme.toml`, shell history, images, or support bundles.

Each API process is deliberately single-tenant. Give each tenant a dedicated Postgres database (or
an independently access-controlled schema and database role); never point processes configured for
different tenants at the same repository tables. The tenant value binds authentication, audit, and
jobs, while database isolation is the trust boundary for repositories, branches, conflicts, and
demo state.

## Deployment preflight

Set deployment values through the secret manager, then run the same fail-closed check used by CI:

```sh
export DATABASE_URL='postgres://<user>:<password>@<host>/<database>?sslmode=require'
export BONHOMME_API_TOKEN='<at-least-32-random-bytes>'
export BONHOMME_API_TENANTS='team-production'
export BONHOMME_API_SUBJECT='bonhomme-api-production'
export BONHOMME_API_ROLE='writer'
cargo run --locked -p bonhomme -- production check \
  --config deploy/bonhomme.production.toml \
  --addr 0.0.0.0:3030
```

The checked production manifest must declare `[audit] export_policy = "external-worm"`. This is an
enforced deployment-policy assertion, not a built-in remote sink: the deployment must also configure
and monitor an automatic exporter/anchor job as described below.

Direct mutating CLI commands write intent/outcome pairs to the same local chain before and after
dispatch. They record only a static command classification, never arguments, storage URLs, paths, or
payload bodies. Set `BONHOMME_CLI_ACTOR` and `BONHOMME_CLI_TENANT` for attributable operator runs; if
absent, the CLI reuses the single API subject/tenant when configured, then falls back to
`local-cli`/`local` for local development. Read-only and long-running service commands are not
classified as CLI mutations; HTTP mutations remain audited at the authenticated request boundary.

The command validates the effective environment and prints a machine-readable report. It does not
contact Postgres or start a listener. A deployment gate should require `ready: true`, then perform a
separate database connectivity and migration check using the release artifact.

## Backup and restore

### Postgres

Create a database-native, checksummed backup and store both files in encrypted object storage:

```sh
backup_name="bonhomme-$(date -u +%Y%m%dT%H%M%SZ).dump"
pg_dump --dbname="$DATABASE_URL" --format=custom --no-owner --file="$backup_name"
shasum -a 256 "$backup_name" > "$backup_name.sha256"
```

Restore drills must use a new, isolated database—not the active production database:

```sh
shasum -a 256 -c "$backup_name.sha256"
createdb "$RESTORE_DATABASE_NAME"
pg_restore --dbname="$RESTORE_DATABASE_URL" --no-owner --exit-on-error "$backup_name"
DATABASE_URL="$RESTORE_DATABASE_URL" cargo run --locked -p bonhomme -- validate \
  --repo "$RESTORE_REPOSITORY" --branch main
```

Record backup time, database server version, schema migration level, checksum, restore duration,
and validation result. Run a restore drill at least quarterly and before storage migrations.

### Embedded Turso development state

Embedded storage is a local/offline tier, not the production reference backend. Stop every process
using it before copying `.bonhomme/bonhomme.db`; copy the audit log separately and verify it after
restore. A live filesystem copy is not a supported backup because pending database writes may not be
present in the main file.

```sh
backup_dir="bonhomme-local-backup-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir "$backup_dir"
cp -p .bonhomme/bonhomme.db "$backup_dir/bonhomme.db"
cp -p .bonhomme/audit.jsonl "$backup_dir/audit.jsonl"
cargo run --locked -p bonhomme -- production verify-audit \
  --path "$backup_dir/audit.jsonl"
```

Restore only while Bonhomme is stopped, preserve the failed database for investigation, start the
service, and validate every important repository/branch before deleting the preserved copy.

## API credential rotation

Bonhomme currently accepts one bearer token per server process. Use a coordinated rotation or put
dual-token rotation at the ingress; do not run a mixed-token fleet and assume every client can reach
every instance.

1. Generate a cryptographically random token of at least 32 bytes in the secret manager.
2. Quiesce mutation traffic and wait for active requests/jobs to finish.
3. Update clients and server deployment references atomically (or use an ingress overlap window).
4. Restart/roll the service, then test an authorized read and mutation with a unique request ID.
5. Confirm the old token receives `401 Unauthorized` on every instance.
6. Verify the mutation's audit intent/outcome pair and revoke/delete the old secret version.

Rotate immediately after suspected exposure, operator departure, accidental logging, or a change in
tenant scope. Changing `BONHOMME_API_SUBJECT`, role, or single-tenant binding should be treated as a
credential rotation because those fields define the principal recorded in the audit chain.

## Audit verification and anchoring

The local audit path is `<project-root>/.bonhomme/audit.jsonl` unless the deployed root differs.
Verify that exact deployed path; the command checks JSON decoding, contiguous sequence numbers,
previous-hash linkage, and every entry hash:

```sh
cargo run --locked -p bonhomme -- production verify-audit \
  --path /var/lib/bonhomme/.bonhomme/audit.jsonl > audit-verification.json
```

Continuously or on a tightly bounded schedule, automatically send `entries`, `headHash`, deployment
identity, and timestamp from the verification report to an immutable external system (for example,
retention-locked object storage or a security log service). Alert when export misses its declared
schedule. The hash chain detects edits and truncation only relative to a previously anchored head.
Deleting the entire local log produces an empty valid chain, so external WORM anchoring is required
to detect wholesale deletion or replacement.

Treat verification failure as an incident. Make a read-only copy before attempting recovery; never
edit the audit log in place or discard an unverifiable tail.

## Incident response

1. **Contain:** remove the service from ingress, block mutation credentials, and preserve read-only
   access only if the incident commander accepts the risk.
2. **Preserve:** snapshot Postgres, the audit log, application logs, job artifacts, deployment
   manifest digest, image digest, and effective non-secret configuration. Record UTC times.
3. **Verify:** run `production verify-audit`, compare its head to the last external anchor, and map
   unmatched `attempt`/`outcome` pairs to database state before retrying any request.
4. **Scope:** identify affected tenants, branches, operations, credentials, workers, and time range.
5. **Eradicate:** rotate tokens/database credentials, patch the root cause, and rebuild from pinned
   dependencies. Do not reuse a possibly compromised worktree or image.
6. **Recover:** restore into isolation, run migrations and semantic validation, compare operation
   counts and audit anchors, then reopen reads before mutations.
7. **Close:** document impact, evidence, decisions, notifications, and follow-up owners. Add a
   regression test or deployment check that detects the original failure mode.

For an audit write failure after dispatch, the API deliberately reports that the mutation may have
completed. Inspect durable state by request ID before retrying; an unconditional retry can duplicate
work.
