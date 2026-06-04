// Package gosdk_pact provides Pact consumer contract tests for the Go SDK
// against the xiaoguai wave-3 API.
//
// Consumer: go-sdk
// Provider: xiaoguai
// Pact spec: v3
//
// 12 interactions covering HotL CRUD + check, Outcomes (record/summary/timeseries),
// and Skills (list/install/uninstall) — identical surface to the TypeScript and
// Python SDK consumers for cross-SDK consistency.
package gosdk_pact_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/pact-foundation/pact-go/v2/consumer"
	"github.com/pact-foundation/pact-go/v2/matchers"
	"github.com/pact-foundation/pact-go/v2/models"
)

// ─────────────────────────────────────────────────────────────────────────────
// Shared constants
// ─────────────────────────────────────────────────────────────────────────────

const (
	tenantUUID  = "11111111-1111-1111-1111-111111111111"
	policyUUID  = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
	installUUID = "cccccccc-cccc-cccc-cccc-cccccccccccc"
	bearer      = "Bearer test-token"
)

// pactDir returns the canonical pacts/ directory two levels up from this file.
func pactDir(t *testing.T) string {
	t.Helper()
	_, filename, _, _ := runtime.Caller(0)
	dir := filepath.Join(filepath.Dir(filename), "..", "..", "pacts")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatalf("create pacts dir: %v", err)
	}
	return dir
}

// newMockProvider creates a PactV3 mock provider wired for the Go SDK consumer.
func newMockProvider(t *testing.T) *consumer.V3HTTPMockProvider {
	t.Helper()
	mock, err := consumer.NewV3Pact(consumer.MockHTTPProviderConfig{
		Consumer: "go-sdk",
		Provider: "xiaoguai",
		PactDir:  pactDir(t),
	})
	if err != nil {
		t.Fatalf("pact mock provider: %v", err)
	}
	return mock
}

// get is a convenience wrapper for a GET with Authorization header.
func get(t *testing.T, url string) *http.Response {
	t.Helper()
	req, _ := http.NewRequest(http.MethodGet, url, nil)
	req.Header.Set("Authorization", bearer)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("GET %s: %v", url, err)
	}
	return resp
}

// post is a convenience wrapper for a JSON POST with Authorization header.
func post(t *testing.T, url string, body any) *http.Response {
	t.Helper()
	b, _ := json.Marshal(body)
	req, _ := http.NewRequest(http.MethodPost, url, bytes.NewReader(b))
	req.Header.Set("Authorization", bearer)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("POST %s: %v", url, err)
	}
	return resp
}

// put is a convenience wrapper for a JSON PUT with Authorization header.
func put(t *testing.T, url string, body any) *http.Response {
	t.Helper()
	b, _ := json.Marshal(body)
	req, _ := http.NewRequest(http.MethodPut, url, bytes.NewReader(b))
	req.Header.Set("Authorization", bearer)
	req.Header.Set("Content-Type", "application/json")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("PUT %s: %v", url, err)
	}
	return resp
}

// del is a convenience wrapper for a DELETE with Authorization header.
func del(t *testing.T, url string) *http.Response {
	t.Helper()
	req, _ := http.NewRequest(http.MethodDelete, url, nil)
	req.Header.Set("Authorization", bearer)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("DELETE %s: %v", url, err)
	}
	return resp
}

// readJSON reads and discards the body (satisfying keep-alive) then unmarshals.
func readJSON(t *testing.T, resp *http.Response) map[string]any {
	t.Helper()
	defer resp.Body.Close()
	b, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	var out map[string]any
	_ = json.Unmarshal(b, &out)
	return out
}

