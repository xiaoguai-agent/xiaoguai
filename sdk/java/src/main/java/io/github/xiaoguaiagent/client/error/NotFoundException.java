package io.github.xiaoguaiagent.client.error;

/** Raised for 404 responses — the requested resource does not exist. */
public final class NotFoundException extends HttpException {
    public NotFoundException(int statusCode, String body) {
        super(statusCode, body);
    }
}
