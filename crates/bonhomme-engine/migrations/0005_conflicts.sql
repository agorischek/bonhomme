CREATE TABLE conflicts (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('slice_apply', 'merge')),
    state TEXT NOT NULL CHECK (state IN ('open', 'resolving', 'resolved', 'abandoned')),
    source_branch_id UUID REFERENCES branches(id) ON DELETE SET NULL,
    target_branch_id UUID REFERENCES branches(id) ON DELETE SET NULL,
    source_branch_name TEXT NOT NULL,
    target_branch_name TEXT NOT NULL,
    slice_id UUID REFERENCES slices(id) ON DELETE SET NULL,
    source_changeset_id UUID REFERENCES changesets(id) ON DELETE SET NULL,
    base_position BIGINT NOT NULL CHECK (base_position >= 0),
    target_position BIGINT NOT NULL CHECK (target_position >= 0),
    conflicts JSONB NOT NULL DEFAULT '[]'::jsonb,
    proposed_operations JSONB NOT NULL DEFAULT '[]'::jsonb,
    resolution_changeset_id UUID REFERENCES changesets(id) ON DELETE SET NULL,
    acknowledged_drop BOOLEAN NOT NULL DEFAULT false,
    created_by TEXT NOT NULL,
    resolved_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ
);

CREATE INDEX idx_conflicts_repository_state ON conflicts(repository_id, state, created_at);
CREATE INDEX idx_conflicts_source_branch ON conflicts(source_branch_id, state);
CREATE INDEX idx_conflicts_target_branch ON conflicts(target_branch_id, state);
