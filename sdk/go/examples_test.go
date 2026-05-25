package xiaoguai_test

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"

	xiaoguai "github.com/xiaoguai-agent/xiaoguai-go-client"
)

// ExampleClient_RecordOutcome demonstrates recording a business outcome.
func ExampleClient_RecordOutcome() {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]bool{"ok": true})
	}))
	defer srv.Close()

	client, _ := xiaoguai.NewClient(srv.URL, xiaoguai.WithToken("tok"))
	ok, err := client.RecordOutcome(context.Background(), xiaoguai.RecordOutcomeRequest{
		TenantID:  "my-tenant",
		AgentName: "sales-bot",
		Kind:      "revenue_usd",
		Value:     1200.0,
	})
	fmt.Println(ok, err)
	// Output:
	// true <nil>
}

// ExampleClient_UninstallSkill demonstrates the skill install/uninstall round-trip.
func ExampleClient_UninstallSkill() {
	installResp := map[string]interface{}{
		"id": "inst-42", "tenant_id": "my-tenant", "pack_slug": "rag-legal",
		"version": "1.0.0", "config": map[string]interface{}{}, "installed_at": "2026-05-25T00:00:00Z",
	}
	uninstallResp := map[string]string{"deleted": "inst-42"}

	var callCount int
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		callCount++
		if callCount == 1 {
			_ = json.NewEncoder(w).Encode(installResp)
		} else {
			_ = json.NewEncoder(w).Encode(uninstallResp)
		}
	}))
	defer srv.Close()

	client, _ := xiaoguai.NewClient(srv.URL, xiaoguai.WithToken("tok"))

	pack, err := client.InstallSkill(context.Background(), xiaoguai.InstallSkillRequest{
		TenantID: "my-tenant",
		PackSlug: "rag-legal",
	})
	if err != nil {
		panic(err)
	}
	fmt.Printf("installed %s (id=%s)\n", pack.PackSlug, pack.ID)

	deleted, err := client.UninstallSkill(context.Background(), pack.ID)
	if err != nil {
		panic(err)
	}
	fmt.Printf("uninstalled %s\n", deleted)
	// Output:
	// installed rag-legal (id=inst-42)
	// uninstalled inst-42
}

// ExampleClient_DeleteHotlPolicy demonstrates deleting a HOTL policy.
func ExampleClient_DeleteHotlPolicy() {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"ok": "true"})
	}))
	defer srv.Close()

	client, _ := xiaoguai.NewClient(srv.URL, xiaoguai.WithToken("tok"))
	err := client.DeleteHotlPolicy(context.Background(), "aaaa-bbbb-cccc-dddd")
	fmt.Println(err)
	// Output:
	// <nil>
}
