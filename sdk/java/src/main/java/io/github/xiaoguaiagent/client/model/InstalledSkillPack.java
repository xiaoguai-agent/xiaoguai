package io.github.xiaoguaiagent.client.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.annotation.JsonProperty;
import java.util.Map;

/**
 * One installed-pack row from {@code GET /v1/skills/installed}.
 *
 * <p>Mirrors {@code InstalledPackRow} in
 * {@code crates/xiaoguai-api/src/skills.rs}.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public record InstalledSkillPack(
        @JsonProperty("id") String id,
        @JsonProperty("tenant_id") String tenantId,
        @JsonProperty("pack_slug") String packSlug,
        @JsonProperty("version") String version,
        /** Config knobs supplied at install time. */
        @JsonProperty("config") Map<String, Object> config,
        /** ISO-8601 timestamp when the pack was installed. */
        @JsonProperty("installed_at") String installedAt
) {}
