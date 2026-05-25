package xiaoguai_test

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	xiaoguai "github.com/xiaoguai-agent/xiaoguai-go-client"
)

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

func newTestServer(t *testing.T, handler http.HandlerFunc) (*httptest.Server, *xiaoguai.Client) {
	t.Helper()
	srv := httptest.NewServer(handler)
	t.Cleanup(srv.Close)
	c, err := xiaoguai.NewClient(srv.URL, xiaoguai.WithToken("test-token"))
	require.NoError(t, err)
	return srv, c
}

func jsonHandler(t *testing.T, statusCode int, body interface{}) http.HandlerFunc {
	t.Helper()
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(statusCode)
		_ = json.NewEncoder(w).Encode(body)
	}
}

// ---------------------------------------------------------------------------
// HotL — ListHotlPolicies
// ---------------------------------------------------------------------------

var samplePolicy = map[string]interface{}{
	"id":             "aaaa-bbbb-cccc-dddd",
	"tenant_id":      "tenant-1",
	"scope":          "llm_call",
	"window_seconds": 3600,
	"max_count":      100,
	"escalate_to":    "ops@example.com",
}

func TestListHotlPolicies_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, []interface{}{samplePolicy}))
	policies, err := c.ListHotlPolicies(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Len(t, policies, 1)
	assert.Equal(t, "aaaa-bbbb-cccc-dddd", policies[0].ID)
	assert.Equal(t, "llm_call", policies[0].Scope)
	assert.Equal(t, 3600, policies[0].WindowSeconds)
}

func TestListHotlPolicies_EmptyResult(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, []interface{}{}))
	policies, err := c.ListHotlPolicies(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Empty(t, policies)
}

func TestListHotlPolicies_WithScope(t *testing.T) {
	var capturedQuery string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		capturedQuery = r.URL.RawQuery
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_ = json.NewEncoder(w).Encode([]interface{}{})
	}))
	t.Cleanup(srv.Close)
	c, err := xiaoguai.NewClient(srv.URL, xiaoguai.WithToken("tok"))
	require.NoError(t, err)

	_, err = c.ListHotlPolicies(context.Background(), "tenant-1", xiaoguai.WithScope("email"))
	require.NoError(t, err)
	assert.Contains(t, capturedQuery, "scope=email")
}

func TestListHotlPolicies_EmptyTenantID(t *testing.T) {
	c, err := xiaoguai.NewClient("http://localhost:9999")
	require.NoError(t, err)
	_, err = c.ListHotlPolicies(context.Background(), "")
	assert.ErrorContains(t, err, "tenantID")
}

func TestListHotlPolicies_404(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 404, map[string]string{"error": "not found"}))
	_, err := c.ListHotlPolicies(context.Background(), "tenant-1")
	require.Error(t, err)
	var nf *xiaoguai.NotFoundError
	assert.True(t, errors.As(err, &nf))
}

func TestListHotlPolicies_500Retries(t *testing.T) {
	calls := 0
	_, c := newTestServer(t, func(w http.ResponseWriter, _ *http.Request) {
		calls++
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(500)
		_ = json.NewEncoder(w).Encode(map[string]string{"error": "internal"})
	})
	_, err := c.ListHotlPolicies(context.Background(), "tenant-1")
	require.Error(t, err)
	var se *xiaoguai.ServerError
	assert.True(t, errors.As(err, &se))
	assert.Equal(t, 3, calls, "should retry 3 times")
}

// ---------------------------------------------------------------------------
// HotL — CreateHotlPolicy
// ---------------------------------------------------------------------------

func TestCreateHotlPolicy_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, samplePolicy))
	maxCount := 100
	policy, err := c.CreateHotlPolicy(context.Background(), xiaoguai.CreateHotlPolicyRequest{
		TenantID:      "tenant-1",
		Scope:         "llm_call",
		WindowSeconds: 3600,
		MaxCount:      &maxCount,
	})
	require.NoError(t, err)
	assert.Equal(t, "aaaa-bbbb-cccc-dddd", policy.ID)
}

