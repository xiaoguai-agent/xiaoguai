package xiaoguai

import (
	"context"
	"encoding/json"
	"fmt"
)

// ---------------------------------------------------------------------------
// HotL — boundary policy CRUD  (v1.2.3)
// ---------------------------------------------------------------------------

// ListHotlPolicies returns HOTL policies for tenantID, optionally filtered by scope.
// Wraps GET /v1/hotl/policies?tenant_id=<uuid>[&scope=<str>].
func (c *Client) ListHotlPolicies(ctx context.Context, tenantID string, opts ...ListOpt) ([]HotlPolicy, error) {
	if tenantID == "" {
		return nil, fmt.Errorf("xiaoguai: tenantID must not be empty")
	}
	p := applyListOpts(opts)
	params := map[string]string{"tenant_id": tenantID}
	if p.scope != nil {
		params["scope"] = *p.scope
	}
	data, err := c.get(ctx, "/v1/hotl/policies", params)
	if err != nil {
		return nil, err
	}
	var policies []HotlPolicy
	if err := json.Unmarshal(data, &policies); err != nil {
		return nil, fmt.Errorf("xiaoguai: decode hotl policies: %w", err)
	}
	if policies == nil {
		policies = []HotlPolicy{}
	}
	return policies, nil
}

// CreateHotlPolicy creates a new HOTL boundary policy.
// At least one of req.MaxCount or req.MaxUSD must be set (validated server-side).
// Wraps POST /v1/hotl/policies.
func (c *Client) CreateHotlPolicy(ctx context.Context, req CreateHotlPolicyRequest) (*HotlPolicy, error) {
	if req.TenantID == "" {
		return nil, fmt.Errorf("xiaoguai: CreateHotlPolicyRequest.TenantID must not be empty")
	}
	if req.Scope == "" {
		return nil, fmt.Errorf("xiaoguai: CreateHotlPolicyRequest.Scope must not be empty")
	}
	data, err := c.post(ctx, "/v1/hotl/policies", req)
	if err != nil {
		return nil, err
	}
	return decode[*HotlPolicy](data)
}

// DeleteHotlPolicy deletes a HOTL policy by policyID.
// Returns NotFoundError when the ID is unknown.
// Wraps DELETE /v1/hotl/policies/:id.
func (c *Client) DeleteHotlPolicy(ctx context.Context, policyID string) error {
	if policyID == "" {
		return fmt.Errorf("xiaoguai: policyID must not be empty")
	}
	_, err := c.delete(ctx, "/v1/hotl/policies/"+policyID)
	return err
}

// CheckHotl is a placeholder for POST /v1/hotl/check.
// The endpoint is not yet exposed by the server; the enforcer runs in-process
// on the session message path.
func (c *Client) CheckHotl(_ context.Context, _, _ string, _ float64) (*HotlVerdict, error) {
	return nil, fmt.Errorf("xiaoguai: CheckHotl: POST /v1/hotl/check is not yet exposed by the server; " +
		"budget checks run in-process when sending messages to a session")
}
