package io.github.xiaoguaiagent.client;

import io.github.xiaoguaiagent.client.internal.HttpExecutor;
import io.github.xiaoguaiagent.client.internal.JsonCodec;
import io.github.xiaoguaiagent.client.model.InstalledSkillPack;
import io.github.xiaoguaiagent.client.model.SkillCatalogEntry;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * API operations for the Skills (pack marketplace) subsystem.
 *
 * <p>Wraps {@code GET /v1/skills/installed}, {@code GET /v1/skills/catalog},
 * {@code POST /v1/skills/install}, and {@code DELETE /v1/skills/install/:id}.
 *
 * <p>Obtain an instance via {@link XiaoguaiClient#skills()}.
 */
public final class SkillsApi {

    private final HttpExecutor http;
    private final JsonCodec json;

    SkillsApi(HttpExecutor http, JsonCodec json) {
        this.http = http;
        this.json = json;
    }

    /**
     * List skill packs installed for a tenant.
     *
     * <p>Wraps {@code GET /v1/skills/installed?tenant=<tenant_id>}.
     *
     * @param tenantId optional — filter by tenant UUID (null = all tenants)
     * @return list of installed packs
     */
    public List<InstalledSkillPack> listInstalledSkills(String tenantId) {
        Map<String, String> params = new HashMap<>();
        if (tenantId != null) params.put("tenant", tenantId);
        String responseBody = http.get("/v1/skills/installed", params);
        return json.decodeList(responseBody, InstalledSkillPack.class);
    }

    /**
     * List all available skill packs from the built-in catalog.
     *
     * <p>Wraps {@code GET /v1/skills/catalog} (public, no auth required).
     *
     * @return list of catalog entries
     */
    public List<SkillCatalogEntry> listSkillCatalog() {
        String responseBody = http.get("/v1/skills/catalog", Map.of());
        // Catalog response wraps the array: {"version":1,"packs":[...]}
        Map<?, ?> wrapper = json.decode(responseBody, Map.class);
        Object packs = wrapper.get("packs");
        if (packs == null) return List.of();
        // Re-encode the packs array and decode as a typed list
        String packsJson = json.encode(packs);
        return json.decodeList(packsJson, SkillCatalogEntry.class);
    }

    /**
     * Install a skill pack for a tenant.
     *
     * <p>Wraps {@code POST /v1/skills/install}.
     *
     * @param tenantId tenant UUID
     * @param packSlug slug from the catalog (e.g. {@code "rag-legal"})
     * @param config   optional knob overrides (null = use defaults)
     * @return the created installation row
     * @throws io.github.xiaoguaiagent.client.error.NotFoundException if {@code packSlug} is unknown
     * @throws io.github.xiaoguaiagent.client.error.ConflictException if the pack is already installed
     */
    public InstalledSkillPack installSkill(String tenantId, String packSlug, Map<String, Object> config) {
        Objects.requireNonNull(tenantId, "tenantId must not be null");
        Objects.requireNonNull(packSlug, "packSlug must not be null");
        Map<String, Object> payload = new HashMap<>();
        payload.put("tenant_id", tenantId);
        payload.put("pack_slug", packSlug);
        payload.put("config", config != null ? config : Map.of());
        String responseBody = http.post("/v1/skills/install", json.encode(payload));
        return json.decode(responseBody, InstalledSkillPack.class);
    }

    /**
     * Install a skill pack with default config.
     *
     * @param tenantId tenant UUID
     * @param packSlug slug from the catalog
     * @return the created installation row
     */
    public InstalledSkillPack installSkill(String tenantId, String packSlug) {
        return installSkill(tenantId, packSlug, null);
    }

    /**
     * Uninstall a skill pack by its installation row ID.
     *
     * <p>Wraps {@code DELETE /v1/skills/install/:id}.
     *
     * @param installId UUID of the installation row
     * @return the deleted {@code installId}
     * @throws io.github.xiaoguaiagent.client.error.NotFoundException if the row is absent
     */
    public String uninstallSkill(String installId) {
        Objects.requireNonNull(installId, "installId must not be null");
        String responseBody = http.delete("/v1/skills/install/" + installId);
        Map<?, ?> resp = json.decode(responseBody, Map.class);
        Object deleted = resp.get("deleted");
        return deleted != null ? deleted.toString() : installId;
    }
}
