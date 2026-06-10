-- T7.1 memory multisource (docs/plans/2026-06-10-memory-multisource.md §1.1):
-- optional team-shared markdown glossary (terminology, constraints,
-- procedures). Injected as a system message into every turn of a session the
-- team is attached to, and into orchestrate member/synthesis runs. NULL =
-- no glossary. Capped at 16 KiB at the write boundary (not in SQL — the
-- repos reject oversized values with InvalidArgument before the write).
ALTER TABLE expert_teams ADD COLUMN glossary_md TEXT;
