package io.github.xiaoguaiagent.client.error;

/** Raised for 409 responses — resource conflict (e.g. pack already installed). */
public final class ConflictException extends HttpException {
    public ConflictException(int statusCode, String body) {
        super(statusCode, body);
    }
}
