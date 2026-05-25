package io.github.xiaoguaiagent.client;

import com.github.tomakehurst.wiremock.junit5.WireMockExtension;
import io.github.xiaoguaiagent.client.error.HttpException;
import io.github.xiaoguaiagent.client.model.OutcomeRecord;
import io.github.xiaoguaiagent.client.model.OutcomeSummary;
import io.github.xiaoguaiagent.client.model.OutcomeTimeseries;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.RegisterExtension;

import java.time.Duration;
import java.util.List;
import java.util.Map;

import static com.github.tomakehurst.wiremock.client.WireMock.*;
import static com.github.tomakehurst.wiremock.core.WireMockConfiguration.wireMockConfig;
import static org.assertj.core.api.Assertions.*;

/**
 * Tests for {@link OutcomesApi} — record, list, summary, and timeseries operations.
 */
class OutcomesApiTest {

    @RegisterExtension
    static WireMockExtension wm = WireMockExtension.newInstance()
            .options(wireMockConfig().dynamicPort())
            .build();

    private XiaoguaiClient client;

    @BeforeEach
    void setUp() {
        client = XiaoguaiClient.builder()
                .baseUrl(wm.baseUrl())
                .bearerToken("tok")
                .timeout(Duration.ofSeconds(5))
                .maxRetries(0)
                .build();
    }

    @AfterEach
    void tearDown() { client.close(); }

    // ---- recordOutcome ----

    @Test
    void recordOutcome_happyPath_returnsTrue() {
        wm.stubFor(post(urlPathEqualTo("/v1/outcomes"))
                .withRequestBody(matchingJsonPath("$.agent_name", equalTo("sales-bot")))
                .withRequestBody(matchingJsonPath("$.kind", equalTo("revenue_usd")))
                .withRequestBody(matchingJsonPath("$.value", equalTo("1200.0")))
                .willReturn(okJson("{\"ok\":true}")));

        boolean result = client.outcomes().recordOutcome("tenant-abc", "sales-bot", "revenue_usd", 1200.0);
        assertThat(result).isTrue();
    }

    @Test
    void recordOutcome_withAllFields_sendsFullPayload() {
        wm.stubFor(post(urlPathEqualTo("/v1/outcomes"))
                .withRequestBody(matchingJsonPath("$.session_id", equalTo("sess-1")))
                .withRequestBody(matchingJsonPath("$.unit", equalTo("USD")))
                .withRequestBody(matchingJsonPath("$.description", equalTo("closed deal")))
                .willReturn(okJson("{\"ok\":true}")));

        boolean result = client.outcomes().recordOutcome(
                "t1", "sales-bot", "revenue_usd", 500.0,
                "sess-1", "USD", "closed deal", Map.of("region", "APAC"));
        assertThat(result).isTrue();
    }

    @Test
    void recordOutcome_serverReturnsFalse_returnsFalse() {
        wm.stubFor(post(urlPathEqualTo("/v1/outcomes"))
                .willReturn(okJson("{\"ok\":false}")));

        boolean result = client.outcomes().recordOutcome("t1", "bot", "kind", 0.0);
        assertThat(result).isFalse();
    }

    @Test
    void recordOutcome_400Response_throwsHttpException() {
        wm.stubFor(post(urlPathEqualTo("/v1/outcomes"))
                .willReturn(badRequest().withBody("{\"error\":\"invalid kind\"}")));

        assertThatThrownBy(() -> client.outcomes().recordOutcome("t1", "bot", "", 0.0))
                .isInstanceOf(HttpException.class)
                .satisfies(e -> assertThat(((HttpException) e).statusCode()).isEqualTo(400));
    }

    // ---- listOutcomes ----

    @Test
    void listOutcomes_happyPath_returnsList() {
        String recordJson = "{\"id\":\"r1\",\"tenant_id\":\"t1\",\"session_id\":null," +
                "\"agent_name\":\"bot\",\"kind\":\"revenue_usd\",\"value\":100.0," +
                "\"unit\":\"USD\",\"description\":null,\"metadata\":{},\"recorded_at\":\"2026-05-25T10:00:00Z\"}";
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes"))
                .withQueryParam("tenant_id", equalTo("t1"))
                .willReturn(okJson("[" + recordJson + "]")));

