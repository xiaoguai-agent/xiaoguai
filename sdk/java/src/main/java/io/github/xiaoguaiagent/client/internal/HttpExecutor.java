package io.github.xiaoguaiagent.client.internal;

import io.github.xiaoguaiagent.client.error.HttpException;
import io.github.xiaoguaiagent.client.error.XiaoguaiException;

import java.io.IOException;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.Executors;

/**
 * Low-level HTTP layer using {@link java.net.http.HttpClient} with virtual threads.
 *
 * <p>All methods are synchronous (blocking) — the virtual-thread executor prevents
 * carrier-thread starvation under high concurrency.
 *
 * <p>Retry policy: a single automatic retry on network-level IOException (connection
 * reset, broken pipe). Idempotent requests (GET, DELETE) also retry on 503.
 */
public final class HttpExecutor implements AutoCloseable {

    private final HttpClient httpClient;
    private final String baseUrl;
    private final String authHeader;
    private final Duration timeout;
    private final int maxRetries;

    public HttpExecutor(String baseUrl, String authHeader, Duration timeout, int maxRetries) {
        this.baseUrl = baseUrl.endsWith("/") ? baseUrl.substring(0, baseUrl.length() - 1) : baseUrl;
        this.authHeader = authHeader;
        this.timeout = timeout;
        this.maxRetries = maxRetries;
        this.httpClient = HttpClient.newBuilder()
                .connectTimeout(timeout)
                .executor(Executors.newVirtualThreadPerTaskExecutor())
                .build();
    }

    /** Execute {@code GET <path>?<queryParams>} and return the raw response body. */
    public String get(String path, Map<String, String> queryParams) {
        String url = buildUrl(path, queryParams);
        HttpRequest request = requestBuilder(url)
                .GET()
                .build();
        return executeWithRetry(request, true);
    }

    /** Execute {@code POST <path>} with a JSON body and return the raw response body. */
    public String post(String path, String jsonBody) {
        String url = buildUrl(path, Map.of());
        HttpRequest request = requestBuilder(url)
                .POST(HttpRequest.BodyPublishers.ofString(jsonBody, StandardCharsets.UTF_8))
                .build();
        return executeWithRetry(request, false);
    }

    /** Execute {@code DELETE <path>} and return the raw response body. */
    public String delete(String path) {
        String url = buildUrl(path, Map.of());
        HttpRequest request = requestBuilder(url)
                .DELETE()
                .build();
        return executeWithRetry(request, true);
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    private HttpRequest.Builder requestBuilder(String url) {
        HttpRequest.Builder builder = HttpRequest.newBuilder()
                .uri(URI.create(url))
                .timeout(timeout)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json");
        if (authHeader != null && !authHeader.isBlank()) {
            builder.header("Authorization", authHeader);
        }
        return builder;
    }

    private String buildUrl(String path, Map<String, String> queryParams) {
        StringBuilder sb = new StringBuilder(baseUrl).append(path);
        if (queryParams != null && !queryParams.isEmpty()) {
            sb.append('?');
            boolean first = true;
            for (Map.Entry<String, String> e : queryParams.entrySet()) {
                if (!first) sb.append('&');
                sb.append(URLEncoder.encode(e.getKey(), StandardCharsets.UTF_8))
                  .append('=')
                  .append(URLEncoder.encode(e.getValue(), StandardCharsets.UTF_8));
                first = false;
            }
        }
        return sb.toString();
    }

    private String executeWithRetry(HttpRequest request, boolean retryOn503) {
        IOException lastIoe = null;
        for (int attempt = 0; attempt <= maxRetries; attempt++) {
            try {
                HttpResponse<String> response = httpClient.send(
                        request, HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8));
                int status = response.statusCode();
                String body = response.body() != null ? response.body() : "";

                if (status >= 200 && status < 300) {
                    return body;
                }

                // Retry on 503 for idempotent requests
                if (retryOn503 && status == 503 && attempt < maxRetries) {
                    continue;
                }

                throw HttpException.of(status, body);
            } catch (IOException e) {
                lastIoe = e;
                if (attempt >= maxRetries) break;
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new XiaoguaiException("HTTP request interrupted", e);
            }
        }
        throw new XiaoguaiException("Network error after " + (maxRetries + 1) + " attempt(s): " + lastIoe.getMessage(), lastIoe);
    }

    @Override
    public void close() {
        // HttpClient is a resource — shutdown executor if needed
        // Java 21 HttpClient does not implement Closeable; GC handles it
    }
}
