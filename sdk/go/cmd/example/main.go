// Command example demonstrates end-to-end usage of the xiaoguai-go-client
// against a configured Xiaoguai API server.
//
// Usage:
//
//	XIAOGUAI_BASE_URL=http://localhost:8080 \
//	XIAOGUAI_TOKEN=my-bearer-token \
//	go run ./cmd/example
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"time"

	xiaoguai "github.com/xiaoguai-agent/xiaoguai-go-client"
)

func main() {
	baseURL := os.Getenv("XIAOGUAI_BASE_URL")
	if baseURL == "" {
		baseURL = "http://localhost:8080"
	}
	token := os.Getenv("XIAOGUAI_TOKEN")

	log.Printf("connecting to %s", baseURL)

	client, err := xiaoguai.NewClient(baseURL,
		xiaoguai.WithToken(token),
		xiaoguai.WithTimeout(15*time.Second),
	)
	if err != nil {
		log.Fatalf("new client: %v", err)
	}

	ctx := context.Background()
	tenantID := "example-tenant"

	// --- Skill catalog ---
	fmt.Println("\n=== Skill catalog ===")
	catalog, err := client.ListSkillCatalog(ctx)
	if err != nil {
		log.Printf("list catalog: %v", err)
	} else {
		fmt.Printf("catalog entries: %d\n", len(catalog))
		for _, p := range catalog {
			fmt.Printf("  %s  %s  [%s]\n", p.Slug, p.Version, p.Category)
		}
	}

	// --- Installed skills ---
	fmt.Println("\n=== Installed skills ===")
	installed, err := client.ListInstalledSkills(ctx, tenantID)
	if err != nil {
		log.Printf("list installed: %v", err)
	} else {
		fmt.Printf("installed packs: %d\n", len(installed))
	}

	// --- HotL policies ---
	fmt.Println("\n=== HotL policies ===")
	policies, err := client.ListHotlPolicies(ctx, tenantID)
	if err != nil {
		log.Printf("list policies: %v", err)
	} else {
		fmt.Printf("policies: %d\n", len(policies))
	}

	// --- Create a HotL policy ---
	maxCount := 200
	policy, err := client.CreateHotlPolicy(ctx, xiaoguai.CreateHotlPolicyRequest{
		TenantID:      tenantID,
		Scope:         "llm_call",
		WindowSeconds: 3600,
		MaxCount:      &maxCount,
	})
	if err != nil {
		log.Printf("create policy: %v", err)
	} else {
		fmt.Printf("created policy: %s  scope=%s\n", policy.ID, policy.Scope)

		// --- Delete it again ---
		if delErr := client.DeleteHotlPolicy(ctx, policy.ID); delErr != nil {
			log.Printf("delete policy: %v", delErr)
		} else {
			fmt.Printf("deleted policy: %s\n", policy.ID)
		}
	}

	// --- Record outcome ---
	fmt.Println("\n=== Record outcome ===")
	ok, err := client.RecordOutcome(ctx, xiaoguai.RecordOutcomeRequest{
		TenantID:  tenantID,
		AgentName: "example-agent",
		Kind:      "revenue_usd",
		Value:     42.50,
	})
	if err != nil {
		log.Printf("record outcome: %v", err)
	} else {
		fmt.Printf("outcome recorded: %v\n", ok)
	}

	// --- Outcomes summary ---
	fmt.Println("\n=== Outcomes summary ===")
	summary, err := client.OutcomesSummary(ctx, tenantID, xiaoguai.WithRange("30d"))
	if err != nil {
		log.Printf("outcomes summary: %v", err)
	} else {
		fmt.Printf("range: %s\n", summary.Range)
		for kind, bucket := range summary.ByKind {
			fmt.Printf("  %s: count=%d sum=%.2f\n", kind, bucket.Count, bucket.Sum)
		}
	}

	// --- Outcomes timeseries ---
	fmt.Println("\n=== Outcomes timeseries ===")
	ts, err := client.OutcomesTimeseries(ctx, tenantID, xiaoguai.WithRange("7d"))
	if err != nil {
		log.Printf("outcomes timeseries: %v", err)
	} else {
		fmt.Printf("days: %d\n", len(ts.Days))
	}

	fmt.Println("\ndone.")
}
