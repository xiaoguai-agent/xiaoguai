"""Wave-3 SDK client tests.

Uses ``httpx.MockTransport`` to intercept requests without a running server.
All tests are marked ``unit`` — no network, no binary required.

Coverage:
  - Happy-path for every implemented method
  - 404 → XiaoguaiNotFoundError
  - 400 → XiaoguaiValidationError
  - 409 → XiaoguaiConflictError
  - 503 → XiaoguaiUnavailableError
  - Model dataclass construction / field access
"""

from __future__ import annotations

import json
from typing import Any, Dict, Optional

import pytest

try:
    import httpx
except ImportError:
    pytest.skip("httpx not installed — skip wave-3 client tests", allow_module_level=True)

from xiaoguai.client import (
    HotlPolicy,
    HotlVerdict,
    InstalledSkillPack,
    OutcomeSummary,
    OutcomeTimeseries,
    SkillPackEntry,
    XiaoguaiClient,
    XiaoguaiConflictError,
    XiaoguaiHTTPError,
    XiaoguaiNotFoundError,
    XiaoguaiUnavailableError,
    XiaoguaiValidationError,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_POLICY_PAYLOAD: Dict[str, Any] = {
    "id": "aaaa-bbbb-cccc-dddd",
    "tenant_id": "tenant-1",
    "scope": "llm_call",
    "window_seconds": 3600,
    "max_count": 100,
    "max_usd": None,
    "escalate_to": "ops@example.com",
}

_INSTALLED_PACK_PAYLOAD: Dict[str, Any] = {
    "id": "inst-1",
    "tenant_id": "tenant-1",
    "pack_slug": "rag-legal",
    "version": "1.0.0",
    "config": {"top_k": 5},
    "installed_at": "2026-05-25T00:00:00Z",
}

_CATALOG_PAYLOAD: Dict[str, Any] = {
    "version": 1,
    "packs": [
        {
            "slug": "rag-legal",
            "name": "Legal RAG",
            "description": "Legal document QA",
            "version": "1.0.0",
            "category": "rag",
            "requires": {},
            "knobs": {},
            "screenshot_url": None,
        }
    ],
}

_SUMMARY_PAYLOAD: Dict[str, Any] = {
    "tenant_id": "tenant-1",
    "range": "7d",
    "summary": {
        "by_kind": {
            "revenue_usd": {"count": 3, "sum": 3600.0, "avg": 1200.0}
        }
    },
}

_TIMESERIES_PAYLOAD: Dict[str, Any] = {
    "tenant_id": "tenant-1",
    "range": "7d",
    "days": [
        {"date": "2026-05-25", "kind": "revenue_usd", "count": 2, "sum": 2400.0}
    ],
}


def _make_transport(
    status: int, body: Any, method: Optional[str] = None, path: Optional[str] = None
) -> httpx.MockTransport:
    """Return a MockTransport that serves *body* as JSON with *status*."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            status_code=status,
            headers={"content-type": "application/json"},
            content=json.dumps(body).encode(),
        )

    return httpx.MockTransport(handler)


def client_with(status: int, body: Any) -> XiaoguaiClient:
    return XiaoguaiClient(
        "http://test",
        token="test-token",
        transport=_make_transport(status, body),
    )


# ---------------------------------------------------------------------------
# HotL — list_hotl_policies
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_list_hotl_policies_happy_path() -> None:
    c = client_with(200, [_POLICY_PAYLOAD])
    policies = c.list_hotl_policies(tenant_id="tenant-1")
    assert len(policies) == 1
    p = policies[0]
    assert isinstance(p, HotlPolicy)
    assert p.id == "aaaa-bbbb-cccc-dddd"
    assert p.scope == "llm_call"
    assert p.max_count == 100
    assert p.max_usd is None
    assert p.escalate_to == "ops@example.com"


@pytest.mark.unit
def test_list_hotl_policies_empty() -> None:
    c = client_with(200, [])
    assert c.list_hotl_policies(tenant_id="tenant-x") == []


@pytest.mark.unit
def test_list_hotl_policies_503_unavailable() -> None:
    c = client_with(503, {"error": "HOTL policy store not wired"})
    with pytest.raises(XiaoguaiUnavailableError):
        c.list_hotl_policies(tenant_id="tenant-1")


# ---------------------------------------------------------------------------
# HotL — create_hotl_policy
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_create_hotl_policy_happy_path() -> None:
    c = client_with(201, _POLICY_PAYLOAD)
    p = c.create_hotl_policy(
        tenant_id="tenant-1",
        scope="llm_call",
        window_seconds=3600,
        max_count=100,
        escalate_to="ops@example.com",
    )
    assert isinstance(p, HotlPolicy)
    assert p.window_seconds == 3600


@pytest.mark.unit
def test_create_hotl_policy_400_invalid() -> None:
    c = client_with(400, {"error": "at least one of max_count or max_usd must be set"})
    with pytest.raises(XiaoguaiValidationError):
        c.create_hotl_policy(
            tenant_id="tenant-1",
            scope="llm_call",
            window_seconds=3600,
            # neither max_count nor max_usd
        )


# ---------------------------------------------------------------------------
# HotL — delete_hotl_policy
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_delete_hotl_policy_happy_path() -> None:
    c = client_with(204, {})
    # Should not raise.
    c.delete_hotl_policy("aaaa-bbbb-cccc-dddd")


@pytest.mark.unit
def test_delete_hotl_policy_404_not_found() -> None:
    c = client_with(404, {"error": "not found"})
    with pytest.raises(XiaoguaiNotFoundError):
        c.delete_hotl_policy("does-not-exist")


# ---------------------------------------------------------------------------
# HotL — get/update/check — NotImplemented stubs
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_get_hotl_policy_raises_not_implemented() -> None:
    c = client_with(200, {})
    with pytest.raises(NotImplementedError):
        c.get_hotl_policy("any-id")


@pytest.mark.unit
def test_update_hotl_policy_raises_not_implemented() -> None:
    c = client_with(200, {})
    with pytest.raises(NotImplementedError):
        c.update_hotl_policy("any-id")


@pytest.mark.unit
def test_check_hotl_raises_not_implemented() -> None:
    c = client_with(200, {})
    with pytest.raises(NotImplementedError):
        c.check_hotl(scope="llm_call", amount=1.0)


# ---------------------------------------------------------------------------
# Outcomes — record_outcome
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_record_outcome_happy_path() -> None:
    c = client_with(201, {"ok": True})
    result = c.record_outcome(
        tenant_id="tenant-1",
        agent_name="sales-bot",
        kind="revenue_usd",
        value=1200.0,
        description="Enterprise deal closed",
    )
    assert result is True


@pytest.mark.unit
def test_record_outcome_400_negative_value() -> None:
    c = client_with(400, {"error": "value must be non-negative"})
    with pytest.raises(XiaoguaiValidationError):
        c.record_outcome(
            tenant_id="tenant-1",
            agent_name="sales-bot",
            kind="revenue_usd",
            value=-1.0,
        )


@pytest.mark.unit
def test_record_outcome_400_empty_kind() -> None:
    c = client_with(400, {"error": "kind must not be empty"})
    with pytest.raises(XiaoguaiValidationError):
        c.record_outcome(
            tenant_id="tenant-1",
            agent_name="sales-bot",
            kind="",
            value=10.0,
        )


# ---------------------------------------------------------------------------
# Outcomes — list_outcomes NotImplemented stub
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_list_outcomes_raises_not_implemented() -> None:
    c = client_with(200, {})
    with pytest.raises(NotImplementedError):
        c.list_outcomes()


# ---------------------------------------------------------------------------
# Outcomes — outcomes_summary
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_outcomes_summary_happy_path() -> None:
    c = client_with(200, _SUMMARY_PAYLOAD)
    summary = c.outcomes_summary(tenant_id="tenant-1", range="7d")
    assert isinstance(summary, OutcomeSummary)
    assert summary.tenant_id == "tenant-1"
    assert summary.range == "7d"
    assert "revenue_usd" in summary.by_kind
    bucket = summary.by_kind["revenue_usd"]
    assert bucket.count == 3
    assert abs(bucket.sum - 3600.0) < 0.01
    assert abs(bucket.avg - 1200.0) < 0.01


@pytest.mark.unit
def test_outcomes_summary_400_missing_tenant() -> None:
    c = client_with(400, {"error": "tenant_id is required"})
    with pytest.raises(XiaoguaiValidationError):
        c.outcomes_summary(tenant_id="")


# ---------------------------------------------------------------------------
# Outcomes — outcomes_timeseries
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_outcomes_timeseries_happy_path() -> None:
    c = client_with(200, _TIMESERIES_PAYLOAD)
    ts = c.outcomes_timeseries(tenant_id="tenant-1", range="7d", kind="revenue_usd")
    assert isinstance(ts, OutcomeTimeseries)
    assert ts.range == "7d"
    assert len(ts.days) == 1
    day = ts.days[0]
    assert day.date == "2026-05-25"
    assert day.count == 2
    assert abs(day.sum - 2400.0) < 0.01


@pytest.mark.unit
def test_outcomes_timeseries_empty_days() -> None:
    payload = {**_TIMESERIES_PAYLOAD, "days": []}
    c = client_with(200, payload)
    ts = c.outcomes_timeseries(tenant_id="tenant-1")
    assert ts.days == []


# ---------------------------------------------------------------------------
# Skills — list_skill_catalog
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_list_skill_catalog_happy_path() -> None:
    c = client_with(200, _CATALOG_PAYLOAD)
    packs = c.list_skill_catalog()
    assert len(packs) == 1
    p = packs[0]
    assert isinstance(p, SkillPackEntry)
    assert p.slug == "rag-legal"
    assert p.category == "rag"


# ---------------------------------------------------------------------------
# Skills — list_installed_skills
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_list_installed_skills_happy_path() -> None:
    c = client_with(200, [_INSTALLED_PACK_PAYLOAD])
    packs = c.list_installed_skills(tenant_id="tenant-1")
    assert len(packs) == 1
    p = packs[0]
    assert isinstance(p, InstalledSkillPack)
    assert p.pack_slug == "rag-legal"
    assert p.config == {"top_k": 5}


@pytest.mark.unit
def test_list_installed_skills_empty() -> None:
    c = client_with(200, [])
    assert c.list_installed_skills(tenant_id="tenant-x") == []


# ---------------------------------------------------------------------------
# Skills — install_skill
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_install_skill_happy_path() -> None:
    c = client_with(200, _INSTALLED_PACK_PAYLOAD)
    pack = c.install_skill(tenant_id="tenant-1", pack_slug="rag-legal")
    assert isinstance(pack, InstalledSkillPack)
    assert pack.pack_slug == "rag-legal"


@pytest.mark.unit
def test_install_skill_404_unknown_slug() -> None:
    c = client_with(404, {"error": "not found"})
    with pytest.raises(XiaoguaiNotFoundError):
        c.install_skill(tenant_id="tenant-1", pack_slug="unknown-slug")


@pytest.mark.unit
def test_install_skill_409_already_installed() -> None:
    c = client_with(409, {"error": "pack already installed"})
    with pytest.raises(XiaoguaiConflictError):
        c.install_skill(tenant_id="tenant-1", pack_slug="rag-legal")


# ---------------------------------------------------------------------------
# Skills — uninstall_skill
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_uninstall_skill_happy_path() -> None:
    c = client_with(200, {"deleted": "inst-1"})
    deleted_id = c.uninstall_skill("inst-1")
    assert deleted_id == "inst-1"


@pytest.mark.unit
def test_uninstall_skill_404() -> None:
    c = client_with(404, {"error": "not found"})
    with pytest.raises(XiaoguaiNotFoundError):
        c.uninstall_skill("does-not-exist")


# ---------------------------------------------------------------------------
# Error class hierarchy
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_specific_errors_are_subclasses_of_http_error() -> None:
    for cls in (
        XiaoguaiNotFoundError,
        XiaoguaiValidationError,
        XiaoguaiConflictError,
        XiaoguaiUnavailableError,
    ):
        assert issubclass(cls, XiaoguaiHTTPError)


@pytest.mark.unit
def test_generic_5xx_raises_base_http_error() -> None:
    c = client_with(500, {"error": "internal server error"})
    with pytest.raises(XiaoguaiHTTPError) as exc_info:
        c.list_hotl_policies(tenant_id="tenant-1")
    assert exc_info.value.status_code == 500


# ---------------------------------------------------------------------------
# Model: HotlVerdict convenience properties
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_hotl_verdict_allowed_property() -> None:
    v = HotlVerdict(verdict="allow", reason=None)
    assert v.allowed is True
    assert v.denied is False


@pytest.mark.unit
def test_hotl_verdict_denied_property() -> None:
    v = HotlVerdict(verdict="deny", reason="budget exceeded")
    assert v.denied is True
    assert v.allowed is False


# ---------------------------------------------------------------------------
# Context manager
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_client_context_manager_closes_cleanly() -> None:
    with client_with(200, []) as c:
        result = c.list_hotl_policies(tenant_id="tenant-1")
    assert result == []
