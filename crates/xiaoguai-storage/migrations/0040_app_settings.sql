-- 0040: generic owner-level key/value settings (SQLite single-owner, DEC-033).
--
-- A small kv store for runtime-editable owner preferences that don't warrant
-- their own table. First consumer: white-label **branding** — the assistant's
-- display name shown across the chat UI — stored as a JSON blob under the
-- `branding` key so the shape can grow (accent colour, tagline, avatar) without
-- another migration. Unset key = the UI falls back to its built-in default
-- ("Xiaoguai" / "小怪"). No tenant column: there is exactly one owner.
CREATE TABLE app_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
