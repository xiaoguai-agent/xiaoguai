package xiaoguai

import (
	"context"
	"encoding/json"
	"fmt"
)

// ---------------------------------------------------------------------------
// Skills — pack marketplace  (v1.2.28)
// ---------------------------------------------------------------------------

// ListInstalledSkills returns skill packs installed for tenantID.
// Wraps GET /v1/skills/installed?tenant=<tenant_id>.
func (c *Client) ListInstalledSkills(ctx context.Context, tenantID string) ([]InstalledSkillPack, error) {
	params := map[string]string{}
	if tenantID != "" {
		params["tenant"] = tenantID
	}
	data, err := c.get(ctx, "/v1/skills/installed", params)
	if err != nil {
		return nil, err
	}
	var packs []InstalledSkillPack
	if err := json.Unmarshal(data, &packs); err != nil {
		return nil, fmt.Errorf("xiaoguai: decode installed skills: %w", err)
	}
	if packs == nil {
		packs = []InstalledSkillPack{}
	}
	return packs, nil
}

// ListSkillCatalog returns all available skill packs from the built-in catalog.
// This endpoint is public (no auth required).
// Wraps GET /v1/skills/catalog.
func (c *Client) ListSkillCatalog(ctx context.Context) ([]SkillPackEntry, error) {
	data, err := c.get(ctx, "/v1/skills/catalog", nil)
	if err != nil {
		return nil, err
	}
	type catalogResp struct {
		Packs []SkillPackEntry `json:"packs"`
	}
	resp, err := decode[catalogResp](data)
	if err != nil {
		return nil, err
	}
	if resp.Packs == nil {
		resp.Packs = []SkillPackEntry{}
	}
	return resp.Packs, nil
}

// InstallSkill installs a skill pack for tenantID.
// packSlug must exist in the built-in catalog; unknown slugs return NotFoundError.
// Returns ConflictError when the pack is already installed for the tenant.
// Wraps POST /v1/skills/install.
func (c *Client) InstallSkill(ctx context.Context, req InstallSkillRequest) (*InstalledSkillPack, error) {
	if req.TenantID == "" {
		return nil, fmt.Errorf("xiaoguai: InstallSkillRequest.TenantID must not be empty")
	}
	if req.PackSlug == "" {
		return nil, fmt.Errorf("xiaoguai: InstallSkillRequest.PackSlug must not be empty")
	}
	if req.Config == nil {
		req.Config = map[string]interface{}{}
	}
	data, err := c.post(ctx, "/v1/skills/install", req)
	if err != nil {
		return nil, err
	}
	return decode[*InstalledSkillPack](data)
}

// UninstallSkill uninstalls a skill pack by its installation row installID.
// Returns the deleted installID on success.
// Returns NotFoundError when the row is absent.
// Wraps DELETE /v1/skills/install/:id.
func (c *Client) UninstallSkill(ctx context.Context, installID string) (string, error) {
	if installID == "" {
		return "", fmt.Errorf("xiaoguai: installID must not be empty")
	}
	data, err := c.delete(ctx, "/v1/skills/install/"+installID)
	if err != nil {
		return "", err
	}
	type deletedResp struct {
		Deleted string `json:"deleted"`
	}
	resp, err := decode[deletedResp](data)
	if err != nil {
		// Graceful fallback: return the installID if body is unexpected.
		return installID, nil
	}
	if resp.Deleted == "" {
		return installID, nil
	}
	return resp.Deleted, nil
}
