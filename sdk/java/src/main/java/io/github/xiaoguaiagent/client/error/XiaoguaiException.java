package io.github.xiaoguaiagent.client.error;

/**
 * Base exception for all xiaoguai client errors.
 *
 * <p>Callers may catch {@code XiaoguaiException} to handle any SDK error, or catch
 * a specific sub-class to handle a particular error condition.
 */
public class XiaoguaiException extends RuntimeException {

    public XiaoguaiException(String message) {
        super(message);
    }

    public XiaoguaiException(String message, Throwable cause) {
        super(message, cause);
    }
}
