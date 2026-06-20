CREATE TABLE source_file_snapshots (
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    branch_id UUID NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    byte_len BIGINT NOT NULL,
    handler TEXT NOT NULL,
    file_symbol_id UUID,
    last_import_position BIGINT NOT NULL,
    importer_version TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (branch_id, path)
);

CREATE INDEX idx_source_file_snapshots_repository ON source_file_snapshots(repository_id);
