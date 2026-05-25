package io.github.xiaoguaiagent.client.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.Map;

/**
 * One outcome attribution record from {@code GET /v1/outcomes}.
 *
 * <p>Mirrors {@code OutcomeRow} in
 * {@code crates/xiaoguai-api/src/outcomes.rs}.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public record OutcomeRecord(
        @JsonProperty("id") String id,
        @JsonProperty("tenant_id") String tenantId,
        @JsonProperty("session_id") String sessionId,
        @JsonProperty("agent_name") String agentName,
        /** One of the well-known kinds or {@code "custom"}. */
        @JsonProperty("kind") String kind,
        @JsonProperty("value") double value,
        /** Unit of measurement (e.g. {@code "USD"}, {@code "minutes"}). */
        @JsonProperty("unit") String unit,
        @JsonProperty("description") String description,
        @JsonProperty("metadata") Map<String, Object> metadata,
        /** ISO-8601 timestamp when the outcome was recorded. */
        @JsonProperty("recorded_at") String recordedAt
) {}
