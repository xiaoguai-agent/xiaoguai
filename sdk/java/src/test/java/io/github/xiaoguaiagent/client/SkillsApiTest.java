package io.github.xiaoguaiagent.client;

import com.github.tomakehurst.wiremock.junit5.WireMockExtension;
import io.github.xiaoguaiagent.client.error.ConflictException;
import io.github.xiaoguaiagent.client.error.HttpException;
import io.github.xiaoguaiagent.client.error.NotFoundException;
import io.github.xiaoguaiagent.client.model.InstalledSkillPack;
import io.github.xiaoguaiagent.client.model.SkillCatalogEntry;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.RegisterExtension;

import java.time.Duration;
import java.util.List;
import java.util.Map;

import static com.github.tomakehurst.wiremock.client.WireMock.*;
import static com.github.tomakehurst.wiremock.core.WireMockConfiguration.wireMockConfig;
import static org.assertj.core.api.Assertions.*;

/**
 * Tests for {@link SkillsApi} — catalog, install, uninstall, and list operations.
 */
class SkillsApiTest {

    @RegisterExtension
    static WireMockExtension wm = WireMockExtension.newInstance()
            .options(wireMockConfig().dynamicPort())
            .build();

    private XiaoguaiClient client;

    private static final String INSTALLED_JSON =
            "{\"id\":\"inst-1\",\"tenant_id\":\"t1\",\"pack_slug\":\"rag-legal\"," +
            "\"version\":\"1.2.0\",\"config\":{},\"installed_at\":\"2026-05-25T09:00:00Z\"}";

    private static final String CATALOG_JSON =
            "{\"version\":1,\"packs\":[{\"slug\":\"rag-legal\",\"name\":\"Legal RAG\"," +
            "\"description\":\"Legal document retrieval\",\"version\":\"1.2.0\"," +
            "\"category\":\"retrieval\",\"requires\":null,\"knobs\":null,\"screenshot_url\":null}]}";

    @BeforeEach
    void setUp() {
        client = XiaoguaiClient.builder()
                .baseUrl(wm.baseUrl())
                .bearerToken("tok")
                .timeout(Duration.ofSeconds(5))
                .maxRetries(0)
                .build();
    }

    @AfterEach
    void tearDown() { client.close(); }

    // ---- listInstalledSkills ----

    @Test
    void listInstalledSkills_happyPath_returnsParsedList() {
        wm.stubFor(get(urlPathEqualTo("/v1/skills/installed"))
                .withQueryParam("tenant", equalTo("t1"))
                .willReturn(okJson("[" + INSTALLED_JSON + "]")));

        List<InstalledSkillPack> packs = client.skills().listInstalledSkills("t1");

        assertThat(packs).hasSize(1);
        InstalledSkillPack p = packs.get(0);
        assertThat(p.id()).isEqualTo("inst-1");
        assertThat(p.tenantId()).isEqualTo("t1");
        assertThat(p.packSlug()).isEqualTo("rag-legal");
        assertThat(p.version()).isEqualTo("1.2.0");
        assertThat(p.installedAt()).isEqualTo("2026-05-25T09:00:00Z");
    }

    @Test
    void listInstalledSkills_noTenantFilter_omitsQueryParam() {
        wm.stubFor(get(urlPathEqualTo("/v1/skills/installed"))
                .willReturn(okJson("[]")));

        assertThat(client.skills().listInstalledSkills(null)).isEmpty();
        wm.verify(getRequestedFor(urlPathEqualTo("/v1/skills/installed"))
                .withoutQueryParam("tenant"));
    }

    @Test
    void listInstalledSkills_emptyList_returnsEmptyList() {
        wm.stubFor(get(urlPathEqualTo("/v1/skills/installed"))
                .willReturn(okJson("[]")));

        assertThat(client.skills().listInstalledSkills("t1")).isEmpty();
    }

    // ---- listSkillCatalog ----

    @Test
    void listSkillCatalog_happyPath_returnsParsedEntries() {
        wm.stubFor(get(urlPathEqualTo("/v1/skills/catalog"))
                .willReturn(okJson(CATALOG_JSON)));

        List<SkillCatalogEntry> entries = client.skills().listSkillCatalog();

        assertThat(entries).hasSize(1);
        SkillCatalogEntry e = entries.get(0);
        assertThat(e.slug()).isEqualTo("rag-legal");
        assertThat(e.name()).isEqualTo("Legal RAG");
        assertThat(e.category()).isEqualTo("retrieval");
    }

