package io.github.xiaoguaiagent.client.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.Map;

/**
 * Aggregated ROI summary response from {@code GET /v1/outcomes/summary}.
 *
 * <p>Contains one {@link Bucket} per outcome kind.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public record OutcomeSummary(
        @JsonProperty("tenant_id") String tenantId,
        /** Time range used for the aggregation, e.g. {@code "30d"}. */
        @JsonProperty("range") String range,
        @JsonProperty("summary") SummaryBody summary
) {

    /** The nested summary envelope containing per-kind buckets. */
    @JsonIgnoreProperties(ignoreUnknown = true)
    public record SummaryBody(
            @JsonProperty("by_kind") Map<String, Bucket> byKind
    ) {}

    /** Aggregated totals for one outcome kind. */
    @JsonIgnoreProperties(ignoreUnknown = true)
    public record Bucket(
            @JsonProperty("count") int count,
            @JsonProperty("sum") double sum,
            @JsonProperty("avg") double avg
    ) {}
}
