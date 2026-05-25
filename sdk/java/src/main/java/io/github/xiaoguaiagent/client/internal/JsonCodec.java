package io.github.xiaoguaiagent.client.internal;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.JavaType;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.datatype.jdk8.Jdk8Module;
import io.github.xiaoguaiagent.client.error.XiaoguaiException;

import java.io.IOException;
import java.util.List;

/**
 * Thin wrapper around Jackson {@link ObjectMapper}.
 *
 * <p>A single shared instance is created per client — ObjectMapper is thread-safe
 * once configured.
 */
public final class JsonCodec {

    private final ObjectMapper mapper;

    public JsonCodec() {
        this.mapper = new ObjectMapper()
                .registerModule(new Jdk8Module())
                .configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false);
    }

    /** Deserialise a JSON string into the given type. */
    public <T> T decode(String json, Class<T> type) {
        try {
            return mapper.readValue(json, type);
        } catch (IOException e) {
            throw new XiaoguaiException("Failed to decode JSON into " + type.getSimpleName() + ": " + e.getMessage(), e);
        }
    }

    /** Deserialise a JSON array string into a {@link List} of the given element type. */
    public <T> List<T> decodeList(String json, Class<T> elementType) {
        try {
            JavaType listType = mapper.getTypeFactory().constructCollectionType(List.class, elementType);
            return mapper.readValue(json, listType);
        } catch (IOException e) {
            throw new XiaoguaiException("Failed to decode JSON list of " + elementType.getSimpleName() + ": " + e.getMessage(), e);
        }
    }

    /** Serialise an object to a JSON string. */
    public String encode(Object value) {
        try {
            return mapper.writeValueAsString(value);
        } catch (IOException e) {
            throw new XiaoguaiException("Failed to encode object to JSON: " + e.getMessage(), e);
        }
    }
}
