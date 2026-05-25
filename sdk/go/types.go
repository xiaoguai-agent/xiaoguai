package xiaoguai

// ---------------------------------------------------------------------------
// HotL types
// ---------------------------------------------------------------------------

// HotlPolicy is one row from GET /v1/hotl/policies.
// Fields mirror the Rust wire type in crates/xiaoguai-api/src/hotl/policy.rs.
type HotlPolicy struct {
	ID            string   `json:"id"`
	TenantID      string   `json:"tenant_id"`
	Scope         string   `json:"scope"`
	WindowSeconds int      `json:"window_seconds"`
	MaxCount      *int     `json:"max_count,omitempty"`
	MaxUSD        *float64 `json:"max_usd,omitempty"`
	EscalateTo    *string  `json:"escalate_to,omitempty"`
}

// CreateHotlPolicyRequest is the body for POST /v1/hotl/policies.
// At least one of MaxCount or MaxUSD must be set (validated server-side).
type CreateHotlPolicyRequest struct {
	TenantID      string   `json:"tenant_id"`
	Scope         string   `json:"scope"`
	WindowSeconds int      `json:"window_seconds"`
	MaxCount      *int     `json:"max_count,omitempty"`
	MaxUSD        *float64 `json:"max_usd,omitempty"`
	EscalateTo    *string  `json:"escalate_to,omitempty"`
}

// HotlVerdict is the decision returned by POST /v1/hotl/check.
// Verdict is one of "allow", "escalate", or "deny".
type HotlVerdict struct {
	Verdict string  `json:"verdict"`
	Reason  *string `json:"reason,omitempty"`
}

// Allowed returns true when the verdict is "allow".
func (v HotlVerdict) Allowed() bool { return v.Verdict == "allow" }

// Denied returns true when the verdict is "deny".
func (v HotlVerdict) Denied() bool { return v.Verdict == "deny" }

// ---------------------------------------------------------------------------
// Outcomes types
// ---------------------------------------------------------------------------

// RecordOutcomeRequest is the body for POST /v1/outcomes.
type RecordOutcomeRequest struct {
	TenantID    string                 `json:"tenant_id"`
	AgentName   string                 `json:"agent_name"`
	Kind        string                 `json:"kind"`
	Value       float64                `json:"value"`
	SessionID   *string                `json:"session_id,omitempty"`
	Unit        *string                `json:"unit,omitempty"`
	Description *string                `json:"description,omitempty"`
	Metadata    map[string]interface{} `json:"metadata"`
}

// OutcomeSummaryBucket holds aggregated totals for one outcome kind.
type OutcomeSummaryBucket struct {
	Kind  string  `json:"kind"`
	Count int     `json:"count"`
	Sum   float64 `json:"sum"`
	Avg   float64 `json:"avg"`
}

// OutcomeSummary is the response from GET /v1/outcomes/summary.
type OutcomeSummary struct {
	TenantID string                          `json:"tenant_id"`
	Range    string                          `json:"range"`
	ByKind   map[string]OutcomeSummaryBucket `json:"by_kind"`
}

// OutcomeDay is one day bucket from GET /v1/outcomes/timeseries.
type OutcomeDay struct {
	Date  string  `json:"date"`
	Kind  string  `json:"kind"`
	Count int     `json:"count"`
	Sum   float64 `json:"sum"`
}

// OutcomeTimeseries is the response from GET /v1/outcomes/timeseries.
type OutcomeTimeseries struct {
	TenantID string       `json:"tenant_id"`
	Range    string       `json:"range"`
	Days     []OutcomeDay `json:"days"`
}

// ---------------------------------------------------------------------------
// Skills types
// ---------------------------------------------------------------------------

// InstalledSkillPack is one row from GET /v1/skills/installed.
type InstalledSkillPack struct {
	ID          string                 `json:"id"`
	TenantID    string                 `json:"tenant_id"`
	PackSlug    string                 `json:"pack_slug"`
	Version     string                 `json:"version"`
	Config      map[string]interface{} `json:"config"`
	InstalledAt string                 `json:"installed_at"`
}

// SkillPackEntry is one entry from GET /v1/skills/catalog.
type SkillPackEntry struct {
	Slug          string                 `json:"slug"`
	Name          string                 `json:"name"`
	Description   string                 `json:"description"`
	Version       string                 `json:"version"`
	Category      string                 `json:"category"`
	Requires      map[string]interface{} `json:"requires"`
	Knobs         map[string]interface{} `json:"knobs"`
	ScreenshotURL *string                `json:"screenshot_url,omitempty"`
}

// InstallSkillRequest is the body for POST /v1/skills/install.
type InstallSkillRequest struct {
	TenantID string                 `json:"tenant_id"`
	PackSlug string                 `json:"pack_slug"`
	Config   map[string]interface{} `json:"config"`
}

// ---------------------------------------------------------------------------
// List options
// ---------------------------------------------------------------------------

// ListOpt is a functional option for list methods that support optional filters.
type ListOpt func(*listParams)

type listParams struct {
	scope    *string
	tenantID *string
	rangeStr *string
	kind     *string
}

// WithScope filters list results by scope.
func WithScope(scope string) ListOpt {
	return func(p *listParams) { p.scope = &scope }
}

// WithRange sets the time range filter for summary / timeseries methods.
// Accepts "24h", "7d", or "30d".
func WithRange(r string) ListOpt {
	return func(p *listParams) { p.rangeStr = &r }
}

// WithKind filters timeseries results to a single outcome kind.
func WithKind(kind string) ListOpt {
	return func(p *listParams) { p.kind = &kind }
}

func applyListOpts(opts []ListOpt) *listParams {
	p := &listParams{}
	for _, o := range opts {
		o(p)
	}
	return p
}
