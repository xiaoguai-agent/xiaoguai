package io.github.xiaoguaiagent.client.error;

/**
 * Raised when the API returns a non-2xx HTTP status code.
 *
 * <p>Use {@link #statusCode()} to inspect the raw HTTP status.
 * {@link #body()} contains the raw response body string (may be empty).
 *
 * <p>For common error conditions prefer catching the more-specific sub-classes:
 * {@link AuthException}, {@link NotFoundException}, {@link ConflictException},
 * {@link RateLimitException}, {@link ServerException}.
 */
public sealed class HttpException extends XiaoguaiException
        permits AuthException, NotFoundException, ConflictException,
                RateLimitException, ServerException {

    private final int statusCode;
    private final String body;

    public HttpException(int statusCode, String body) {
        super("HTTP " + statusCode + ": " + extractMessage(body));
        this.statusCode = statusCode;
        this.body = body;
    }

    /** HTTP status code of the response. */
    public int statusCode() {
        return statusCode;
    }

    /** Raw response body (may be empty or partial JSON). */
    public String body() {
        return body;
    }

    /**
     * Factory — maps a status code to the most-specific sub-class.
     *
     * @param statusCode HTTP response status
     * @param body       raw response body string
     * @return a {@link HttpException} or sub-class instance
     */
    public static HttpException of(int statusCode, String body) {
        return switch (statusCode) {
            case 401, 403 -> new AuthException(statusCode, body);
            case 404 -> new NotFoundException(statusCode, body);
            case 409 -> new ConflictException(statusCode, body);
            case 429 -> new RateLimitException(statusCode, body);
            default -> statusCode >= 500
                    ? new ServerException(statusCode, body)
                    : new HttpException(statusCode, body);
        };
    }

    private static String extractMessage(String body) {
        if (body == null || body.isBlank()) return "(empty body)";
        // Try to extract {"error":"..."} or {"message":"..."} without a full JSON parse
        for (String key : new String[]{" \"error\":", "\"error\":", "\"message\":"}) {
            int idx = body.indexOf(key);
            if (idx >= 0) {
                int start = body.indexOf('"', idx + key.length());
                int end = body.indexOf('"', start + 1);
                if (start >= 0 && end > start) return body.substring(start + 1, end);
            }
        }
        return body.length() > 120 ? body.substring(0, 120) + "…" : body;
    }
}
