package io.github.xiaoguaiagent.client;

import com.github.tomakehurst.wiremock.junit5.WireMockExtension;
import io.github.xiaoguaiagent.client.error.AuthException;
import io.github.xiaoguaiagent.client.error.HttpException;
import io.github.xiaoguaiagent.client.error.ServerException;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.RegisterExtension;

import java.time.Duration;

import static com.github.tomakehurst.wiremock.client.WireMock.*;
import static com.github.tomakehurst.wiremock.core.WireMockConfiguration.wireMockConfig;
import static org.assertj.core.api.Assertions.*;

/**
 * Tests for {@link XiaoguaiClient} lifecycle, builder, and cross-cutting behaviour.
 */
class XiaoguaiClientTest {

    @RegisterExtension
    static WireMockExtension wm = WireMockExtension.newInstance()
            .options(wireMockConfig().dynamicPort())
            .build();

    private XiaoguaiClient client;

    @BeforeEach
    void setUp() {
        client = XiaoguaiClient.builder()
                .baseUrl(wm.baseUrl())
                .bearerToken("test-token")
                .timeout(Duration.ofSeconds(5))
                .maxRetries(0)
                .build();
    }

    @AfterEach
    void tearDown() {
        client.close();
    }

    // ---- Builder ----

    @Test
    void builder_missingBaseUrl_throwsIllegalState() {
        assertThatThrownBy(() -> XiaoguaiClient.builder().build())
                .isInstanceOf(IllegalStateException.class)
                .hasMessageContaining("baseUrl");
    }

    @Test
    void builder_negativeRetries_throwsIllegalArgument() {
        assertThatThrownBy(() -> XiaoguaiClient.builder().maxRetries(-1))
                .isInstanceOf(IllegalArgumentException.class);
    }

    @Test
    void builder_noToken_sendsRequestWithoutAuthHeader() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .withoutHeader("Authorization")
                .willReturn(okJson("[]")));

        try (XiaoguaiClient noAuth = XiaoguaiClient.builder().baseUrl(wm.baseUrl()).build()) {
            assertThat(noAuth.hotl().listHotlPolicies("t1")).isEmpty();
        }
        wm.verify(getRequestedFor(urlPathEqualTo("/v1/hotl/policies")));
    }

    @Test
    void create_factoryMethod_returnsWorkingClient() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .willReturn(okJson("[]")));

        try (XiaoguaiClient c = XiaoguaiClient.create(wm.baseUrl(), "tok")) {
            assertThat(c.hotl().listHotlPolicies("t1")).isEmpty();
        }
    }

    // ---- Auth errors ----

    @Test
    void request_401Response_throwsAuthException() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .willReturn(unauthorized()));

        assertThatThrownBy(() -> client.hotl().listHotlPolicies("t1"))
                .isInstanceOf(AuthException.class)
                .satisfies(e -> assertThat(((AuthException) e).statusCode()).isEqualTo(401));
    }

    @Test
    void request_403Response_throwsAuthException() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .willReturn(forbidden()));

        assertThatThrownBy(() -> client.hotl().listHotlPolicies("t1"))
                .isInstanceOf(AuthException.class)
                .satisfies(e -> assertThat(((AuthException) e).statusCode()).isEqualTo(403));
    }

    // ---- Server errors ----

    @Test
    void request_500Response_throwsServerException() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .willReturn(serverError().withBody("{\"error\":\"internal\"}")));

        assertThatThrownBy(() -> client.hotl().listHotlPolicies("t1"))
                .isInstanceOf(ServerException.class)
                .satisfies(e -> assertThat(((ServerException) e).statusCode()).isEqualTo(500));
    }

    @Test
    void request_503Response_noRetry_throwsServerException() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .willReturn(serviceUnavailable()));

        assertThatThrownBy(() -> client.hotl().listHotlPolicies("t1"))
                .isInstanceOf(ServerException.class)
                .satisfies(e -> assertThat(((ServerException) e).statusCode()).isEqualTo(503));
    }

    // ---- Retry behaviour ----

    @Test
    void retry_503_idempotentRequest_retriesOnce() {
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .inScenario("retry")
                .whenScenarioStateIs("Started")
                .willReturn(serviceUnavailable())
                .willSetStateTo("second"));
        wm.stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
                .inScenario("retry")
                .whenScenarioStateIs("second")
                .willReturn(okJson("[]")));

        try (XiaoguaiClient retryClient = XiaoguaiClient.builder()
                .baseUrl(wm.baseUrl())
                .bearerToken("tok")
                .maxRetries(1)
                .build()) {
            assertThat(retryClient.hotl().listHotlPolicies("t1")).isEmpty();
        }
        wm.verify(2, getRequestedFor(urlPathEqualTo("/v1/hotl/policies")));
    }

    // ---- AutoCloseable / try-with-resources ----

    @Test
    void client_isAutoCloseable() {
        wm.stubFor(get(urlPathEqualTo("/v1/skills/catalog"))
                .willReturn(okJson("{\"version\":1,\"packs\":[]}")));

        try (XiaoguaiClient c = XiaoguaiClient.create(wm.baseUrl(), null)) {
            assertThat(c.skills().listSkillCatalog()).isEmpty();
        }
        // No exception means close() completed without error
    }

    // ---- Sub-API accessor smoke ----

    @Test
    void apiAccessors_returnNonNull() {
        assertThat(client.hotl()).isNotNull();
        assertThat(client.outcomes()).isNotNull();
        assertThat(client.skills()).isNotNull();
    }
}
