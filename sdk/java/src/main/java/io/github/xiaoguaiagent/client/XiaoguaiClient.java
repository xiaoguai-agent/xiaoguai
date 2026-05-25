package io.github.xiaoguaiagent.client;

import io.github.xiaoguaiagent.client.internal.HttpExecutor;
import io.github.xiaoguaiagent.client.internal.JsonCodec;

import java.time.Duration;

/**
 * Main entry point for the xiaoguai REST API Java SDK.
 *
 * <p>This client is thread-safe and should be shared across the application.
 * It implements {@link AutoCloseable} so it can be used in try-with-resources.
 *
 * <p>Usage:
 * <pre>{@code
 * try (XiaoguaiClient client = XiaoguaiClient.builder()
 *         .baseUrl("http://localhost:8080")
 *         .bearerToken("my-token")
 *         .build()) {
 *
 *     // HotL policy management
 *     var policies = client.hotl().listHotlPolicies("tenant-uuid");
 *
 *     // Outcomes telemetry
 *     client.outcomes().recordOutcome("tenant-uuid", "sales-bot", "revenue_usd", 1200.0);
 *
 *     // Skills marketplace
 *     var catalog = client.skills().listSkillCatalog();
 * }
 * }</pre>
 *
 * <p>All API methods throw sub-classes of
 * {@link io.github.xiaoguaiagent.client.error.XiaoguaiException} on failure.
 *
 * @see XiaoguaiClientBuilder
 * @see HotlApi
 * @see OutcomesApi
 * @see SkillsApi
 */
public final class XiaoguaiClient implements AutoCloseable {

    private final HttpExecutor httpExecutor;
    private final JsonCodec jsonCodec;
    private final HotlApi hotlApi;
    private final OutcomesApi outcomesApi;
    private final SkillsApi skillsApi;

    /**
     * Create a client via the fluent builder.
     *
     * @return a new {@link XiaoguaiClientBuilder}
     */
    public static XiaoguaiClientBuilder builder() {
        return new XiaoguaiClientBuilder();
    }

    /**
     * Convenience factory: create a client with a base URL and bearer token.
     *
     * @param baseUrl     root URL of the xiaoguai-api server
     * @param bearerToken bearer token (may be null for dev environments)
     * @return configured client
     */
    public static XiaoguaiClient create(String baseUrl, String bearerToken) {
        return builder().baseUrl(baseUrl).bearerToken(bearerToken).build();
    }

    /**
     * Package-private constructor — use {@link #builder()} or {@link #create(String, String)}.
     */
    XiaoguaiClient(String baseUrl, String authHeader, Duration timeout, int maxRetries) {
        this.jsonCodec = new JsonCodec();
        this.httpExecutor = new HttpExecutor(baseUrl, authHeader, timeout, maxRetries);
        this.hotlApi = new HotlApi(httpExecutor, jsonCodec);
        this.outcomesApi = new OutcomesApi(httpExecutor, jsonCodec);
        this.skillsApi = new SkillsApi(httpExecutor, jsonCodec);
    }

    // -------------------------------------------------------------------------
    // API namespace accessors
    // -------------------------------------------------------------------------

    /**
     * HOTL (Human-On-The-Loop) boundary policy operations.
     *
     * @return the HotL API facade
     */
    public HotlApi hotl() {
        return hotlApi;
    }

    /**
     * Outcomes ROI telemetry operations.
     *
     * @return the Outcomes API facade
     */
    public OutcomesApi outcomes() {
        return outcomesApi;
    }

    /**
     * Skills marketplace operations.
     *
     * @return the Skills API facade
     */
    public SkillsApi skills() {
        return skillsApi;
    }

    // -------------------------------------------------------------------------
    // Lifecycle
    // -------------------------------------------------------------------------

    /**
     * Close the underlying HTTP connection pool.
     *
     * <p>The client should not be used after calling this method.
     */
    @Override
    public void close() {
        httpExecutor.close();
    }
}
