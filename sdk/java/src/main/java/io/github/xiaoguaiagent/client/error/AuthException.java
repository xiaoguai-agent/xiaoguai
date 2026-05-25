package io.github.xiaoguaiagent.client.error;

/** Raised for 401 / 403 responses — invalid or missing credentials. */
public final class AuthException extends HttpException {
    public AuthException(int statusCode, String body) {
        super(statusCode, body);
    }
}
