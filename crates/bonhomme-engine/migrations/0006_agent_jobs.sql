CREATE TABLE agent_jobs (
    id UUID PRIMARY KEY,
    tenant_key TEXT NOT NULL CHECK (octet_length(tenant_key) BETWEEN 1 AND 128),
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (octet_length(kind) BETWEEN 1 AND 128),
    idempotency_key TEXT NOT NULL CHECK (octet_length(idempotency_key) BETWEEN 1 AND 256),
    payload JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'leased', 'succeeded', 'failed', 'cancelled')),
    attempt_count BIGINT NOT NULL DEFAULT 0 CHECK (attempt_count BETWEEN 0 AND max_attempts),
    max_attempts BIGINT NOT NULL CHECK (max_attempts BETWEEN 1 AND 100),
    retry_backoff_seconds BIGINT NOT NULL CHECK (retry_backoff_seconds BETWEEN 0 AND 86400),
    max_backoff_seconds BIGINT NOT NULL
        CHECK (max_backoff_seconds BETWEEN retry_backoff_seconds AND 604800),
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_owner TEXT CHECK (lease_owner IS NULL OR octet_length(lease_owner) BETWEEN 1 AND 128),
    lease_token_digest TEXT CHECK (
        lease_token_digest IS NULL OR octet_length(lease_token_digest) = 64
    ),
    lease_expires_at TIMESTAMPTZ,
    result JSONB,
    error TEXT CHECK (error IS NULL OR octet_length(error) <= 16384),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    CHECK (
        (state = 'leased' AND lease_owner IS NOT NULL AND lease_token_digest IS NOT NULL
            AND lease_expires_at IS NOT NULL)
        OR (state <> 'leased' AND lease_owner IS NULL AND lease_token_digest IS NULL
            AND lease_expires_at IS NULL)
    ),
    CHECK ((state IN ('succeeded', 'failed', 'cancelled')) = (completed_at IS NOT NULL)),
    CHECK (result IS NULL OR state = 'succeeded'),
    UNIQUE (tenant_key, idempotency_key)
);

CREATE INDEX idx_agent_jobs_available
    ON agent_jobs(tenant_key, repository_id, state, available_at, created_at);
CREATE INDEX idx_agent_jobs_expired_leases
    ON agent_jobs(tenant_key, repository_id, lease_expires_at)
    WHERE state = 'leased';