func TestCreateHotlPolicy_ValidationError(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 400, map[string]string{"error": "max_count or max_usd required"}))
	_, err := c.CreateHotlPolicy(context.Background(), xiaoguai.CreateHotlPolicyRequest{
		TenantID:      "tenant-1",
		Scope:         "llm_call",
		WindowSeconds: 3600,
	})
	require.Error(t, err)
	var ve *xiaoguai.ValidationError
	assert.True(t, errors.As(err, &ve))
}

func TestCreateHotlPolicy_EmptyTenantID(t *testing.T) {
	c, err := xiaoguai.NewClient("http://localhost:9999")
	require.NoError(t, err)
	_, err = c.CreateHotlPolicy(context.Background(), xiaoguai.CreateHotlPolicyRequest{Scope: "x"})
	assert.ErrorContains(t, err, "TenantID")
}

// ---------------------------------------------------------------------------
// HotL — DeleteHotlPolicy
// ---------------------------------------------------------------------------

func TestDeleteHotlPolicy_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, map[string]string{"ok": "true"}))
	err := c.DeleteHotlPolicy(context.Background(), "aaaa-bbbb-cccc-dddd")
	require.NoError(t, err)
}

func TestDeleteHotlPolicy_NotFound(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 404, map[string]string{"error": "not found"}))
	err := c.DeleteHotlPolicy(context.Background(), "no-such-id")
	require.Error(t, err)
	var nf *xiaoguai.NotFoundError
	assert.True(t, errors.As(err, &nf))
}

// ---------------------------------------------------------------------------
// Outcomes — RecordOutcome
// ---------------------------------------------------------------------------

func TestRecordOutcome_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, map[string]bool{"ok": true}))
	ok, err := c.RecordOutcome(context.Background(), xiaoguai.RecordOutcomeRequest{
		TenantID:  "tenant-1",
		AgentName: "sales-bot",
		Kind:      "revenue_usd",
		Value:     1200.0,
	})
	require.NoError(t, err)
	assert.True(t, ok)
}

func TestRecordOutcome_ValidationError(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 422, map[string]string{"error": "invalid value"}))
	_, err := c.RecordOutcome(context.Background(), xiaoguai.RecordOutcomeRequest{
		TenantID:  "tenant-1",
		AgentName: "bot",
		Kind:      "revenue_usd",
		Value:     -1,
	})
	require.Error(t, err)
	var ve *xiaoguai.ValidationError
	assert.True(t, errors.As(err, &ve))
}

func TestRecordOutcome_EmptyKind(t *testing.T) {
	c, err := xiaoguai.NewClient("http://localhost:9999")
	require.NoError(t, err)
	_, err = c.RecordOutcome(context.Background(), xiaoguai.RecordOutcomeRequest{
		TenantID:  "tenant-1",
		AgentName: "bot",
	})
	assert.ErrorContains(t, err, "Kind")
}

// ---------------------------------------------------------------------------
// Outcomes — OutcomesSummary
// ---------------------------------------------------------------------------

var sampleSummaryResp = map[string]interface{}{
	"tenant_id": "tenant-1",
	"range":     "30d",
	"summary": map[string]interface{}{
		"by_kind": map[string]interface{}{
			"revenue_usd": map[string]interface{}{
				"count": 5,
				"sum":   6000.0,
				"avg":   1200.0,
			},
		},
	},
}

func TestOutcomesSummary_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, sampleSummaryResp))
	summary, err := c.OutcomesSummary(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Equal(t, "tenant-1", summary.TenantID)
	assert.Equal(t, "30d", summary.Range)
	bucket, ok := summary.ByKind["revenue_usd"]
	require.True(t, ok)
	assert.Equal(t, 5, bucket.Count)
	assert.InDelta(t, 6000.0, bucket.Sum, 0.01)
}

func TestOutcomesSummary_WithRange(t *testing.T) {
	var capturedQuery string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		capturedQuery = r.URL.RawQuery
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_ = json.NewEncoder(w).Encode(sampleSummaryResp)
	}))
	t.Cleanup(srv.Close)
	c, err := xiaoguai.NewClient(srv.URL)
	require.NoError(t, err)

	_, err = c.OutcomesSummary(context.Background(), "tenant-1", xiaoguai.WithRange("7d"))
	require.NoError(t, err)
	assert.Contains(t, capturedQuery, "range=7d")
}

// ---------------------------------------------------------------------------
// Outcomes — OutcomesTimeseries
// ---------------------------------------------------------------------------