        List<OutcomeRecord> records = client.outcomes().listOutcomes("t1");
        assertThat(records).hasSize(1);
        assertThat(records.get(0).agentName()).isEqualTo("bot");
        assertThat(records.get(0).value()).isEqualTo(100.0);
    }

    @Test
    void listOutcomes_withFilters_passesQueryParams() {
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes"))
                .withQueryParam("tenant_id", equalTo("t1"))
                .withQueryParam("kind", equalTo("revenue_usd"))
                .withQueryParam("agent_name", equalTo("bot"))
                .withQueryParam("limit", equalTo("10"))
                .willReturn(okJson("[]")));

        assertThat(client.outcomes().listOutcomes("t1", "bot", "revenue_usd", 10)).isEmpty();
        wm.verify(getRequestedFor(urlPathEqualTo("/v1/outcomes"))
                .withQueryParam("limit", equalTo("10")));
    }

    // ---- outcomesSummary ----

    @Test
    void outcomesSummary_happyPath_parsesByKind() {
        String summaryJson = "{\"tenant_id\":\"t1\",\"range\":\"30d\"," +
                "\"summary\":{\"by_kind\":{\"revenue_usd\":{\"count\":5,\"sum\":3200.0,\"avg\":640.0}}}}";
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes/summary"))
                .withQueryParam("tenant_id", equalTo("t1"))
                .willReturn(okJson(summaryJson)));

        OutcomeSummary summary = client.outcomes().outcomesSummary("t1");
        assertThat(summary.tenantId()).isEqualTo("t1");
        assertThat(summary.range()).isEqualTo("30d");
        assertThat(summary.summary().byKind()).containsKey("revenue_usd");
        assertThat(summary.summary().byKind().get("revenue_usd").count()).isEqualTo(5);
        assertThat(summary.summary().byKind().get("revenue_usd").sum()).isEqualTo(3200.0);
    }

    @Test
    void outcomesSummary_withRange_passesQueryParam() {
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes/summary"))
                .withQueryParam("range", equalTo("7d"))
                .willReturn(okJson("{\"tenant_id\":\"t1\",\"range\":\"7d\",\"summary\":{\"by_kind\":{}}}")));

        OutcomeSummary summary = client.outcomes().outcomesSummary("t1", "7d");
        assertThat(summary.range()).isEqualTo("7d");
    }

    @Test
    void outcomesSummary_500Response_throwsServerException() {
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes/summary"))
                .willReturn(serverError()));

        assertThatThrownBy(() -> client.outcomes().outcomesSummary("t1"))
                .isInstanceOf(HttpException.class)
                .satisfies(e -> assertThat(((HttpException) e).statusCode()).isEqualTo(500));
    }

    // ---- outcomesTimeseries ----

    @Test
    void outcomesTimeseries_happyPath_parsesDays() {
        String tsJson = "{\"tenant_id\":\"t1\",\"range\":\"7d\"," +
                "\"days\":[{\"date\":\"2026-05-25\",\"kind\":\"revenue_usd\",\"count\":2,\"sum\":800.0}]}";
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes/timeseries"))
                .withQueryParam("tenant_id", equalTo("t1"))
                .willReturn(okJson(tsJson)));

        OutcomeTimeseries ts = client.outcomes().outcomesTimeseries("t1");
        assertThat(ts.days()).hasSize(1);
        assertThat(ts.days().get(0).date()).isEqualTo("2026-05-25");
        assertThat(ts.days().get(0).kind()).isEqualTo("revenue_usd");
        assertThat(ts.days().get(0).sum()).isEqualTo(800.0);
    }

    @Test
    void outcomesTimeseries_withKindFilter_passesQueryParam() {
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes/timeseries"))
                .withQueryParam("kind", equalTo("hours_saved"))
                .willReturn(okJson("{\"tenant_id\":\"t1\",\"range\":\"30d\",\"days\":[]}")));

        OutcomeTimeseries ts = client.outcomes().outcomesTimeseries("t1", null, "hours_saved");
        assertThat(ts.days()).isEmpty();
        wm.verify(getRequestedFor(urlPathEqualTo("/v1/outcomes/timeseries"))
                .withQueryParam("kind", equalTo("hours_saved")));
    }

    @Test
    void outcomesTimeseries_emptyDays_returnsEmptyList() {
        wm.stubFor(get(urlPathEqualTo("/v1/outcomes/timeseries"))
                .willReturn(okJson("{\"tenant_id\":\"t1\",\"range\":\"24h\",\"days\":[]}")));

        assertThat(client.outcomes().outcomesTimeseries("t1", "24h", null).days()).isEmpty();
    }
}
