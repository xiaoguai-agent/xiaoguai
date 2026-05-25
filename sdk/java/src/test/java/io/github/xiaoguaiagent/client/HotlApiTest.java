package io.github.xiaoguaiagent.client;

import com.github.tomakehurst.wiremock.junit5.WireMockExtension;
import io.github.xiaoguaiagent.client.error.ConflictException;
import io.github.xiaoguaiagent.client.error.NotFoundException;
import io.github.xiaoguaiagent.client.model.HotlPolicy;
import io.github.xiaoguaiagent.client.model.HotlVerdict;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.RegisterExtension;

import java.time.Duration;
import java.util.List;

import static com.github.tomakehurst.wiremock.client.WireMock.*;
import static com.github.tomakehurst.wiremock.core.WireMockConfiguration.wireMockConfig;
import static org.assertj.core.api.Assertions.*;

/**
 * Tests for {@link HotlApi} — CRUD policy operations and check endpoint.
 */
class HotlApiTest {

    @RegisterExtension
    static WireMockExtension wm = WireMockExtension.newInstance()
            .options(wireMockConfig().dynamicPort())
            .build();

    private XiaoguaiClient client;

    private static final String POLICY_JSON =
            "{\"id\":\"pol-1\",\"tenant_id\":\"tenant-abc\",\"scope\":\"llm_call\"," +
            "\"window_seconds\":3600,\"max_count\":100,\"max_usd\":5.0,\"escalate_to\":\"#alerts\"}";

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

    // ---- listHotlPolicies ----