var sampleTimeseriesResp = map[string]interface{}{
	"tenant_id": "tenant-1",
	"range":     "7d",
	"days": []interface{}{
		map[string]interface{}{"date": "2026-05-25", "kind": "revenue_usd", "count": 2, "sum": 2400.0},
	},
}

func TestOutcomesTimeseries_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, sampleTimeseriesResp))
	ts, err := c.OutcomesTimeseries(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Len(t, ts.Days, 1)
	assert.Equal(t, "2026-05-25", ts.Days[0].Date)
	assert.InDelta(t, 2400.0, ts.Days[0].Sum, 0.01)
}

func TestOutcomesTimeseries_EmptyDays(t *testing.T) {
	resp := map[string]interface{}{"tenant_id": "tenant-1", "range": "7d", "days": []interface{}{}}
	_, c := newTestServer(t, jsonHandler(t, 200, resp))
	ts, err := c.OutcomesTimeseries(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Empty(t, ts.Days)
}

// ---------------------------------------------------------------------------
// Skills — ListSkillCatalog
// ---------------------------------------------------------------------------

var sampleCatalogResp = map[string]interface{}{
	"version": 1,
	"packs": []interface{}{
		map[string]interface{}{
			"slug": "rag-legal", "name": "Legal RAG", "description": "Legal document QA",
			"version": "1.0.0", "category": "rag", "requires": map[string]interface{}{},
			"knobs": map[string]interface{}{}, "screenshot_url": nil,
		},
	},
}

func TestListSkillCatalog_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, sampleCatalogResp))
	packs, err := c.ListSkillCatalog(context.Background())
	require.NoError(t, err)
	assert.Len(t, packs, 1)
	assert.Equal(t, "rag-legal", packs[0].Slug)
	assert.Equal(t, "rag", packs[0].Category)
}

// ---------------------------------------------------------------------------
// Skills — ListInstalledSkills
// ---------------------------------------------------------------------------

var sampleInstalledPack = map[string]interface{}{
	"id": "inst-1", "tenant_id": "tenant-1", "pack_slug": "rag-legal",
	"version": "1.0.0", "config": map[string]interface{}{"top_k": 5},
	"installed_at": "2026-05-25T00:00:00Z",
}

func TestListInstalledSkills_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, []interface{}{sampleInstalledPack}))
	packs, err := c.ListInstalledSkills(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Len(t, packs, 1)
	assert.Equal(t, "rag-legal", packs[0].PackSlug)
}

func TestListInstalledSkills_Empty(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, []interface{}{}))
	packs, err := c.ListInstalledSkills(context.Background(), "tenant-x")
	require.NoError(t, err)
	assert.Empty(t, packs)
}

// ---------------------------------------------------------------------------
// Skills — InstallSkill
// ---------------------------------------------------------------------------

func TestInstallSkill_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, sampleInstalledPack))
	pack, err := c.InstallSkill(context.Background(), xiaoguai.InstallSkillRequest{
		TenantID: "tenant-1",
		PackSlug: "rag-legal",
	})
	require.NoError(t, err)
	assert.Equal(t, "rag-legal", pack.PackSlug)
}

func TestInstallSkill_NotFound(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 404, map[string]string{"error": "not found"}))
	_, err := c.InstallSkill(context.Background(), xiaoguai.InstallSkillRequest{
		TenantID: "tenant-1",
		PackSlug: "unknown-slug",
	})
	require.Error(t, err)
	var nf *xiaoguai.NotFoundError
	assert.True(t, errors.As(err, &nf))
}

func TestInstallSkill_AlreadyInstalled(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 409, map[string]string{"error": "pack already installed"}))
	_, err := c.InstallSkill(context.Background(), xiaoguai.InstallSkillRequest{
		TenantID: "tenant-1",
		PackSlug: "rag-legal",
	})
	require.Error(t, err)
	var ce *xiaoguai.ConflictError
	assert.True(t, errors.As(err, &ce))
}

// ---------------------------------------------------------------------------
// Skills — UninstallSkill
// ---------------------------------------------------------------------------

func TestUninstallSkill_HappyPath(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 200, map[string]string{"deleted": "inst-1"}))
	deleted, err := c.UninstallSkill(context.Background(), "inst-1")
	require.NoError(t, err)
	assert.Equal(t, "inst-1", deleted)
}

