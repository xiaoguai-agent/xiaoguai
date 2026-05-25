package io.github.xiaoguaiagent.client.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * One row from {@code GET /v1/hotl/policies}.
 *
 * <p>Mirrors {@code HotlPolicyRow} in
 * {@code crates/xiaoguai-api/src/hotl/policy.rs}.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public record HotlPolicy(
        @JsonProperty("id") String id,
        @JsonProperty("tenant_id") String tenantId,
        /** Action category this policy applies to, e.g. {@code "llm_call"}. */
        @JsonProperty("scope") String scope,
        /** Rolling window width in seconds. */
        @JsonProperty("window_seconds") int windowSeconds,
        /** Maximum invocation count within the window. {@code null} = no count limit. */
        @JsonProperty("max_count") Integer maxCount,
        /** Maximum cumulative USD cost within the window. {@code null} = no cost limit. */
        @JsonProperty("max_usd") Double maxUsd,
        /** Escalation destination (IM channel or email). {@code null} = deny on breach. */
        @JsonProperty("escalate_to") String escalateTo
) {}
