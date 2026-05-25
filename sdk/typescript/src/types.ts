/**
 * Typed models for the xiaoguai wave-3 REST API.
 * Mirrors the Rust wire types in crates/xiaoguai-api/src/{hotl/policy,outcomes,skills}.rs
 */

// ---------------------------------------------------------------------------
// HotL — boundary policy
// ---------------------------------------------------------------------------

/** One row in `hotl_policies`. */
export interface HotlPolicy {
  id: string;
  tenant_id: string;
  /** Action category this policy applies to, e.g. `"llm_call"`. */
  scope: string;
  /** Rolling window width in seconds. */
  window_seconds: number;
  /** Maximum invocation count within the window. `null` = no count limit. */
  max_count: number | null;
  /** Maximum cumulative USD cost within the window. `null` = no cost limit. */
  max_usd: number | null;
  /** Escalation destination (IM channel or email). `null` = deny on breach. */
  escalate_to: string | null;
}

/** Body for `POST /v1/hotl/policies`. */
export interface CreateHotlPolicyRequest {
  tenant_id: string;
  scope: string;
  window_seconds: number;
  max_count?: number | null;
  max_usd?: number | null;
  escalate_to?: string | null;
}

/**
 * Decision returned by the HOTL enforcer.
 * The server's enforcer runs in-process on the message path;
 * a dedicated check endpoint is not yet wired.
 */
export type HotlVerdictKind = "allow" | "escalate" | "deny";

export interface HotlVerdict {
  verdict: HotlVerdictKind;
  /** Human-readable reason when verdict is `"escalate"` or `"deny"`. */
  reason?: string;
}

// ---------------------------------------------------------------------------
// Outcomes — ROI telemetry
// ---------------------------------------------------------------------------

export interface RecordOutcomeRequest {
  tenant_id: string;
  agent_name: string;
  /** One of the well-known kinds or `"custom"`. */
  kind: string;
  value: number;
  session_id?: string | null;
  unit?: string | null;
  description?: string | null;
  metadata?: Record<string, unknown>;
}

export interface RecordOutcomeResponse {
  ok: boolean;
}

/** Per-kind aggregate bucket in the summary response. */
export interface OutcomeAggregate {
  count: number;
  sum: number;
  avg: number;
}

export interface OutcomeSummaryResponse {
  tenant_id: string;
  range: string;
  summary: {
    by_kind: Record<string, OutcomeAggregate>;
  };
}

/** One day bucket in the timeseries response. */
export interface OutcomeDay {
  date: string;
  kind: string;
  count: number;
  sum: number;
}

export interface OutcomesTimeseriesResponse {
  tenant_id: string;
  range: string;
  days: OutcomeDay[];
}

// ---------------------------------------------------------------------------
// Skills — pack marketplace
// ---------------------------------------------------------------------------

/** Feature-flag / env-var prerequisites. */
export interface PackRequires {
  feature_flags?: string[];
  env_keys?: string[];
}

/** Knob definition from the catalog. */
export type KnobSchema =
  | { type: "integer"; default: number; description: string }
  | { type: "boolean"; default: boolean; description: string }
  | { type: "string"; enum?: string[]; default: string; description: string };

/** One entry in the skill catalog. */
export interface SkillPackEntry {
  slug: string;
  name: string;
  description: string;
  version: string;
  category: string;
  requires?: PackRequires;
  knobs?: Record<string, KnobSchema>;
  screenshot_url?: string | null;
}

export interface SkillCatalogResponse {
  version: number;
  packs: SkillPackEntry[];
}

/** Installed-pack row returned by `GET /v1/skills/installed`. */
export interface InstalledSkillPack {
  id: string;
  tenant_id: string;
  pack_slug: string;
  version: string;
  config: Record<string, unknown>;
  installed_at: string;
}

export interface InstallSkillRequest {
  tenant_id: string;
  pack_slug: string;
  config?: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

export interface ListHotlPoliciesParams {
  tenant_id: string;
  scope?: string;
  [key: string]: string | undefined;
}

export interface OutcomesSummaryParams {
  tenant_id: string;
  /** `"24h"` | `"7d"` | `"30d"`. Defaults to `"30d"`. */
  range?: string;
  [key: string]: string | undefined;
}

export interface OutcomesTimeseriesParams {
  tenant_id: string;
  range?: string;
  kind?: string;
  [key: string]: string | undefined;
}