func TestUninstallSkill_NotFound(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 404, map[string]string{"error": "not found"}))
	_, err := c.UninstallSkill(context.Background(), "does-not-exist")
	require.Error(t, err)
	var nf *xiaoguai.NotFoundError
	assert.True(t, errors.As(err, &nf))
}

// ---------------------------------------------------------------------------
// Error type hierarchy
// ---------------------------------------------------------------------------

func TestErrorHierarchy(t *testing.T) {
	cases := []struct {
		status int
		target interface{ Unwrap() error }
	}{
		{401, &xiaoguai.AuthError{}},
		{404, &xiaoguai.NotFoundError{}},
		{400, &xiaoguai.ValidationError{}},
		{409, &xiaoguai.ConflictError{}},
		{429, &xiaoguai.RateLimitError{}},
	}
	for _, tc := range cases {
		_, c := newTestServer(t, jsonHandler(t, tc.status, map[string]string{"error": "test"}))
		_, err := c.ListHotlPolicies(context.Background(), "t")
		require.Error(t, err)
		var base *xiaoguai.HTTPError
		assert.True(t, errors.As(err, &base), "expected HTTPError base for %d", tc.status)
	}
}

func TestGeneric5xxRaisesServerError(t *testing.T) {
	_, c := newTestServer(t, jsonHandler(t, 500, map[string]string{"error": "internal"}))
	_, err := c.ListHotlPolicies(context.Background(), "tenant-1")
	require.Error(t, err)
	var se *xiaoguai.ServerError
	assert.True(t, errors.As(err, &se))
	assert.Equal(t, 500, se.StatusCode)
}

// ---------------------------------------------------------------------------
// Context cancellation
// ---------------------------------------------------------------------------

func TestContextCancel(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(500 * time.Millisecond)
		w.WriteHeader(200)
		_, _ = w.Write([]byte("[]"))
	}))
	t.Cleanup(srv.Close)

	c, err := xiaoguai.NewClient(srv.URL)
	require.NoError(t, err)

	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()

	_, err = c.ListHotlPolicies(ctx, "tenant-1")
	require.Error(t, err)
}

// ---------------------------------------------------------------------------
// Custom HTTP client injection
// ---------------------------------------------------------------------------

func TestCustomHTTPClient(t *testing.T) {
	srv := httptest.NewServer(jsonHandler(t, 200, []interface{}{samplePolicy}))
	t.Cleanup(srv.Close)

	custom := &http.Client{Timeout: 5 * time.Second}
	c, err := xiaoguai.NewClient(srv.URL, xiaoguai.WithHTTPClient(custom))
	require.NoError(t, err)

	policies, err := c.ListHotlPolicies(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Len(t, policies, 1)
}

// ---------------------------------------------------------------------------
// Auth header
// ---------------------------------------------------------------------------

func TestAuthHeaderIsSent(t *testing.T) {
	var capturedAuth string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		capturedAuth = r.Header.Get("Authorization")
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = w.Write([]byte("[]"))
	}))
	t.Cleanup(srv.Close)

	c, err := xiaoguai.NewClient(srv.URL, xiaoguai.WithToken("my-secret-token"))
	require.NoError(t, err)
	_, err = c.ListHotlPolicies(context.Background(), "tenant-1")
	require.NoError(t, err)
	assert.Equal(t, "Bearer my-secret-token", capturedAuth)
}

// ---------------------------------------------------------------------------
// NewClient validation
// ---------------------------------------------------------------------------

func TestNewClient_EmptyBaseURL(t *testing.T) {
	_, err := xiaoguai.NewClient("")
	require.Error(t, err)
}

// ---------------------------------------------------------------------------
// HotlVerdict helpers
// ---------------------------------------------------------------------------

func TestHotlVerdictAllowed(t *testing.T) {
	v := xiaoguai.HotlVerdict{Verdict: "allow"}
	assert.True(t, v.Allowed())
	assert.False(t, v.Denied())
}

func TestHotlVerdictDenied(t *testing.T) {
	reason := "budget exceeded"
	v := xiaoguai.HotlVerdict{Verdict: "deny", Reason: &reason}
	assert.True(t, v.Denied())
	assert.False(t, v.Allowed())
}
