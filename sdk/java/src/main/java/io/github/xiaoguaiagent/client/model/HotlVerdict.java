package io.github.xiaoguaiagent.client.model;

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonValue;

/**
 * Decision returned by the HOTL enforcer via {@code POST /v1/hotl/check}.
 *
 * <p>Note: a dedicated check endpoint is not yet wired on the server side.
 * Budget checks run in-process when sending messages to a session.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public record HotlVerdict(
        @JsonProperty("verdict") Kind verdict,
        /** Human-readable reason when verdict is {@code ESCALATE} or {@code DENY}. */
        @JsonProperty("reason") String reason
) {

    /** Whether this verdict allows the action. */
    public boolean isAllowed() {
        return verdict == Kind.ALLOW;
    }

    /** Whether this verdict denies the action. */
    public boolean isDenied() {
        return verdict == Kind.DENY;
    }

    /** The three possible HOTL verdict kinds. */
    public enum Kind {
        ALLOW("allow"),
        ESCALATE("escalate"),
        DENY("deny");

        private final String value;

        Kind(String value) {
            this.value = value;
        }

        @JsonValue
        public String value() {
            return value;
        }

        @JsonCreator
        public static Kind fromValue(String value) {
            for (Kind k : values()) {
                if (k.value.equalsIgnoreCase(value)) return k;
            }
            throw new IllegalArgumentException("Unknown verdict kind: " + value);
        }
    }
}
