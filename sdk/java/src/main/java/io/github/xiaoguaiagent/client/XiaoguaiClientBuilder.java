package io.github.xiaoguaiagent.client;

import java.time.Duration;

/**
 * Fluent builder for {@link XiaoguaiClient}.
 *
 * <p>Usage:
 * <pre>{@code
 * XiaoguaiClient client = XiaoguaiClient.builder()
 *     .baseUrl("https://api.example.com")
 *     .bearerToken("my-secret-token")
 *     .timeout(Duration.ofSeconds(20))
 *     .maxRetries(2)
 *     .build();
 * }</pre>
 */
public final class XiaoguaiClientBuilder {

    private String baseUrl;
    private String bearerToken;
    private Duration timeout = Duration.ofSeconds(30);
    private int maxRetries = 1;

    XiaoguaiClientBuilder() {}

    /**
     * Set the root URL of the running {@code xiaoguai-api} server.
     *
     * <p>Must not include a trailing {@code /v1} path segment.
     * Example: {@code "http://localhost:8080"}.
     *
     * @param baseUrl the server base URL
     * @return this builder
     */
    public XiaoguaiClientBuilder baseUrl(String baseUrl) {
        this.baseUrl = baseUrl;
        return this;
    }

    /**
     * Set the Bearer token for the {@code Authorization} header.
     *
     * <p>Pass {@code null} when the server has auth disabled (dev / tests).
     *
     * @param token the bearer token
     * @return this builder
     */
    public XiaoguaiClientBuilder bearerToken(String token) {
        this.bearerToken = token;
        return this;
    }

    /**
     * Per-request timeout. Defaults to 30 seconds.
     *
     * @param timeout the request timeout
     * @return this builder
     */
    public XiaoguaiClientBuilder timeout(Duration timeout) {
        this.timeout = timeout;
        return this;
    }

    /**
     * Number of retries on transient network errors and 503 responses.
     * Defaults to 1. Set to 0 to disable retries.
     *
     * @param maxRetries retry count
     * @return this builder
     */
    public XiaoguaiClientBuilder maxRetries(int maxRetries) {
        if (maxRetries < 0) throw new IllegalArgumentException("maxRetries must be >= 0");
        this.maxRetries = maxRetries;
        return this;
    }

    /**
     * Build and return a configured {@link XiaoguaiClient}.
     *
     * @return new client instance
     * @throws IllegalStateException if {@code baseUrl} is not set
     */
    public XiaoguaiClient build() {
        if (baseUrl == null || baseUrl.isBlank()) {
            throw new IllegalStateException("baseUrl must be set before calling build()");
        }
        String authHeader = (bearerToken != null && !bearerToken.isBlank())
                ? "Bearer " + bearerToken
                : null;
        return new XiaoguaiClient(baseUrl, authHeader, timeout, maxRetries);
    }
}
