package io.github.xiaoguaiagent.client.error;

/** Raised for 429 responses — client has exceeded the server rate limit. */
public final class RateLimitException extends HttpException {
    public RateLimitException(int statusCode, String body) {
        super(statusCode, body);
    }
}
