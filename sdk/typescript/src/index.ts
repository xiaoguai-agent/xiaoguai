/**
 * @xiaoguai/client — TypeScript SDK for the xiaoguai wave-3 REST API.
 *
 * @example
 * ```ts
 * import { XiaoguaiClient } from "@xiaoguai/client";
 *
 * const client = new XiaoguaiClient({
 *   baseUrl: "http://localhost:8080",
 *   token: "my-bearer-token",
 * });
 *
 * const policies = await client.listHotlPolicies({ tenant_id: "tenant-uuid" });
 * ```
 */

// Main client
export { XiaoguaiClient } from "./client.js";
export type { XiaoguaiClientConfig } from "./client.js";

// Error hierarchy
export {
  XiaoguaiError,
  HttpError,
  AuthError,
  ForbiddenError,
  NotFoundError,
  ConflictError,
  ValidationError,
  RateLimitError,
  ServerError,
  NotImplementedError,
} from "./errors.js";

// Types (re-export all named types so consumers don't need to import from sub-paths)
export type {
  // HotL
  HotlPolicy,
  CreateHotlPolicyRequest,
  HotlVerdict,
  HotlVerdictKind,
  ListHotlPoliciesParams,
  // Outcomes
  RecordOutcomeRequest,
  RecordOutcomeResponse,
  OutcomeAggregate,
  OutcomeSummaryResponse,
  OutcomeDay,
  OutcomesTimeseriesResponse,
  OutcomesSummaryParams,
  OutcomesTimeseriesParams,
  // Skills
  PackRequires,
  KnobSchema,
  SkillPackEntry,
  SkillCatalogResponse,
  InstalledSkillPack,
  InstallSkillRequest,
} from "./types.js";

// Internal http utilities (for advanced consumers who want to extend the client)
export type { FetchFn } from "./http.js";
