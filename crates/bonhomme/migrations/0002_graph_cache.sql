CREATE TABLE graph_cache (
    branch_id UUID PRIMARY KEY REFERENCES branches(id) ON DELETE CASCADE,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    operation_count BIGINT NOT NULL,
    operation_fingerprint TEXT NOT NULL,
    graph JSONB NOT NULL,
    rendered_files JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_graph_cache_repository ON graph_cache(repository_id);
