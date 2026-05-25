package io.github.xiaoguaiagent.client.error;

/** Raised for 5xx responses — server-side error. */
public final class ServerException extends HttpException {
    public ServerException(int statusCode, String body) {
        super(statusCode, body);
    }
}
