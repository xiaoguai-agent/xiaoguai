package io.github.xiaoguaiagent.client.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.List;
import java.util.Map;

/**
 * One entry in the skill catalog from {@code GET /v1/skills/catalog}.
 *
 * <p>Mirrors {@code SkillPackEntry} in
 * {@code crates/xiaoguai-api/src/skills.rs}.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public record SkillCatalogEntry(
        @JsonProperty("slug") String slug,
        @JsonProperty("name") String name,
        @JsonProperty("description") String description,
        @JsonProperty("version") String version,
        @JsonProperty("category") String category,
        /** Feature-flags and env-key prerequisites. */
        @JsonProperty("requires") Requires requires,
        /** User-configurable knobs and their JSON schemas. */
        @JsonProperty("knobs") Map<String, Object> knobs,
        @JsonProperty("screenshot_url") String screenshotUrl
) {

    /** Prerequisite declarations for a skill pack. */
    @JsonIgnoreProperties(ignoreUnknown = true)
    public record Requires(
            @JsonProperty("feature_flags") List<String> featureFlags,
            @JsonProperty("env_keys") List<String> envKeys
    ) {}
}
