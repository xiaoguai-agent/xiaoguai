package io.github.xiaoguaiagent.client;

import io.github.xiaoguaiagent.client.internal.HttpExecutor;
import io.github.xiaoguaiagent.client.internal.JsonCodec;
import io.github.xiaoguaiagent.client.model.OutcomeRecord;
import io.github.xiaoguaiagent.client.model.OutcomeSummary;
import io.github.xiaoguaiagent.client.model.OutcomeTimeseries;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * API operations for the Outcomes (ROI telemetry) subsystem.
 *
 * <p>Wraps {@code POST /v1/outcomes}, {@code GET /v1/outcomes/summary},
 * and {@code GET /v1/outcomes/timeseries}.
 *
 * <p>Obtain an instance via {@link XiaoguaiClient#outcomes()}.
 */
public final class OutcomesApi {

    private final HttpExecutor http;
    private final JsonCodec json;

    OutcomesApi(HttpExecutor http, JsonCodec json) {
        this.http = http;
        this.json = json;
    }

    /**
     * Record a business outcome attribution.
     *
     * <p>Wraps {@code POST /v1/outcomes}.
     *
     * @param tenantId    tenant UUID
     * @param agentName   name of the agent that produced the outcome
     * @param kind        outcome kind (e.g. {@code "revenue_usd"}, {@code "hours_saved"})
     * @param value       numeric value of the outcome
     * @param sessionId   optional session UUID for attribution
     * @param unit        optional unit label (e.g. {@code "USD"}, {@code "minutes"})
     * @param description optional human-readable description
     * @param metadata    optional freeform key-value map
     * @return {@code true} when the server acknowledges the record
     */
    public boolean recordOutcome(
            String tenantId,
            String agentName,
            String kind,
            double value,
            String sessionId,
            String unit,
            String description,
            Map<String, Object> metadata) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Objects.requireNonNull(agentName, "agentName must not be null");
        Objects.requireNonNull(kind, "kind must not be null");
        Map<String, Object> payload = new HashMap<>();
        payload.put("tenant_id", tenantId);
        payload.put("agent_name", agentName);
        payload.put("kind", kind);
        payload.put("value", value);
        payload.put("metadata", metadata != null ? metadata : Map.of());
        if (sessionId != null) payload.put("session_id", sessionId);
        if (unit != null) payload.put("unit", unit);
        if (description != null) payload.put("description", description);
        String responseBody = http.post("/v1/outcomes", json.encode(payload));
        Map<?, ?> resp = json.decode(responseBody, Map.class);
        Object ok = resp.get("ok");
        return Boolean.TRUE.equals(ok);
    }

    /**
     * Record an outcome with only the required fields.
     *
     * @param tenantId  tenant UUID
     * @param agentName agent name
     * @param kind      outcome kind
     * @param value     numeric value
     * @return {@code true} when the server acknowledges the record
     */
    public boolean recordOutcome(String tenantId, String agentName, String kind, double value) {
        return recordOutcome(tenantId, agentName, kind, value, null, null, null, null);
    }

    /**
     * List raw outcome records with optional filters.
     *
     * <p>Wraps {@code GET /v1/outcomes}.
     *
     * @param tenantId  required tenant UUID
     * @param agentName optional filter by agent name
     * @param kind      optional filter by outcome kind
     * @param limit     max results to return (null = server default)
     * @return list of matching outcome records
     */
    public List<OutcomeRecord> listOutcomes(
            String tenantId,
            String agentName,
            String kind,
            Integer limit) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Map<String, String> params = new HashMap<>();
        params.put("tenant_id", tenantId);
        if (agentName != null) params.put("agent_name", agentName);
        if (kind != null) params.put("kind", kind);
        if (limit != null) params.put("limit", limit.toString());
        String responseBody = http.get("/v1/outcomes", params);
        return json.decodeList(responseBody, OutcomeRecord.class);
    }

    /**
     * List all raw outcome records for a tenant.
     *
     * @param tenantId required tenant UUID
     * @return list of matching outcome records
     */
    public List<OutcomeRecord> listOutcomes(String tenantId) {
        return listOutcomes(tenantId, null, null, null);
    }

    /**
     * Aggregated ROI summary — one bucket per outcome kind.
     *
     * <p>Wraps {@code GET /v1/outcomes/summary}.
     *
     * @param tenantId tenant UUID
     * @param range    time range: {@code "24h"}, {@code "7d"}, or {@code "30d"}
     *                 (null defaults to {@code "30d"})
     * @return summary with per-kind aggregates
     */
    public OutcomeSummary outcomesSummary(String tenantId, String range) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Map<String, String> params = new HashMap<>();
        params.put("tenant_id", tenantId);
        if (range != null) params.put("range", range);
        String responseBody = http.get("/v1/outcomes/summary", params);
        return json.decode(responseBody, OutcomeSummary.class);
    }

    /**
     * Aggregated ROI summary using the default 30-day range.
     *
     * @param tenantId tenant UUID
     * @return summary with per-kind aggregates
     */
    public OutcomeSummary outcomesSummary(String tenantId) {
        return outcomesSummary(tenantId, null);
    }

    /**
     * Daily time-series breakdown.
     *
     * <p>Wraps {@code GET /v1/outcomes/timeseries}.
     *
     * @param tenantId tenant UUID
     * @param range    time range: {@code "24h"}, {@code "7d"}, or {@code "30d"}
     *                 (null defaults to {@code "30d"})
     * @param kind     optional — filter to a single outcome kind
     * @return time-series with one entry per (date, kind) pair
     */
    public OutcomeTimeseries outcomesTimeseries(String tenantId, String range, String kind) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Map<String, String> params = new HashMap<>();
        params.put("tenant_id", tenantId);
        if (range != null) params.put("range", range);
        if (kind != null) params.put("kind", kind);
        String responseBody = http.get("/v1/outcomes/timeseries", params);
        return json.decode(responseBody, OutcomeTimeseries.class);
    }

    /**
     * Daily time-series for all outcome kinds over the default 30-day range.
     *
     * @param tenantId tenant UUID
     * @return full time-series
     */
    public OutcomeTimeseries outcomesTimeseries(String tenantId) {
        return outcomesTimeseries(tenantId, null, null);
    }
}
