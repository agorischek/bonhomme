ALTER TABLE agent_jobs
    ADD COLUMN request_fingerprint TEXT NOT NULL DEFAULT repeat('0', 64)
    CHECK (octet_length(request_fingerprint) = 64);
