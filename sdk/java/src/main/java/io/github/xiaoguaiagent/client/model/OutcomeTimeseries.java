package io.github.xiaoguaiagent.client.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;

/**
 * Daily time-series breakdown from {@code GET /v1/outcomes/timeseries}.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public record OutcomeTimeseries(
        @JsonProperty("tenant_id") String tenantId,
        /** Time range used for the query, e.g. {@code "7d"}. */
        @JsonProperty("range") String range,
        @JsonProperty("days") List<Day> days
) {

    /** One day bucket in the time-series. */
    @JsonIgnoreProperties(ignoreUnknown = true)
    public record Day(
            /** ISO-8601 date string, e.g. {@code "2026-05-25"}. */
            @JsonProperty("date") String date,
            @JsonProperty("kind") String kind,
            @JsonProperty("count") int count,
            @JsonProperty("sum") double sum
    ) {}
}
