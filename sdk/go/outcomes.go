package xiaoguai

import (
	"context"
	"fmt"
)

// ---------------------------------------------------------------------------
// Outcomes — ROI telemetry  (v1.2.4)
// ---------------------------------------------------------------------------

// RecordOutcome records a business outcome attribution.
// Returns true on success.
// Raises ValidationError for invalid payloads (negative value, empty kind, etc.).
// Wraps POST /v1/outcomes.
func (c *Client) RecordOutcome(ctx context.Context, req RecordOutcomeRequest) (bool, error) {
	if req.TenantID == "" {
		return false, fmt.Errorf("xiaoguai: RecordOutcomeRequest.TenantID must not be empty")
	}
	if req.AgentName == "" {
		return false, fmt.Errorf("xiaoguai: RecordOutcomeRequest.AgentName must not be empty")
	}
	if req.Kind == "" {
		return false, fmt.Errorf("xiaoguai: RecordOutcomeRequest.Kind must not be empty")
	}
	if req.Metadata == nil {
		req.Metadata = map[string]interface{}{}
	}
	data, err := c.post(ctx, "/v1/outcomes", req)
	if err != nil {
		return false, err
	}
	type okResp struct {
		OK bool `json:"ok"`
	}
	resp, err := decode[okResp](data)
	if err != nil {
		return false, err
	}
	return resp.OK, nil
}

// OutcomesSummary returns aggregated ROI totals — one bucket per outcome kind.
// range accepts "24h", "7d", or "30d" (default "30d").
// Wraps GET /v1/outcomes/summary.
func (c *Client) OutcomesSummary(ctx context.Context, tenantID string, opts ...ListOpt) (*OutcomeSummary, error) {
	if tenantID == "" {
		return nil, fmt.Errorf("xiaoguai: tenantID must not be empty")
	}
	p := applyListOpts(opts)
	params := map[string]string{"tenant_id": tenantID}
	if p.rangeStr != nil {
		params["range"] = *p.rangeStr
	}
	data, err := c.get(ctx, "/v1/outcomes/summary", params)
	if err != nil {
		return nil, err
	}
	// The server wraps the by_kind map inside a "summary" envelope.
	type rawSummary struct {
		TenantID string `json:"tenant_id"`
		Range    string `json:"range"`
		Summary  struct {
			ByKind map[string]OutcomeSummaryBucket `json:"by_kind"`
		} `json:"summary"`
	}
	raw, err := decode[rawSummary](data)
	if err != nil {
		return nil, err
	}
	byKind := raw.Summary.ByKind
	if byKind == nil {
		byKind = map[string]OutcomeSummaryBucket{}
	}
	return &OutcomeSummary{
		TenantID: raw.TenantID,
		Range:    raw.Range,
		ByKind:   byKind,
	}, nil
}

// OutcomesTimeseries returns a daily time-series breakdown.
// Wraps GET /v1/outcomes/timeseries.
func (c *Client) OutcomesTimeseries(ctx context.Context, tenantID string, opts ...ListOpt) (*OutcomeTimeseries, error) {
	if tenantID == "" {
		return nil, fmt.Errorf("xiaoguai: tenantID must not be empty")
	}
	p := applyListOpts(opts)
	params := map[string]string{"tenant_id": tenantID}
	if p.rangeStr != nil {
		params["range"] = *p.rangeStr
	}
	if p.kind != nil {
		params["kind"] = *p.kind
	}
	data, err := c.get(ctx, "/v1/outcomes/timeseries", params)
	if err != nil {
		return nil, err
	}
	ts, err := decode[OutcomeTimeseries](data)
	if err != nil {
		return nil, err
	}
	if ts.Days == nil {
		ts.Days = []OutcomeDay{}
	}
	return &ts, nil
}
