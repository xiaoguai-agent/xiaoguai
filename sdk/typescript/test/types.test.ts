/**
 * Type-correctness tests.
 *
 * These tests verify that the exported types are correct and that the type
 * system catches misuse at compile time. They also serve as usage examples.
 */

import { describe, it, expect } from "vitest";
import type {
  HotlPolicy,
  CreateHotlPolicyRequest,
  HotlVerdict,
  HotlVerdictKind,
  RecordOutcomeRequest,
  OutcomeSummaryResponse,
  OutcomesTimeseriesResponse,
  InstalledSkillPack,
  SkillPackEntry,
  OutcomeAggregate,
  OutcomeDay,
} from "../src/index.js";

// ---------------------------------------------------------------------------
// HotlPolicy shape
// ---------------------------------------------------------------------------

describe("HotlPolicy type", () => {
  it("accepts a fully-specified policy", () => {
    const policy: HotlPolicy = {
      id: "aaaabbbb-0000-0000-0000-000000000001",
      tenant_id: "tenant-abc",
      scope: "llm_call",
      window_seconds: 3600,
      max_count: 100,
      max_usd: 50.0,
      escalate_to: "ops@example.com",
    };
    expect(policy.scope).toBe("llm_call");
  });

  it("accepts null optional fields", () => {
    const policy: HotlPolicy = {
      id: "id-1",
      tenant_id: "t1",
      scope: "email_send",
      window_seconds: 60,
      max_count: null,
      max_usd: null,
      escalate_to: null,
    };
    expect(policy.max_count).toBeNull();
    expect(policy.max_usd).toBeNull();
    expect(policy.escalate_to).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// CreateHotlPolicyRequest shape
// ---------------------------------------------------------------------------

describe("CreateHotlPolicyRequest type", () => {
  it("accepts minimal request (no optionals)", () => {
    const req: CreateHotlPolicyRequest = {
      tenant_id: "t1",
      scope: "llm_call",
      window_seconds: 3600,
    };
    expect(req.tenant_id).toBe("t1");
  });

  it("accepts full request with all optionals", () => {
    const req: CreateHotlPolicyRequest = {
      tenant_id: "t1",
      scope: "llm_call",
      window_seconds: 3600,
      max_count: 100,
      max_usd: 50.0,
      escalate_to: "ops@example.com",
    };
    expect(req.max_count).toBe(100);
  });
});

// ---------------------------------------------------------------------------
// HotlVerdict
// ---------------------------------------------------------------------------

describe("HotlVerdict type", () => {
  it("allow verdict has no reason", () => {
    const v: HotlVerdict = { verdict: "allow" };
    expect(v.verdict).toBe("allow");
  });

  it("deny verdict with reason", () => {
    const v: HotlVerdict = { verdict: "deny", reason: "max_count exceeded" };
    expect(v.reason).toBe("max_count exceeded");
  });

  it("escalate verdict", () => {
    const v: HotlVerdict = { verdict: "escalate", reason: "escalated to ops@example.com" };
    expect(v.verdict).toBe("escalate");
  });

  it("HotlVerdictKind union type covers all cases", () => {
    const kinds: HotlVerdictKind[] = ["allow", "deny", "escalate"];
    expect(kinds).toHaveLength(3);
  });
});

// ---------------------------------------------------------------------------
// RecordOutcomeRequest shape
// ---------------------------------------------------------------------------

describe("RecordOutcomeRequest type", () => {
  it("accepts minimal request", () => {
    const req: RecordOutcomeRequest = {
      tenant_id: "t1",
      agent_name: "sales-bot",
      kind: "revenue_usd",
      value: 1200.0,
    };
    expect(req.value).toBe(1200.0);
  });

  it("accepts full request with metadata", () => {
    const req: RecordOutcomeRequest = {
      tenant_id: "t1",
      agent_name: "hr-bot",
      kind: "hours_saved",
      value: 8.0,
      session_id: "session-123",
      unit: "hours",
      description: "Automated onboarding",
      metadata: { deal_id: "deal-456", region: "APAC" },
    };
    expect(req.metadata?.["deal_id"]).toBe("deal-456");
  });
});

// ---------------------------------------------------------------------------
// Outcomes response shapes
// ---------------------------------------------------------------------------

describe("OutcomeSummaryResponse type", () => {
  it("has correct shape", () => {
    const agg: OutcomeAggregate = { count: 3, sum: 1500, avg: 500 };
    const resp: OutcomeSummaryResponse = {
      tenant_id: "t1",
      range: "30d",
      summary: {
        by_kind: { revenue_usd: agg },
      },
    };
    expect(resp.summary.by_kind["revenue_usd"]?.sum).toBe(1500);
  });
});

describe("OutcomesTimeseriesResponse type", () => {
  it("has correct shape", () => {
    const day: OutcomeDay = { date: "2026-05-25", kind: "revenue_usd", count: 2, sum: 800 };
    const resp: OutcomesTimeseriesResponse = {
      tenant_id: "t1",
      range: "7d",
      days: [day],
    };
    expect(resp.days[0]?.sum).toBe(800);
  });
});

// ---------------------------------------------------------------------------
// Skills shapes
// ---------------------------------------------------------------------------

describe("InstalledSkillPack type", () => {
  it("has correct shape", () => {
    const pack: InstalledSkillPack = {
      id: "row-1",
      tenant_id: "t1",
      pack_slug: "rag-legal",
      version: "1.0.0",
      config: { top_k: 10 },
      installed_at: "2026-05-25T00:00:00Z",
    };
    expect(pack.config["top_k"]).toBe(10);
  });
});

describe("SkillPackEntry type", () => {
  it("accepts minimal entry", () => {
    const entry: SkillPackEntry = {
      slug: "rag-hr",
      name: "RAG HR",
      description: "HR knowledge pack",
      version: "1.0.0",
      category: "hr",
    };
    expect(entry.slug).toBe("rag-hr");
  });

  it("accepts full entry with knobs and requires", () => {
    const entry: SkillPackEntry = {
      slug: "rag-finance",
      name: "RAG Finance",
      description: "Finance knowledge pack",
      version: "1.0.0",
      category: "finance",
      requires: { feature_flags: ["rag"], env_keys: ["VECTOR_DB_URL"] },
      knobs: {
        top_k: { type: "integer", default: 5, description: "Number of results" },
        model: { type: "string", default: "claude-3", description: "Model to use" },
      },
      screenshot_url: "https://example.com/screenshot.png",
    };
    expect(entry.requires?.feature_flags).toContain("rag");
    expect(entry.knobs?.["top_k"]).toBeDefined();
  });
});
