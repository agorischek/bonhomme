# Durable agent jobs

Bonhomme's agent queue is stored in the configured database. A job belongs to both an authenticated
tenant and a repository. Clients never submit the tenant in a job payload: the HTTP layer derives it
from the bearer principal and applies it to every enqueue, list, lease, heartbeat, result, failure,
and cancellation operation.

## Worker protocol

1. Enqueue with `POST /api/repos/{repo}/jobs`. Supply a stable `idempotencyKey`, `kind`, and JSON
   `payload`; retrying an identical request returns the original job. Keys are tenant-wide, so
   reusing one with a different repository or immutable field is rejected.
2. A worker calls `POST /api/repos/{repo}/jobs/lease` with a stable `owner`, optional `kind`, and a
   lease between 1 and 3,600 seconds. Leasing is atomic and returns an opaque `leaseToken`; only its
   digest is stored. Expired leases are reclaimable, including after either the API or worker process
   restarts.
3. Long work periodically calls `POST /api/repos/{repo}/jobs/{job}/heartbeat` with the same owner.
   Supply both the owner and lease token. Only the current, unexpired lease holder can extend or
   finish a job; ordinary list/show responses never expose the token.
4. Finish with `/succeed` and a JSON `result`, or `/fail` and an error string. Failures requeue with
   capped exponential backoff until `maxAttempts`, then become terminal. Diagnostics larger than
   16 KiB are UTF-8-safely truncated with an explicit marker so reporting an oversized failure can
   never strand an otherwise valid lease.
5. An authorized writer can call `/cancel`. Terminal jobs retain their result or error for later
   inspection through `GET /api/repos/{repo}/jobs/{job}`.

Workers must make their external side effects idempotent using the Bonhomme job ID. A lease prevents
concurrent ownership; it cannot provide exactly-once execution if a worker performs an external
side effect and crashes before persisting success. Check cancellation before expensive work and
again before committing a side effect.

## Operational guidance

- Give each worker process a unique owner value that remains stable for its lifetime.
- Use the shortest lease that safely exceeds one heartbeat interval, with at least two heartbeats
  per lease window.
- Alert on old queued jobs, repeated lease expiry, terminal failures, and jobs near `maxAttempts`.
- During credential rotation, let leases drain or preserve the worker tenant and owner identity.
- Database backup and restore automatically include jobs; after restore, expired leased jobs are
  reclaimed by the next worker poll.
