CREATE TABLE slices (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    branch_id UUID NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    base_position BIGINT NOT NULL CHECK (base_position >= 0),
    root_symbols JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_slices_repository ON slices(repository_id, created_at);
CREATE INDEX idx_slices_branch ON slices(branch_id, created_at);
