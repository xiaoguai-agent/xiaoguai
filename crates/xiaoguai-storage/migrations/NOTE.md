# Migration Numbering Note

This directory follows a sequential numbering scheme. The assignment as of the
`chore/integration-premerge` branch is:

| Number | File | Branch |
|--------|------|--------|
| 0016 | `0016_tasks.sql` | `feat/kanban-backend-tasks` |
| 0017 | `0017_workspaces.sql` | `feat/workspace-multiboard` |
| 0018 | `0018_personas.sql` | `feat/personas-crate` (renumbered) |
| 0019 | reserved for `feat/memory-crate` | |

## Renumber reason

`feat/personas-crate` originally created this file as `0016_personas.sql`.
During integration-prep it was discovered that `feat/kanban-backend-tasks`
also claims 0016 (tasks) and `feat/workspace-multiboard` claims 0017
(workspaces). To avoid a collision the personas migration was renumbered to
0018 here.

**Action required for the personas-crate agent**: when merging
`feat/personas-crate`, discard `0016_personas.sql` from that branch and use
`0018_personas.sql` from this integration branch instead.  Update any
reference in `crates/xiaoguai-storage/src/migrations.rs` that hard-codes the
filename.
