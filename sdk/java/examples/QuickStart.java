import io.github.xiaoguaiagent.client.XiaoguaiClient;
import io.github.xiaoguaiagent.client.error.ConflictException;
import io.github.xiaoguaiagent.client.error.HttpException;
import io.github.xiaoguaiagent.client.model.HotlPolicy;
import io.github.xiaoguaiagent.client.model.HotlVerdict;
import io.github.xiaoguaiagent.client.model.InstalledSkillPack;
import io.github.xiaoguaiagent.client.model.OutcomeSummary;
import io.github.xiaoguaiagent.client.model.OutcomeTimeseries;
import io.github.xiaoguaiagent.client.model.SkillCatalogEntry;

import java.time.Duration;
import java.util.List;
import java.util.Map;

/**
 * End-to-end usage example for the xiaoguai Java SDK.
 *
 * <p>Prerequisites:
 * <ul>
 *   <li>A running xiaoguai-api server (e.g. {@code cargo run -p xiaoguai-api})
 *   <li>A valid bearer token
 * </ul>
 *
 * <p>Run from the SDK root after {@code mvn package}:
 * <pre>
 *   java -cp target/xiaoguai-client-0.1.0.jar:examples QuickStart \
 *        http://localhost:8080 my-bearer-token tenant-uuid
 * </pre>
 */
public class QuickStart {

    public static void main(String[] args) {
        String baseUrl    = args.length > 0 ? args[0] : "http://localhost:8080";
        String token      = args.length > 1 ? args[1] : null;
        String tenantId   = args.length > 2 ? args[2] : "demo-tenant";

        // ----------------------------------------------------------------
        // 1. Build the client
        // ----------------------------------------------------------------
        try (XiaoguaiClient client = XiaoguaiClient.builder()
                .baseUrl(baseUrl)
                .bearerToken(token)
                .timeout(Duration.ofSeconds(15))
                .maxRetries(1)
                .build()) {

            System.out.println("=== xiaoguai Java SDK — QuickStart ===");
            System.out.printf("Server : %s%nTenant : %s%n%n", baseUrl, tenantId);

            // ----------------------------------------------------------------
            // 2. HotL — create and list a policy
            // ----------------------------------------------------------------
            System.out.println("--- HotL Policies ---");
            HotlPolicy policy = client.hotl().createHotlPolicy(
                    tenantId,
                    "llm_call",        // scope
                    3600,              // window_seconds
                    100,               // max_count
                    5.00,              // max_usd
                    "#ops-alerts"      // escalate_to
            );
            System.out.printf("Created policy: id=%s scope=%s window=%ds max_usd=%.2f%n",
                    policy.id(), policy.scope(), policy.windowSeconds(), policy.maxUsd());

            List<HotlPolicy> policies = client.hotl().listHotlPolicies(tenantId);
            System.out.printf("Active policies for tenant: %d%n%n", policies.size());

            // ----------------------------------------------------------------
            // 3. HotL — pre-flight budget check
            // ----------------------------------------------------------------
            System.out.println("--- HotL Check ---");
            try {
                HotlVerdict verdict = client.hotl().checkHotl(tenantId, "llm_call", 0.10);
                System.out.printf("Verdict: %s (allowed=%b)%n%n", verdict.verdict(), verdict.isAllowed());
            } catch (HttpException e) {
                // Endpoint may not be wired yet — continue
                System.out.printf("checkHotl not available: %s%n%n", e.getMessage());
            }

            // ----------------------------------------------------------------
            // 4. Outcomes — record and summarise
            // ----------------------------------------------------------------
            System.out.println("--- Outcomes ---");
            boolean ok = client.outcomes().recordOutcome(
                    tenantId,
                    "sales-bot",
                    "revenue_usd",
                    1200.0,
                    null,
                    "USD",
                    "Closed enterprise deal",
                    Map.of("region", "APAC", "product", "pro")
            );
            System.out.printf("Recorded outcome: ok=%b%n", ok);

            OutcomeSummary summary = client.outcomes().outcomesSummary(tenantId, "30d");
            System.out.printf("Summary over %s:%n", summary.range());
            if (summary.summary() != null && summary.summary().byKind() != null) {
                summary.summary().byKind().forEach((kind, bucket) ->
                        System.out.printf("  %-20s count=%d  sum=%.2f  avg=%.2f%n",
                                kind, bucket.count(), bucket.sum(), bucket.avg()));
            }

            OutcomeTimeseries ts = client.outcomes().outcomesTimeseries(tenantId, "7d", null);
            System.out.printf("Timeseries days: %d%n%n", ts.days().size());

            // ----------------------------------------------------------------
            // 5. Skills — catalog and install
            // ----------------------------------------------------------------
            System.out.println("--- Skills ---");
            List<SkillCatalogEntry> catalog = client.skills().listSkillCatalog();
            System.out.printf("Catalog entries: %d%n", catalog.size());
            catalog.stream().limit(3).forEach(e ->
                    System.out.printf("  [%s] %s (%s)%n", e.category(), e.name(), e.slug()));

            if (!catalog.isEmpty()) {
                String slug = catalog.get(0).slug();
                try {
                    InstalledSkillPack installed = client.skills().installSkill(tenantId, slug);
                    System.out.printf("Installed '%s' — install_id=%s%n", slug, installed.id());

                    // List installed and then uninstall
                    List<InstalledSkillPack> allInstalled = client.skills().listInstalledSkills(tenantId);
                    System.out.printf("Installed count: %d%n", allInstalled.size());

                    String deleted = client.skills().uninstallSkill(installed.id());
                    System.out.printf("Uninstalled: %s%n", deleted);
                } catch (ConflictException ce) {
                    System.out.printf("Pack '%s' already installed (409 conflict)%n", slug);
                }
            }

            // ----------------------------------------------------------------
            // 6. Cleanup — delete the policy we created
            // ----------------------------------------------------------------
            System.out.println();
            System.out.println("--- Cleanup ---");
            client.hotl().deleteHotlPolicy(policy.id());
            System.out.printf("Deleted policy %s%n", policy.id());
            System.out.println("\nQuickStart complete.");
        }
    }
}
