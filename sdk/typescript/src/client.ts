/**
 * XiaoguaiClient — wave-3 REST API client.
 *
 * Covers:
 *   - HotL boundary policy CRUD  (v1.2.3)
 *   - Outcome telemetry          (v1.2.4)
 *   - Skill pack marketplace     (v1.2.28)
 *
 * Usage:
 *   import { XiaoguaiClient } from "@xiaoguai/client";
 *   const client = new XiaoguaiClient({ baseUrl: "http://localhost:8080", token: "my-token" });
 *   const policies = await client.listHotlPolicies({ tenant_id: "..." });
 */

import { HttpClient, type FetchFn } from "./http.js";
import { NotImplementedError } from "./errors.js";
import type {
  CreateHotlPolicyRequest,
  HotlPolicy,
  HotlVerdict,
  InstalledSkillPack,
  InstallSkillRequest,
  ListHotlPoliciesParams,
  OutcomesSummaryParams,
  OutcomeSummaryResponse,
  OutcomesTimeseriesParams,
  OutcomesTimeseriesResponse,
  RecordOutcomeRequest,
  SkillCatalogResponse,
  SkillPackEntry,
} from "./types.js";

// ---------------------------------------------------------------------------
// Client config
// ---------------------------------------------------------------------------

export interface XiaoguaiClientConfig {
  /** Root URL of the running server, e.g. `"http://localhost:8080"`. No trailing `/v1`. */
  baseUrl: string;
  /** Bearer token. Pass `undefined` when the server has auth disabled. */
  token?: string;
  /** Per-request timeout in milliseconds. Defaults to 30 000. */
  timeout?: number;
  /**
   * Custom fetch implementation. Defaults to the global `fetch`.
   * Useful for injecting mocks in tests or for Node.js < 18 environments.
   */
  fetch?: FetchFn;
}

// ---------------------------------------------------------------------------
// Client class
// ---------------------------------------------------------------------------

export class XiaoguaiClient {
  private readonly http: HttpClient;

  constructor(config: XiaoguaiClientConfig) {
    const fetchFn: FetchFn =
      config.fetch ??
      (typeof globalThis !== "undefined" && typeof (globalThis as typeof globalThis & { fetch?: FetchFn }).fetch === "function"
        ? (globalThis as typeof globalThis & { fetch: FetchFn }).fetch
        : (() => {
            throw new Error(
              "No global fetch found. Pass a fetch implementation via the `fetch` option.",
            );
          }) as unknown as FetchFn);

    this.http = new HttpClient({
      baseUrl: config.baseUrl,
      token: config.token,
      timeoutMs: config.timeout ?? 30_000,
      fetch: fetchFn,
    });
  }

  // -------------------------------------------------------------------------
  // HotL — boundary policy CRUD  (v1.2.3)
  // -------------------------------------------------------------------------

  /**
   * List HOTL policies for a tenant, optionally filtered by scope.
   *
   * Wraps `GET /v1/hotl/policies?tenant_id=<uuid>[&scope=<str>]`.
   */
  listHotlPolicies(
    params: ListHotlPoliciesParams,
    signal?: AbortSignal,
  ): Promise<HotlPolicy[]> {
    return this.http.get<HotlPolicy[]>("/v1/hotl/policies", params, signal);
  }

  /**
   * Create a new HOTL policy.
   *
   * At least one of `max_count` or `max_usd` must be provided.
   * Wraps `POST /v1/hotl/policies`.
   */
  createHotlPolicy(
    req: CreateHotlPolicyRequest,
    signal?: AbortSignal,
  ): Promise<HotlPolicy> {
    return this.http.post<HotlPolicy>("/v1/hotl/policies", req, signal);
  }

  /**
   * Fetch a single HOTL policy by ID.
   *
   * Note: the server currently only exposes list + create + delete, not a
   * dedicated GET-by-id endpoint. This method throws `NotImplementedError`.
   * Use `listHotlPolicies` and filter client-side as a workaround.
   */
  getHotlPolicy(_policyId: string): Promise<HotlPolicy> {
    throw new NotImplementedError(
      "GET /v1/hotl/policies/:id is not yet exposed by the server. " +
        "Use listHotlPolicies({ tenant_id }) and filter client-side.",
    );
  }

  /**
   * Update an existing HOTL policy.
   *
   * Note: the server does not yet expose a PATCH/PUT endpoint for policies.
   * Delete and re-create the policy as a workaround.
   */
  updateHotlPolicy(_policyId: string, _updates: Partial<CreateHotlPolicyRequest>): Promise<HotlPolicy> {
    throw new NotImplementedError(
      "PATCH /v1/hotl/policies/:id is not yet exposed by the server. " +
        "Delete and re-create the policy instead.",
    );
  }

