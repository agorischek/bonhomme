CREATE TABLE repositories (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE branches (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    base_branch_id UUID REFERENCES branches(id) ON DELETE SET NULL,
    base_position BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(repository_id, name)
);

CREATE TABLE tasks (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE changesets (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    task_id UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    branch_id UUID NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    created_by TEXT NOT NULL DEFAULT 'human',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE operations (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    branch_id UUID NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    changeset_id UUID NOT NULL REFERENCES changesets(id) ON DELETE CASCADE,
    position BIGINT NOT NULL,
    op_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(branch_id, position)
);

CREATE TABLE attachments (
    id UUID PRIMARY KEY,
    repository_id UUID NOT NULL REFERENCES repositories(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL,
    entity_id UUID NOT NULL,
    attachment_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_branches_repository_name ON branches(repository_id, name);
CREATE INDEX idx_changesets_branch ON changesets(branch_id, created_at);
CREATE INDEX idx_operations_branch_position ON operations(branch_id, position);
CREATE INDEX idx_operations_changeset ON operations(changeset_id);
CREATE INDEX idx_operations_payload ON operations USING GIN(payload);
CREATE INDEX idx_attachments_entity ON attachments(entity_type, entity_id);
