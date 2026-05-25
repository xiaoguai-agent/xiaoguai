package io.github.xiaoguaiagent.client;

import io.github.xiaoguaiagent.client.error.XiaoguaiException;
import io.github.xiaoguaiagent.client.internal.HttpExecutor;
import io.github.xiaoguaiagent.client.internal.JsonCodec;
import io.github.xiaoguaiagent.client.model.HotlPolicy;
import io.github.xiaoguaiagent.client.model.HotlVerdict;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * API operations for the HOTL (Human-On-The-Loop) boundary-policy subsystem.
 *
 * <p>Wraps {@code GET/POST/DELETE /v1/hotl/policies} and (placeholder)
 * {@code POST /v1/hotl/check}.
 *
 * <p>Obtain an instance via {@link XiaoguaiClient#hotl()}.
 */
public final class HotlApi {

    private final HttpExecutor http;
    private final JsonCodec json;

    HotlApi(HttpExecutor http, JsonCodec json) {
        this.http = http;
        this.json = json;
    }

    // -------------------------------------------------------------------------
    // Policies
    // -------------------------------------------------------------------------

    /**
     * List HOTL policies for the given tenant.
     *
     * <p>Wraps {@code GET /v1/hotl/policies?tenant_id=<uuid>[&scope=<str>]}.
     *
     * @param tenantId required tenant UUID
     * @param scope    optional — filter by action scope (e.g. {@code "llm_call"})
     * @return list of matching policies (may be empty)
     */
    public List<HotlPolicy> listHotlPolicies(String tenantId, String scope) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Map<String, String> params = new HashMap<>();
        params.put("tenant_id", tenantId);
        if (scope != null) params.put("scope", scope);
        String body = http.get("/v1/hotl/policies", params);
        return json.decodeList(body, HotlPolicy.class);
    }

    /**
     * List HOTL policies for the given tenant (no scope filter).
     *
     * @param tenantId required tenant UUID
     * @return list of policies (may be empty)
     */
    public List<HotlPolicy> listHotlPolicies(String tenantId) {
        return listHotlPolicies(tenantId, null);
    }

    /**
     * Create a new HOTL policy.
     *
     * <p>At least one of {@code maxCount} or {@code maxUsd} must be non-null.
     * Wraps {@code POST /v1/hotl/policies}.
     *
     * @param tenantId      tenant UUID
     * @param scope         action category, e.g. {@code "llm_call"}
     * @param windowSeconds rolling window width in seconds
     * @param maxCount      maximum invocation count (null = no count limit)
     * @param maxUsd        maximum cumulative cost in USD (null = no cost limit)
     * @param escalateTo    escalation destination on breach (null = deny)
     * @return the created policy row
     */
    public HotlPolicy createHotlPolicy(
            String tenantId,
            String scope,
            int windowSeconds,
            Integer maxCount,
            Double maxUsd,
            String escalateTo) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Objects.requireNonNull(scope, "scope must not be null");
        Map<String, Object> payload = new HashMap<>();
        payload.put("tenant_id", tenantId);
        payload.put("scope", scope);
        payload.put("window_seconds", windowSeconds);
        if (maxCount != null) payload.put("max_count", maxCount);
        if (maxUsd != null) payload.put("max_usd", maxUsd);
        if (escalateTo != null) payload.put("escalate_to", escalateTo);
        String responseBody = http.post("/v1/hotl/policies", json.encode(payload));
        return json.decode(responseBody, HotlPolicy.class);
    }

    /**
     * Get a single HOTL policy by ID.
     *
     * <p>Wraps {@code GET /v1/hotl/policies/:id}.
     *
     * @param policyId UUID of the policy
     * @return the policy
     * @throws io.github.xiaoguaiagent.client.error.NotFoundException if the id is unknown
     */
    public HotlPolicy getHotlPolicy(String policyId) {
        Objects.requireNonNull(policyId, "policyId must not be null");
        String responseBody = http.get("/v1/hotl/policies/" + policyId, Map.of());
        return json.decode(responseBody, HotlPolicy.class);
    }

    /**
     * Update an existing HOTL policy (replace semantics).
     *
     * <p>Wraps {@code PUT /v1/hotl/policies/:id}.
     *
     * <p>Note: if the server does not yet expose this endpoint, delete and
     * re-create the policy as a workaround.
     *
     * @param policyId      UUID of the policy to update
     * @param windowSeconds new rolling window width in seconds
     * @param maxCount      new max count (null = no count limit)
     * @param maxUsd        new max cost (null = no cost limit)
     * @param escalateTo    new escalation destination (null = deny on breach)
     * @return the updated policy row
     */
    public HotlPolicy updateHotlPolicy(
            String policyId,
            int windowSeconds,
            Integer maxCount,
            Double maxUsd,
            String escalateTo) {
        Objects.requireNonNull(policyId, "policyId must not be null");
        Map<String, Object> payload = new HashMap<>();
        payload.put("window_seconds", windowSeconds);
        if (maxCount != null) payload.put("max_count", maxCount);
        if (maxUsd != null) payload.put("max_usd", maxUsd);
        if (escalateTo != null) payload.put("escalate_to", escalateTo);
        String responseBody = http.post("/v1/hotl/policies/" + policyId, json.encode(payload));
        return json.decode(responseBody, HotlPolicy.class);
    }

    /**
     * Delete a HOTL policy by ID.
     *
     * <p>Wraps {@code DELETE /v1/hotl/policies/:id}.
     *
     * @param policyId UUID of the policy to delete
     * @throws io.github.xiaoguaiagent.client.error.NotFoundException if the id is unknown
     */
    public void deleteHotlPolicy(String policyId) {
        Objects.requireNonNull(policyId, "policyId must not be null");
        http.delete("/v1/hotl/policies/" + policyId);
    }

    /**
     * Pre-flight budget check — returns a {@link HotlVerdict} without triggering an LLM call.
     *
     * <p>Wraps {@code POST /v1/hotl/check}.
     *
     * <p>Note: a dedicated check endpoint may not yet be wired on the server.
     * Budget checks also run in-process on the message path.
     *
     * @param tenantId tenant to check against
     * @param scope    action scope (e.g. {@code "llm_call"})
     * @param amount   proposed cost in USD or invocation count (context-dependent)
     * @return a verdict of {@code ALLOW}, {@code ESCALATE}, or {@code DENY}
     */
    public HotlVerdict checkHotl(String tenantId, String scope, double amount) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Objects.requireNonNull(scope, "scope must not be null");
        Map<String, Object> payload = new HashMap<>();
        payload.put("tenant_id", tenantId);
        payload.put("scope", scope);
        payload.put("amount", amount);
        String responseBody = http.post("/v1/hotl/check", json.encode(payload));
        return json.decode(responseBody, HotlVerdict.class);
    }
}