    @Test
    void listHotlPolicies_happyPath_returnsParsedList() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .withQueryParam("tenant_id", equalTo("tenant-abc"))
                .willReturn(okJson("[" + POLICY_JSON + "]")));

        List<HotlPolicy> policies = client.hotl().listHotlPolicies("tenant-abc");

        assertThat(policies).hasSize(1);
        HotlPolicy p = policies.get(0);
        assertThat(p.id()).isEqualTo("pol-1");
        assertThat(p.tenantId()).isEqualTo("tenant-abc");
        assertThat(p.scope()).isEqualTo("llm_call");
        assertThat(p.windowSeconds()).isEqualTo(3600);
        assertThat(p.maxCount()).isEqualTo(100);
        assertThat(p.maxUsd()).isEqualTo(5.0);
        assertThat(p.escalateTo()).isEqualTo("#alerts");
    }

    @Test
    void listHotlPolicies_withScope_passesQueryParam() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .withQueryParam("tenant_id", equalTo("t1"))
                .withQueryParam("scope", equalTo("llm_call"))
                .willReturn(okJson("[]")));

        assertThat(client.hotl().listHotlPolicies("t1", "llm_call")).isEmpty();
        wm.verify(getRequestedFor(urlPathEqualTo("/v1/hotl/policies"))
                .withQueryParam("scope", equalTo("llm_call")));
    }

    @Test
    void listHotlPolicies_emptyList_returnsEmptyList() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .willReturn(okJson("[]")));

        assertThat(client.hotl().listHotlPolicies("t1")).isEmpty();
    }

    @Test
    void listHotlPolicies_nullTenantId_throwsNPE() {
        assertThatNullPointerException()
                .isThrownBy(() -> client.hotl().listHotlPolicies(null))
                .withMessageContaining("tenantId");
    }

    // ---- createHotlPolicy ----

    @Test
    void createHotlPolicy_happyPath_returnsCreatedPolicy() {
        wm.stubFor(post(urlPathEqualTo("/v1/hotl/policies"))
                .withRequestBody(matchingJsonPath("$.tenant_id", equalTo("tenant-abc")))
                .withRequestBody(matchingJsonPath("$.scope", equalTo("llm_call")))
                .willReturn(okJson(POLICY_JSON)));

        HotlPolicy created = client.hotl().createHotlPolicy(
                "tenant-abc", "llm_call", 3600, 100, 5.0, "#alerts");

        assertThat(created.id()).isEqualTo("pol-1");
        assertThat(created.scope()).isEqualTo("llm_call");
    }

    @Test
    void createHotlPolicy_missingOptionalFields_stillSucceeds() {
        wm.stubFor(post(urlPathEqualTo("/v1/hotl/policies"))
                .willReturn(okJson("{\"id\":\"pol-2\",\"tenant_id\":\"t2\",\"scope\":\"tool_call\"," +
                        "\"window_seconds\":60,\"max_count\":null,\"max_usd\":null,\"escalate_to\":null}")));

        HotlPolicy p = client.hotl().createHotlPolicy("t2", "tool_call", 60, null, null, null);
        assertThat(p.maxCount()).isNull();
        assertThat(p.maxUsd()).isNull();
        assertThat(p.escalateTo()).isNull();
    }

    // ---- getHotlPolicy ----

    @Test
    void getHotlPolicy_happyPath_returnsSinglePolicy() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies/pol-1"))
                .willReturn(okJson(POLICY_JSON)));

        HotlPolicy p = client.hotl().getHotlPolicy("pol-1");
        assertThat(p.id()).isEqualTo("pol-1");
    }

    @Test
    void getHotlPolicy_notFound_throwsNotFoundException() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies/bad-id"))
                .willReturn(notFound().withBody("{\"error\":\"not found\"}")));

        assertThatThrownBy(() -> client.hotl().getHotlPolicy("bad-id"))
                .isInstanceOf(NotFoundException.class);
    }

    // ---- deleteHotlPolicy ----

    @Test
    void deleteHotlPolicy_happyPath_noException() {
        wm.stubFor(delete(urlPathEqualTo("/v1/hotl/policies/pol-1"))
                .willReturn(ok().withBody("{}")));

        assertThatCode(() -> client.hotl().deleteHotlPolicy("pol-1"))
                .doesNotThrowAnyException();
    }

    @Test
    void deleteHotlPolicy_notFound_throwsNotFoundException() {
        wm.stubFor(delete(urlPathEqualTo("/v1/hotl/policies/gone"))
                .willReturn(notFound().withBody("{\"error\":\"not found\"}")));

        assertThatThrownBy(() -> client.hotl().deleteHotlPolicy("gone"))
                .isInstanceOf(NotFoundException.class);
    }

    // ---- checkHotl ----

    @Test
    void checkHotl_verdictAllow_parsesCorrectly() {
        wm.stubFor(post(urlPathEqualTo("/v1/hotl/check"))
                .withRequestBody(matchingJsonPath("$.scope", equalTo("llm_call")))
                .willReturn(okJson("{\"verdict\":\"allow\",\"reason\":null}")));

        HotlVerdict verdict = client.hotl().checkHotl("tenant-abc", "llm_call", 0.50);

        assertThat(verdict.verdict()).isEqualTo(HotlVerdict.Kind.ALLOW);
        assertThat(verdict.isAllowed()).isTrue();
        assertThat(verdict.isDenied()).isFalse();
    }

    @Test
    void checkHotl_verdictDeny_hasReason() {
        wm.stubFor(post(urlPathEqualTo("/v1/hotl/check"))
                .willReturn(okJson("{\"verdict\":\"deny\",\"reason\":\"budget exceeded\"}")));

        HotlVerdict verdict = client.hotl().checkHotl("tenant-abc", "llm_call", 1000.0);

        assertThat(verdict.verdict()).isEqualTo(HotlVerdict.Kind.DENY);
        assertThat(verdict.isDenied()).isTrue();
        assertThat(verdict.reason()).isEqualTo("budget exceeded");
    }

    @Test
    void checkHotl_verdictEscalate_parsesCorrectly() {
        wm.stubFor(post(urlPathEqualTo("/v1/hotl/check"))
                .willReturn(okJson("{\"verdict\":\"escalate\",\"reason\":\"threshold reached\"}")));

        HotlVerdict verdict = client.hotl().checkHotl("t1", "llm_call", 3.0);
        assertThat(verdict.verdict()).isEqualTo(HotlVerdict.Kind.ESCALATE);
    }
}