  /**
   * Delete a HOTL policy by ID.
   *
   * Wraps `DELETE /v1/hotl/policies/:id`.
   * Throws `NotFoundError` when the ID is unknown.
   */
  async deleteHotlPolicy(policyId: string, signal?: AbortSignal): Promise<void> {
    await this.http.delete<unknown>(`/v1/hotl/policies/${policyId}`, signal);
  }

  /**
   * Check whether an action is within budget.
   *
   * Note: the server's enforcer runs in-process on the message path.
   * A dedicated `POST /v1/hotl/check` endpoint is not yet wired into the
   * router. This method throws `NotImplementedError` until it is.
   */
  checkHotl(_scope: string, _amount: number, _tenantId?: string): Promise<HotlVerdict> {
    throw new NotImplementedError(
      "POST /v1/hotl/check is not yet exposed by the server. " +
        "Budget checks run in-process when sending messages to a session.",
    );
  }

  // -------------------------------------------------------------------------
  // Outcomes — ROI telemetry  (v1.2.4)
  // -------------------------------------------------------------------------

  /**
   * Record a business outcome attribution.
   *
   * Wraps `POST /v1/outcomes`.
   * Returns `true` on success.
   */
  async recordOutcome(
    req: RecordOutcomeRequest,
    signal?: AbortSignal,
  ): Promise<boolean> {
    const res = await this.http.post<{ ok: boolean }>("/v1/outcomes", req, signal);
    return Boolean(res?.ok);
  }

  /**
   * List raw outcome records.
   *
   * Note: the server only exposes aggregated endpoints (`/summary` and
   * `/timeseries`). This method throws `NotImplementedError`.
   * Use `outcomesSummary` or `outcomesTimeseries` instead.
   */
  listOutcomes(_filter?: Record<string, unknown>): Promise<unknown[]> {
    throw new NotImplementedError(
      "GET /v1/outcomes (raw list) is not yet exposed by the server. " +
        "Use outcomesSummary() or outcomesTimeseries() instead.",
    );
  }

  /**
   * Aggregated ROI summary — one bucket per outcome kind.
   *
   * `range` accepts `"24h"`, `"7d"`, or `"30d"` (default `"30d"`).
   * Wraps `GET /v1/outcomes/summary`.
   */
  outcomesSummary(
    params: OutcomesSummaryParams,
    signal?: AbortSignal,
  ): Promise<OutcomeSummaryResponse> {
    return this.http.get<OutcomeSummaryResponse>("/v1/outcomes/summary", params, signal);
  }

  /**
   * Daily time-series breakdown.
   *
   * `range` accepts `"24h"`, `"7d"`, or `"30d"` (default `"30d"`).
   * `kind` optionally filters to a single outcome kind (e.g. `"revenue_usd"`).
   * Wraps `GET /v1/outcomes/timeseries`.
   */
  outcomesTimeseries(
    params: OutcomesTimeseriesParams,
    signal?: AbortSignal,
  ): Promise<OutcomesTimeseriesResponse> {
    return this.http.get<OutcomesTimeseriesResponse>(
      "/v1/outcomes/timeseries",
      params,
      signal,
    );
  }

  // -------------------------------------------------------------------------
  // Skills — pack marketplace  (v1.2.28)
  // -------------------------------------------------------------------------

  /**
   * List skill packs installed for a tenant.
   *
   * Wraps `GET /v1/skills/installed?tenant=<tenant_id>`.
   */
  listInstalledSkills(
    tenantId?: string,
    signal?: AbortSignal,
  ): Promise<InstalledSkillPack[]> {
    const params = tenantId ? { tenant: tenantId } : undefined;
    return this.http.get<InstalledSkillPack[]>("/v1/skills/installed", params, signal);
  }

  /**
   * Install a skill pack for a tenant.
   *
   * `pack_slug` must exist in the built-in catalog.
   * Throws `ConflictError` when already installed.
   * Wraps `POST /v1/skills/install`.
   */
  installSkill(
    req: InstallSkillRequest,
    signal?: AbortSignal,
  ): Promise<InstalledSkillPack> {
    return this.http.post<InstalledSkillPack>("/v1/skills/install", req, signal);
  }

  /**
   * Uninstall a skill pack by its installation row ID.
   *
   * Throws `NotFoundError` when the row is absent.
   * Wraps `DELETE /v1/skills/install/:id`.
   */
  async uninstallSkill(installId: string, signal?: AbortSignal): Promise<string> {
    const data = await this.http.delete<{ deleted: string }>(
      `/v1/skills/install/${installId}`,
      signal,
    );
    return data?.deleted ?? installId;
  }

  /**
   * List all available skill packs from the built-in catalog.
   *
   * Wraps `GET /v1/skills/catalog` (public, no auth required).
   */
  async listSkillCatalog(signal?: AbortSignal): Promise<SkillPackEntry[]> {
    const data = await this.http.get<SkillCatalogResponse>("/v1/skills/catalog", undefined, signal);
    return data?.packs ?? [];
  }
}