// policyBody returns matcher maps for a HotlPolicy response.
func policyBody() map[string]any {
	uuidPattern := `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`
	return map[string]any{
		"id":             matchers.Term(policyUUID, uuidPattern),
		"tenant_id":      matchers.Term(tenantUUID, uuidPattern),
		"scope":          matchers.Like("llm_call"),
		"window_seconds": matchers.Like(3600),
		"max_count":      matchers.Like(100),
		"max_usd":        matchers.Like(5.0),
		"escalate_to":    matchers.Like("ops@example.com"),
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// HotL policies
// ─────────────────────────────────────────────────────────────────────────────

// TestListHotlPolicies — Interaction 1.
func TestListHotlPolicies(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("tenant has one HotL policy").
		UponReceiving("a GET /v1/hotl/policies request for tenant 11111111").
		WithRequest("GET", "/v1/hotl/policies", func(b *consumer.V3RequestBuilder) {
			b.Query("tenant_id", matchers.String(tenantUUID)).
				Header("Authorization", matchers.String(bearer))
		}).
		WillRespondWith(200, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(matchers.EachLike(policyBody(), 1))
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := get(t, fmt.Sprintf("%s/v1/hotl/policies?tenant_id=%s", base, tenantUUID))
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("expected 200, got %d", resp.StatusCode)
		}
		resp.Body.Close()
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestCreateHotlPolicy — Interaction 2.
func TestCreateHotlPolicy(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("HotL policy store is available").
		UponReceiving("a POST /v1/hotl/policies request").
		WithRequest("POST", "/v1/hotl/policies", func(b *consumer.V3RequestBuilder) {
			b.Header("Authorization", matchers.String(bearer)).
				Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{
					"tenant_id":      tenantUUID,
					"scope":          "llm_call",
					"window_seconds": 3600,
					"max_count":      100,
					"max_usd":        5.0,
					"escalate_to":    "ops@example.com",
				})
		}).
		WillRespondWith(201, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(policyBody())
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := post(t, fmt.Sprintf("%s/v1/hotl/policies", base), map[string]any{
			"tenant_id":      tenantUUID,
			"scope":          "llm_call",
			"window_seconds": 3600,
			"max_count":      100,
			"max_usd":        5.0,
			"escalate_to":    "ops@example.com",
		})
		if resp.StatusCode != http.StatusCreated {
			return fmt.Errorf("expected 201, got %d", resp.StatusCode)
		}
		body := readJSON(t, resp)
		if _, ok := body["id"]; !ok {
			return fmt.Errorf("expected 'id' in response")
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestGetHotlPolicy — Interaction 3.
func TestGetHotlPolicy(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		GivenWithParameter(models.ProviderState{Name: "HotL policy exists", Parameters: map[string]any{"id": policyUUID}}).
		UponReceiving(fmt.Sprintf("a GET /v1/hotl/policies/%s request", policyUUID)).
		WithRequest("GET", fmt.Sprintf("/v1/hotl/policies/%s", policyUUID),
			func(b *consumer.V3RequestBuilder) {
				b.Header("Authorization", matchers.String(bearer))
			}).
		WillRespondWith(200, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(policyBody())
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := get(t, fmt.Sprintf("%s/v1/hotl/policies/%s", base, policyUUID))
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("expected 200, got %d", resp.StatusCode)
		}
		resp.Body.Close()
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestUpdateHotlPolicy — Interaction 4.
func TestUpdateHotlPolicy(t *testing.T) {
	mock := newMockProvider(t)

	updatedBody := policyBody()
	updatedBody["window_seconds"] = matchers.Like(7200)
	updatedBody["max_count"] = matchers.Like(200)

	mock.
		AddInteraction().
		GivenWithParameter(models.ProviderState{Name: "HotL policy exists", Parameters: map[string]any{"id": policyUUID}}).
		UponReceiving(fmt.Sprintf("a PUT /v1/hotl/policies/%s request", policyUUID)).
		WithRequest("PUT", fmt.Sprintf("/v1/hotl/policies/%s", policyUUID),
			func(b *consumer.V3RequestBuilder) {
				b.Header("Authorization", matchers.String(bearer)).
					Header("Content-Type", matchers.String("application/json")).
					JSONBody(map[string]any{
						"tenant_id":      tenantUUID,
						"scope":          "llm_call",
						"window_seconds": 7200,
						"max_count":      200,
						"max_usd":        nil,
						"escalate_to":    nil,
					})
			}).
		WillRespondWith(200, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(updatedBody)
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := put(t, fmt.Sprintf("%s/v1/hotl/policies/%s", base, policyUUID), map[string]any{
			"tenant_id":      tenantUUID,
			"scope":          "llm_call",
			"window_seconds": 7200,
			"max_count":      200,
			"max_usd":        nil,
			"escalate_to":    nil,
		})
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("expected 200, got %d", resp.StatusCode)
		}
		resp.Body.Close()
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestDeleteHotlPolicy — Interaction 5.
func TestDeleteHotlPolicy(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		GivenWithParameter(models.ProviderState{Name: "HotL policy exists", Parameters: map[string]any{"id": policyUUID}}).
		UponReceiving(fmt.Sprintf("a DELETE /v1/hotl/policies/%s request", policyUUID)).
		WithRequest("DELETE", fmt.Sprintf("/v1/hotl/policies/%s", policyUUID),
			func(b *consumer.V3RequestBuilder) {
				b.Header("Authorization", matchers.String(bearer))
			}).
		WillRespondWith(204, func(_ *consumer.V3ResponseBuilder) {})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := del(t, fmt.Sprintf("%s/v1/hotl/policies/%s", base, policyUUID))
		if resp.StatusCode != http.StatusNoContent {
			return fmt.Errorf("expected 204, got %d", resp.StatusCode)
		}
		resp.Body.Close()
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestHotlCheckAllow — Interaction 6.
func TestHotlCheckAllow(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("tenant HotL policy exists and budget is within limits").
		UponReceiving("a POST /v1/hotl/check request within budget").
		WithRequest("POST", "/v1/hotl/check", func(b *consumer.V3RequestBuilder) {
			b.Header("Authorization", matchers.String(bearer)).
				Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{
					"tenant_id": tenantUUID,
					"scope":     "llm_call",
					"amount":    0.0025,
				})
		}).
		WillRespondWith(200, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{
					"verdict": matchers.Like("allow"),
					"reason":  nil,
				})
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := post(t, fmt.Sprintf("%s/v1/hotl/check", base), map[string]any{
			"tenant_id": tenantUUID,
			"scope":     "llm_call",
			"amount":    0.0025,
		})
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("expected 200, got %d", resp.StatusCode)
		}
		body := readJSON(t, resp)
		if body["verdict"] != "allow" {
			return fmt.Errorf("expected verdict=allow, got %v", body["verdict"])
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Outcomes
// ─────────────────────────────────────────────────────────────────────────────

// TestRecordOutcome — Interaction 7.
func TestRecordOutcome(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("outcome writer is available").
		UponReceiving("a POST /v1/outcomes request").
		WithRequest("POST", "/v1/outcomes", func(b *consumer.V3RequestBuilder) {
			b.Header("Authorization", matchers.String(bearer)).
				Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{
					"tenant_id":   "tenant_acme",
					"session_id":  "sess_abc123",
					"agent_name":  "sales-bot",
					"kind":        "revenue_usd",
					"value":       1250.0,
					"unit":        "usd",
					"description": "Closed deal D-4471",
					"metadata":    map[string]any{"deal_id": "D-4471"},
				})
		}).
		WillRespondWith(201, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{"ok": true})
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := post(t, fmt.Sprintf("%s/v1/outcomes", base), map[string]any{
			"tenant_id":   "tenant_acme",
			"session_id":  "sess_abc123",
			"agent_name":  "sales-bot",
			"kind":        "revenue_usd",
			"value":       1250.0,
			"unit":        "usd",
			"description": "Closed deal D-4471",
			"metadata":    map[string]any{"deal_id": "D-4471"},
		})
		if resp.StatusCode != http.StatusCreated {
			return fmt.Errorf("expected 201, got %d", resp.StatusCode)
		}
		body := readJSON(t, resp)
		if body["ok"] != true {
			return fmt.Errorf("expected ok=true")
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestOutcomesSummary — Interaction 8.
func TestOutcomesSummary(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("tenant has recorded outcomes").
		UponReceiving("a GET /v1/outcomes/summary request for 7d").
		WithRequest("GET", "/v1/outcomes/summary", func(b *consumer.V3RequestBuilder) {
			b.Query("tenant_id", matchers.String("tenant_acme")).
				Query("range", matchers.String("7d")).
				Header("Authorization", matchers.String(bearer))
		}).
		WillRespondWith(200, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{
					"tenant_id": matchers.Like("tenant_acme"),
					"range":     matchers.Like("7d"),
					"summary": matchers.Like(map[string]any{
						"by_kind": matchers.Like(map[string]any{
							"revenue_usd": map[string]any{
								"sum":   matchers.Like(42000.0),
								"count": matchers.Like(18),
								"avg":   matchers.Like(2333.33),
							},
						}),
					}),
				})
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := get(t, fmt.Sprintf("%s/v1/outcomes/summary?tenant_id=tenant_acme&range=7d", base))
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("expected 200, got %d", resp.StatusCode)
		}
		body := readJSON(t, resp)
		if _, ok := body["summary"]; !ok {
			return fmt.Errorf("missing 'summary' key")
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestOutcomesTimeseries — Interaction 9.
func TestOutcomesTimeseries(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("tenant has recorded outcomes").
		UponReceiving("a GET /v1/outcomes/timeseries request for 7d").
		WithRequest("GET", "/v1/outcomes/timeseries", func(b *consumer.V3RequestBuilder) {
			b.Query("tenant_id", matchers.String("tenant_acme")).
				Query("range", matchers.String("7d")).
				Header("Authorization", matchers.String(bearer))
		}).
		WillRespondWith(200, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{
					"tenant_id": matchers.Like("tenant_acme"),
					"range":     matchers.Like("7d"),
					"days": matchers.EachLike(map[string]any{
						"date":  matchers.Like("2026-05-20"),
						"kind":  matchers.Like("revenue_usd"),
						"sum":   matchers.Like(5000.0),
						"count": matchers.Like(2),
					}, 1),
				})
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := get(t, fmt.Sprintf("%s/v1/outcomes/timeseries?tenant_id=tenant_acme&range=7d", base))
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("expected 200, got %d", resp.StatusCode)
		}
		resp.Body.Close()
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Skills
// ─────────────────────────────────────────────────────────────────────────────

func installedPackBody() map[string]any {
	uuidPattern := `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`
	return map[string]any{
		"id":           matchers.Term(installUUID, uuidPattern),
		"tenant_id":    matchers.Like("tenant_acme"),
		"pack_slug":    matchers.Like("pr-review"),
		"version":      matchers.Like("1.0.0"),
		"config":       matchers.Like(map[string]any{}),
		"installed_at": matchers.Like("2026-05-25T12:34:56Z"),
	}
}

// TestListInstalledSkills — Interaction 10.
func TestListInstalledSkills(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("tenant has installed skill packs").
		UponReceiving("a GET /v1/skills/installed request").
		WithRequest("GET", "/v1/skills/installed", func(b *consumer.V3RequestBuilder) {
			b.Query("tenant_id", matchers.String("tenant_acme")).
				Header("Authorization", matchers.String(bearer))
		}).
		WillRespondWith(200, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(matchers.EachLike(installedPackBody(), 1))
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := get(t, fmt.Sprintf("%s/v1/skills/installed?tenant_id=tenant_acme", base))
		if resp.StatusCode != http.StatusOK {
			return fmt.Errorf("expected 200, got %d", resp.StatusCode)
		}
		resp.Body.Close()
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestInstallSkillPack — Interaction 11.
func TestInstallSkillPack(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		Given("skill pack pr-review exists in catalog").
		UponReceiving("a POST /v1/skills/install request").
		WithRequest("POST", "/v1/skills/install", func(b *consumer.V3RequestBuilder) {
			b.Header("Authorization", matchers.String(bearer)).
				Header("Content-Type", matchers.String("application/json")).
				JSONBody(map[string]any{
					"tenant_id": "tenant_acme",
					"pack_slug": "pr-review",
					"config":    map[string]any{},
				})
		}).
		WillRespondWith(201, func(b *consumer.V3ResponseBuilder) {
			b.Header("Content-Type", matchers.String("application/json")).
				JSONBody(installedPackBody())
		})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := post(t, fmt.Sprintf("%s/v1/skills/install", base), map[string]any{
			"tenant_id": "tenant_acme",
			"pack_slug": "pr-review",
			"config":    map[string]any{},
		})
		if resp.StatusCode != http.StatusCreated {
			return fmt.Errorf("expected 201, got %d", resp.StatusCode)
		}
		body := readJSON(t, resp)
		if body["pack_slug"] != "pr-review" {
			return fmt.Errorf("expected pack_slug=pr-review")
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

// TestUninstallSkillPack — Interaction 12.
func TestUninstallSkillPack(t *testing.T) {
	mock := newMockProvider(t)

	mock.
		AddInteraction().
		GivenWithParameter(models.ProviderState{Name: "skill pack installation exists", Parameters: map[string]any{"id": installUUID}}).
		UponReceiving(fmt.Sprintf("a DELETE /v1/skills/install/%s request", installUUID)).
		WithRequest("DELETE", fmt.Sprintf("/v1/skills/install/%s", installUUID),
			func(b *consumer.V3RequestBuilder) {
				b.Header("Authorization", matchers.String(bearer))
			}).
		WillRespondWith(204, func(_ *consumer.V3ResponseBuilder) {})

	if err := mock.ExecuteTest(t, func(cfg consumer.MockServerConfig) error {
		base := fmt.Sprintf("http://%s:%d", cfg.Host, cfg.Port)
		resp := del(t, fmt.Sprintf("%s/v1/skills/install/%s", base, installUUID))
		if resp.StatusCode != http.StatusNoContent {
			return fmt.Errorf("expected 204, got %d", resp.StatusCode)
		}
		resp.Body.Close()
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}