    @Test
    void listSkillCatalog_emptyPacks_returnsEmptyList() {
        wm.stubFor(get(urlPathEqualTo("/v1/skills/catalog"))
                .willReturn(okJson("{\"version\":1,\"packs\":[]}")));

        assertThat(client.skills().listSkillCatalog()).isEmpty();
    }

    @Test
    void listSkillCatalog_500Response_throwsHttpException() {
        wm.stubFor(get(urlPathEqualTo("/v1/skills/catalog"))
                .willReturn(serverError()));

        assertThatThrownBy(() -> client.skills().listSkillCatalog())
                .isInstanceOf(HttpException.class)
                .satisfies(e -> assertThat(((HttpException) e).statusCode()).isEqualTo(500));
    }

    // ---- installSkill ----

    @Test
    void installSkill_happyPath_returnsInstalledPack() {
        wm.stubFor(post(urlPathEqualTo("/v1/skills/install"))
                .withRequestBody(matchingJsonPath("$.tenant_id", equalTo("t1")))
                .withRequestBody(matchingJsonPath("$.pack_slug", equalTo("rag-legal")))
                .willReturn(okJson(INSTALLED_JSON)));

        InstalledSkillPack pack = client.skills().installSkill("t1", "rag-legal");
        assertThat(pack.id()).isEqualTo("inst-1");
        assertThat(pack.packSlug()).isEqualTo("rag-legal");
    }

    @Test
    void installSkill_withConfig_sendsConfigInPayload() {
        wm.stubFor(post(urlPathEqualTo("/v1/skills/install"))
                .withRequestBody(matchingJsonPath("$.config.max_docs", equalTo("50")))
                .willReturn(okJson(INSTALLED_JSON)));

        InstalledSkillPack pack = client.skills().installSkill(
                "t1", "rag-legal", Map.of("max_docs", 50));
        assertThat(pack).isNotNull();
    }

    @Test
    void installSkill_alreadyInstalled_throwsConflictException() {
        wm.stubFor(post(urlPathEqualTo("/v1/skills/install"))
                .willReturn(aResponse().withStatus(409).withBody("{\"error\":\"already installed\"}")));

        assertThatThrownBy(() -> client.skills().installSkill("t1", "rag-legal"))
                .isInstanceOf(ConflictException.class)
                .satisfies(e -> assertThat(((ConflictException) e).statusCode()).isEqualTo(409));
    }

    @Test
    void installSkill_unknownSlug_throwsNotFoundException() {
        wm.stubFor(post(urlPathEqualTo("/v1/skills/install"))
                .willReturn(notFound().withBody("{\"error\":\"pack not found\"}")));

        assertThatThrownBy(() -> client.skills().installSkill("t1", "nonexistent-pack"))
                .isInstanceOf(NotFoundException.class);
    }

    @Test
    void installSkill_nullTenantId_throwsNPE() {
        assertThatNullPointerException()
                .isThrownBy(() -> client.skills().installSkill(null, "slug"))
                .withMessageContaining("tenantId");
    }

    // ---- uninstallSkill ----

    @Test
    void uninstallSkill_happyPath_returnsDeletedId() {
        wm.stubFor(delete(urlPathEqualTo("/v1/skills/install/inst-1"))
                .willReturn(okJson("{\"deleted\":\"inst-1\"}")));

        String deleted = client.skills().uninstallSkill("inst-1");
        assertThat(deleted).isEqualTo("inst-1");
    }

    @Test
    void uninstallSkill_notFound_throwsNotFoundException() {
        wm.stubFor(delete(urlPathEqualTo("/v1/skills/install/bad-id"))
                .willReturn(notFound().withBody("{\"error\":\"not found\"}")));

        assertThatThrownBy(() -> client.skills().uninstallSkill("bad-id"))
                .isInstanceOf(NotFoundException.class);
    }

    @Test
    void uninstallSkill_nullInstallId_throwsNPE() {
        assertThatNullPointerException()
                .isThrownBy(() -> client.skills().uninstallSkill(null))
                .withMessageContaining("installId");
    }

    @Test
    void uninstallSkill_500Response_throwsServerException() {
        wm.stubFor(delete(urlPathEqualTo("/v1/skills/install/inst-x"))
                .willReturn(serverError()));

        assertThatThrownBy(() -> client.skills().uninstallSkill("inst-x"))
                .isInstanceOf(HttpException.class)
                .satisfies(e -> assertThat(((HttpException) e).statusCode()).isEqualTo(500));
    }
}
