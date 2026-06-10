/**
 * Memory subsystem wire types — mirror the SHIPPED Rust routes in
 * `crates/xiaoguai-api/src/routes/memory.rs` (`/v1/memories`) and the
 * DTOs in `xiaoguai_memory::types`. When the Rust crate adds a field,
 * mirror it here.
 *
 * Every `/v1/memories` JSON response wraps its payload in `{ data }`
 * ([`MemoryEnvelope`]); the client methods unwrap it.
 */

// ---- Enums -----------------------------------------------------------------

/** Mirrors `MemoryKind` (serde snake_case): facts / episodes / preferences. */
export type MemoryKind = 'facts' | 'episodes' | 'preferences';

export const MEMORY_KINDS: readonly MemoryKind[] = ['facts', 'episodes', 'preferences'];

// ---- Core record -----------------------------------------------------------

/** Mirrors `xiaoguai_memory::types::Memory`. */
export interface MemoryRecord {
  /** UUID assigned by the server. */
  id: string;
  kind: MemoryKind;
  /** Natural-language content of the memory. */
  content: string;
  /** Embedding vector; the server omits it when empty. */
  content_embedding?: number[];
  /** Topic tags (includes `source:` convention tags, T7.2). */
  tags: string[];
  /** RFC 3339 expiry. null = never expires. */
  ttl_at: string | null;
  /** RFC 3339. Immutable after creation. */
  created_at: string;
  /** RFC 3339 or null if never recalled. */
  last_recalled_at: string | null;
  recall_count: number;
}

// ---- List ------------------------------------------------------------------

/** Query for `GET /v1/memories?kind=&tags=&limit=&offset=`. */
export interface ListMemoriesQuery {
  kind?: MemoryKind;
  /** Sent as one comma-separated `tags=` param; a memory must carry every given tag. */
  tags?: string[];
  /** Server default: 50. */
  limit?: number;
  offset?: number;
}

// ---- Create / Update -------------------------------------------------------

/** Body for `POST /v1/memories`. */
export interface CreateMemoryRequest {
  kind: MemoryKind;
  content: string;
  tags?: string[];
  /** RFC 3339 expiry; omit or null = never expires. */
  ttl_at?: string | null;
}

/** Body for `PUT /v1/memories/:id`. Only content, tags, ttl are mutable. */
export interface UpdateMemoryRequest {
  content?: string;
  tags?: string[];
  /** `null` clears the TTL; omitting the field leaves it unchanged. */
  ttl_at?: string | null;
}

// ---- Semantic recall ---------------------------------------------------------

/** Body for `POST /v1/memories/recall`. */
export interface RecallMemoriesRequest {
  query: string;
  /** Server default: 5. */
  top_k?: number;
  kind_filter?: MemoryKind;
  tag_filter?: string[];
  /** Optional session UUID recorded on the recall trace. */
  session_id?: string;
}

/**
 * One recall / similarity hit. Returned by `POST /v1/memories/recall`
 * and `GET /v1/memories/similar/:id`.
 */
export interface RecalledMemory {
  memory: MemoryRecord;
  /** Cosine similarity in [0, 1]. Higher is more similar. */
  score: number;
}

// ---- Response envelope -------------------------------------------------------

/** `{ data }` wrapper used by every `/v1/memories` JSON response. */
export interface MemoryEnvelope<T> {
  data: T;
}

// ---- Import / export (T7.2) -------------------------------------------------
//
//   GET  /v1/memories/export?kind=   → text/plain JSONL document
//   POST /v1/memories/import         → text/plain JSONL body
// Response shape mirrors `xiaoguai_memory::jsonl::ImportReport`.

/** One skipped line from a JSONL import (1-based line number + reason). */
export interface MemoryImportSkippedLine {
  line: number;
  reason: string;
}

/**
 * Outcome of `POST /v1/memories/import`. Fail-soft: malformed lines are
 * reported in `skipped`, never abort the run.
 */
export interface MemoryImportReport {
  imported: number;
  skipped: MemoryImportSkippedLine[];
}
