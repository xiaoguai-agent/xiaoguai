/**
 * v1.4-ready — Memory subsystem wire types + client methods.
 *
 * The `xiaoguai-memory` Rust crate (task #155) is not yet shipped.
 * These types mirror the planned API contract described in ADR-0019.
 * When the crate ships, validate wire shapes here and remove the 404
 * fallback in the Memory pane.
 */

// ---- Enums -----------------------------------------------------------------

export type MemoryType = 'fact' | 'episode' | 'preference';

// ---- Core record -----------------------------------------------------------

export interface MemoryRecord {
  /** UUID assigned by the server. */
  id: string;
  type: MemoryType;
  /** Markdown-formatted content. */
  content: string;
  /** Operator-assigned labels. */
  tags: string[];
  tenant_id: string;
  /** ID of the agent that created this memory. */
  agent_id: string | null;
  /** RFC 3339. Immutable after creation. */
  created_at: string;
  /** RFC 3339 or null if never recalled. */
  last_recalled_at: string | null;
  recall_count: number;
  /** ISO 8601 duration string, e.g. "P30D". null = never expires. */
  ttl: string | null;
}

// ---- List ------------------------------------------------------------------

export interface ListMemoriesQuery {
  type?: MemoryType;
  tenant_id?: string;
  agent_id?: string;
  tag?: string;
  /** RFC 3339 inclusive lower bound on created_at. */
  since?: string;
  /** RFC 3339 inclusive upper bound on created_at. */
  until?: string;
  limit?: number;
  offset?: number;
}

export interface ListMemoriesResponse {
  records: MemoryRecord[];
  total: number;
  limit: number;
  offset: number;
}

// ---- Create / Update -------------------------------------------------------

export interface CreateMemoryRequest {
  type: MemoryType;
  content: string;
  tags?: string[];
  tenant_id: string;
  agent_id?: string | null;
  ttl?: string | null;
}

export interface UpdateMemoryRequest {
  /** Only content, tags, and ttl are mutable. */
  content?: string;
  tags?: string[];
  ttl?: string | null;
}

// ---- Recall trace ----------------------------------------------------------

export interface RecallEntry {
  memory_id: string;
  /** Cosine similarity in [0, 1]. */
  relevance_score: number;
  /** Which agent triggered the recall. */
  agent_id: string;
  /** RFC 3339. */
  recalled_at: string;
  /** Snapshot of the memory content at recall time (first 200 chars). */
  content_preview: string;
  type: MemoryType;
  tags: string[];
}

export interface RecallTraceResponse {
  session_id: string | null;
  query: string | null;
  entries: RecallEntry[];
  total: number;
}

// ---- Vector neighbors ------------------------------------------------------

export interface SimilarMemory {
  memory_id: string;
  /** Cosine similarity in [0, 1]. */
  similarity: number;
  content_preview: string;
  type: MemoryType;
  tags: string[];
  created_at: string;
}

export interface FindSimilarMemoriesResponse {
  anchor_id: string;
  neighbors: SimilarMemory[];
}

// ---- Import / export (T7.2) -------------------------------------------------
//
// Unlike the types above (planned ADR-0019 contract, still pending wire
// validation), these mirror the SHIPPED Rust routes in
// `crates/xiaoguai-api/src/routes/memory.rs`:
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
