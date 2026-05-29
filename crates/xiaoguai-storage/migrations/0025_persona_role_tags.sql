-- Sprint-9 S9-7 (DEC-021): persona role tags.
--
-- v1.6+ introduces the planner/worker/critic triangle pattern. The
-- three roles are regular personas (see lld-personas.md v1.6+ note —
-- no enforced role type), so to make role-shaped personas
-- *discoverable* by operators and the admin UI, we add an optional
-- tags array. Convention is to prefix with `role/`:
--   role/planner-default
--   role/worker-coder
--   role/critic-strict
-- but the column is free-form so operators can also tag by domain
-- (`domain/finance`, `domain/k8s`, ...) for filtering.
--
-- NULL or empty-array means "untagged" — the persona is still usable
-- in any pattern; tags are purely a discovery convenience.

ALTER TABLE personas
    ADD COLUMN IF NOT EXISTS tags TEXT[];

-- GIN index makes `WHERE 'role/critic' = ANY(tags)` and
-- `WHERE tags && ARRAY['role/critic','role/worker']` queries cheap
-- enough to use in the admin UI's persona picker without paging
-- through all personas.
CREATE INDEX IF NOT EXISTS idx_personas_tags
    ON personas USING GIN (tags);

-- Comment on the column so `psql \d+ personas` documents the convention.
COMMENT ON COLUMN personas.tags IS
    'Optional discovery tags (free-form). Convention: role/{planner,worker,critic} '
    'plus domain/* prefixes. Triangle pattern picker filters on role/* tags. '
    'NULL or empty = untagged. See DEC-HLD-021 + lld-personas.md v1.6+.';
