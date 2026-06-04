"""
Pact consumer contract tests — Python SDK vs. xiaoguai wave-3 API.

Consumer: python-sdk
Provider: xiaoguai
Pact spec: v3

Covers 12 interactions — identical surface to the TypeScript SDK consumer so
that both SDKs pin the same provider behaviour:
  - HotL CRUD (list, create, get, update, delete) + check
  - Outcomes (record, summary, timeseries)
  - Skills (list installed, install, uninstall)
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import requests
from pact import Consumer, Provider  # type: ignore[import-untyped]
from pact.matchers import EachLike, Like, Term  # type: ignore[import-untyped]

PACT_DIR = str(Path(__file__).parent.parent.parent / "pacts")

TENANT_UUID = "11111111-1111-1111-1111-111111111111"
POLICY_UUID = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
INSTALL_UUID = "cccccccc-cccc-cccc-cccc-cccccccccccc"
BEARER = "Bearer test-token"

UUID_PATTERN = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"

POLICY_BODY = {
    "id": Term(UUID_PATTERN, POLICY_UUID),
    "tenant_id": Term(UUID_PATTERN, TENANT_UUID),
    "scope": Like("llm_call"),
    "window_seconds": Like(3600),
    "max_count": Like(100),
    "max_usd": Like(5.0),
    "escalate_to": Like("ops@example.com"),
}


@pytest.fixture(scope="module")
def pact():
    """Configure the Pact mock provider for the module."""
    _pact = Consumer("python-sdk").has_pact_with(
        Provider("xiaoguai"),
        pact_dir=PACT_DIR,
        specification_version="3.0.0",
        log_dir="/tmp/pact-python-sdk",
    )
    _pact.start_service()
    yield _pact
    _pact.stop_service()


# ─────────────────────────────────────────────────────────────────────────────
# HotL policies
# ─────────────────────────────────────────────────────────────────────────────


def test_list_hotl_policies(pact: Consumer) -> None:
    """Interaction 1: GET /v1/hotl/policies → 200 array."""
    (
        pact.given("tenant has one HotL policy")
        .upon_receiving("a GET /v1/hotl/policies request for tenant 11111111")
        .with_request(
            method="GET",
            path="/v1/hotl/policies",
            query={"tenant_id": TENANT_UUID},
            headers={"Authorization": BEARER},
        )
        .will_respond_with(
            status=200,
            headers={"Content-Type": "application/json"},
            body=EachLike(POLICY_BODY),
        )
    )

    with pact:
        resp = requests.get(
            f"{pact.uri}/v1/hotl/policies",
            params={"tenant_id": TENANT_UUID},
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 200
    body = resp.json()
    assert isinstance(body, list)
    assert len(body) > 0


def test_create_hotl_policy(pact: Consumer) -> None:
    """Interaction 2: POST /v1/hotl/policies → 201."""
    (
        pact.given("HotL policy store is available")
        .upon_receiving("a POST /v1/hotl/policies request")
        .with_request(
            method="POST",
            path="/v1/hotl/policies",
            headers={
                "Authorization": BEARER,
                "Content-Type": "application/json",
            },
            body={
                "tenant_id": TENANT_UUID,
                "scope": "llm_call",
                "window_seconds": 3600,
                "max_count": 100,
                "max_usd": 5.0,
                "escalate_to": "ops@example.com",
            },
        )
        .will_respond_with(
            status=201,
            headers={"Content-Type": "application/json"},
            body=POLICY_BODY,
        )
    )

    with pact:
        resp = requests.post(
            f"{pact.uri}/v1/hotl/policies",
            json={
                "tenant_id": TENANT_UUID,
                "scope": "llm_call",
                "window_seconds": 3600,
                "max_count": 100,
                "max_usd": 5.0,
                "escalate_to": "ops@example.com",
            },
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 201
    body = resp.json()
    assert "id" in body
    assert body["scope"] == "llm_call"


def test_get_hotl_policy(pact: Consumer) -> None:
    """Interaction 3: GET /v1/hotl/policies/:id → 200."""
    (
        pact.given(f"HotL policy {POLICY_UUID} exists")
        .upon_receiving(f"a GET /v1/hotl/policies/{POLICY_UUID} request")
        .with_request(
            method="GET",
            path=f"/v1/hotl/policies/{POLICY_UUID}",
            headers={"Authorization": BEARER},
        )
        .will_respond_with(
            status=200,
            headers={"Content-Type": "application/json"},
            body=POLICY_BODY,
        )
    )

    with pact:
        resp = requests.get(
            f"{pact.uri}/v1/hotl/policies/{POLICY_UUID}",
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 200
    assert "id" in resp.json()


def test_update_hotl_policy(pact: Consumer) -> None:
    """Interaction 4: PUT /v1/hotl/policies/:id → 200."""
    (
        pact.given(f"HotL policy {POLICY_UUID} exists")
        .upon_receiving(f"a PUT /v1/hotl/policies/{POLICY_UUID} request")
        .with_request(
            method="PUT",
            path=f"/v1/hotl/policies/{POLICY_UUID}",
            headers={
                "Authorization": BEARER,
                "Content-Type": "application/json",
            },
            body={
                "tenant_id": TENANT_UUID,
                "scope": "llm_call",
                "window_seconds": 7200,
                "max_count": 200,
                "max_usd": None,
                "escalate_to": None,
            },
        )
        .will_respond_with(
            status=200,
            headers={"Content-Type": "application/json"},
            body={**POLICY_BODY, "window_seconds": Like(7200), "max_count": Like(200)},
        )
    )

    with pact:
        resp = requests.put(
            f"{pact.uri}/v1/hotl/policies/{POLICY_UUID}",
            json={
                "tenant_id": TENANT_UUID,
                "scope": "llm_call",
                "window_seconds": 7200,
                "max_count": 200,
                "max_usd": None,
                "escalate_to": None,
            },
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 200


def test_delete_hotl_policy(pact: Consumer) -> None:
    """Interaction 5: DELETE /v1/hotl/policies/:id → 204."""
    (
        pact.given(f"HotL policy {POLICY_UUID} exists")
        .upon_receiving(f"a DELETE /v1/hotl/policies/{POLICY_UUID} request")
        .with_request(
            method="DELETE",
            path=f"/v1/hotl/policies/{POLICY_UUID}",
            headers={"Authorization": BEARER},
        )
        .will_respond_with(status=204)
    )

    with pact:
        resp = requests.delete(
            f"{pact.uri}/v1/hotl/policies/{POLICY_UUID}",
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 204


def test_hotl_check_allow(pact: Consumer) -> None:
    """Interaction 6: POST /v1/hotl/check → allow verdict."""
    (
        pact.given("tenant HotL policy exists and budget is within limits")
        .upon_receiving("a POST /v1/hotl/check request within budget")
        .with_request(
            method="POST",
            path="/v1/hotl/check",
            headers={
                "Authorization": BEARER,
                "Content-Type": "application/json",
            },
            body={
                "tenant_id": TENANT_UUID,
                "scope": "llm_call",
                "amount": 0.0025,
            },
        )
        .will_respond_with(
            status=200,
            headers={"Content-Type": "application/json"},
            body={"verdict": Like("allow"), "reason": None},
        )
    )

    with pact:
        resp = requests.post(
            f"{pact.uri}/v1/hotl/check",
            json={"tenant_id": TENANT_UUID, "scope": "llm_call", "amount": 0.0025},
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 200
    assert resp.json()["verdict"] == "allow"


# ─────────────────────────────────────────────────────────────────────────────
# Outcomes
# ─────────────────────────────────────────────────────────────────────────────


def test_record_outcome(pact: Consumer) -> None:
    """Interaction 7: POST /v1/outcomes → 201."""
    (
        pact.given("outcome writer is available")
        .upon_receiving("a POST /v1/outcomes request")
        .with_request(
            method="POST",
            path="/v1/outcomes",
            headers={
                "Authorization": BEARER,
                "Content-Type": "application/json",
            },
            body={
                "tenant_id": "tenant_acme",
                "session_id": "sess_abc123",
                "agent_name": "sales-bot",
                "kind": "revenue_usd",
                "value": 1250.0,
                "unit": "usd",
                "description": "Closed deal D-4471",
                "metadata": {"deal_id": "D-4471"},
            },
        )
        .will_respond_with(
            status=201,
            headers={"Content-Type": "application/json"},
            body={"ok": True},
        )
    )

    with pact:
        resp = requests.post(
            f"{pact.uri}/v1/outcomes",
            json={
                "tenant_id": "tenant_acme",
                "session_id": "sess_abc123",
                "agent_name": "sales-bot",
                "kind": "revenue_usd",
                "value": 1250.0,
                "unit": "usd",
                "description": "Closed deal D-4471",
                "metadata": {"deal_id": "D-4471"},
            },
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 201
    assert resp.json()["ok"] is True


def test_outcomes_summary(pact: Consumer) -> None:
    """Interaction 8: GET /v1/outcomes/summary → 200."""
    (
        pact.given("tenant has recorded outcomes")
        .upon_receiving("a GET /v1/outcomes/summary request for 7d")
        .with_request(
            method="GET",
            path="/v1/outcomes/summary",
            query={"tenant_id": "tenant_acme", "range": "7d"},
            headers={"Authorization": BEARER},
        )
        .will_respond_with(
            status=200,
            headers={"Content-Type": "application/json"},
            body={
                "tenant_id": Like("tenant_acme"),
                "range": Like("7d"),
                "summary": Like(
                    {
                        "by_kind": Like(
                            {
                                "revenue_usd": {
                                    "sum": Like(42000.0),
                                    "count": Like(18),
                                    "avg": Like(2333.33),
                                }
                            }
                        )
                    }
                ),
            },
        )
    )

    with pact:
        resp = requests.get(
            f"{pact.uri}/v1/outcomes/summary",
            params={"tenant_id": "tenant_acme", "range": "7d"},
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 200
    body = resp.json()
    assert "summary" in body
    assert "by_kind" in body["summary"]


def test_outcomes_timeseries(pact: Consumer) -> None:
    """Interaction 9: GET /v1/outcomes/timeseries → 200."""
    (
        pact.given("tenant has recorded outcomes")
        .upon_receiving("a GET /v1/outcomes/timeseries request for 7d")
        .with_request(
            method="GET",
            path="/v1/outcomes/timeseries",
            query={"tenant_id": "tenant_acme", "range": "7d"},
            headers={"Authorization": BEARER},
        )
        .will_respond_with(
            status=200,
            headers={"Content-Type": "application/json"},
            body={
                "tenant_id": Like("tenant_acme"),
                "range": Like("7d"),
                "days": EachLike(
                    {
                        "date": Like("2026-05-20"),
                        "kind": Like("revenue_usd"),
                        "sum": Like(5000.0),
                        "count": Like(2),
                    }
                ),
            },
        )
    )

    with pact:
        resp = requests.get(
            f"{pact.uri}/v1/outcomes/timeseries",
            params={"tenant_id": "tenant_acme", "range": "7d"},
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 200
    body = resp.json()
    assert isinstance(body["days"], list)


# ─────────────────────────────────────────────────────────────────────────────
# Skills
# ─────────────────────────────────────────────────────────────────────────────

INSTALLED_PACK_BODY = {
    "id": Term(UUID_PATTERN, INSTALL_UUID),
    "tenant_id": Like("tenant_acme"),
    "pack_slug": Like("pr-review"),
    "version": Like("1.0.0"),
    "config": Like({}),
    "installed_at": Like("2026-05-25T12:34:56Z"),
}


def test_list_installed_skills(pact: Consumer) -> None:
    """Interaction 10: GET /v1/skills/installed → 200."""
    (
        pact.given("tenant has installed skill packs")
        .upon_receiving("a GET /v1/skills/installed request")
        .with_request(
            method="GET",
            path="/v1/skills/installed",
            query={"tenant_id": "tenant_acme"},
            headers={"Authorization": BEARER},
        )
        .will_respond_with(
            status=200,
            headers={"Content-Type": "application/json"},
            body=EachLike(INSTALLED_PACK_BODY),
        )
    )

    with pact:
        resp = requests.get(
            f"{pact.uri}/v1/skills/installed",
            params={"tenant_id": "tenant_acme"},
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 200
    assert isinstance(resp.json(), list)


def test_install_skill_pack(pact: Consumer) -> None:
    """Interaction 11: POST /v1/skills/install → 201."""
    (
        pact.given("skill pack pr-review exists in catalog")
        .upon_receiving("a POST /v1/skills/install request")
        .with_request(
            method="POST",
            path="/v1/skills/install",
            headers={
                "Authorization": BEARER,
                "Content-Type": "application/json",
            },
            body={"tenant_id": "tenant_acme", "pack_slug": "pr-review", "config": {}},
        )
        .will_respond_with(
            status=201,
            headers={"Content-Type": "application/json"},
            body=INSTALLED_PACK_BODY,
        )
    )

    with pact:
        resp = requests.post(
            f"{pact.uri}/v1/skills/install",
            json={"tenant_id": "tenant_acme", "pack_slug": "pr-review", "config": {}},
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 201
    body = resp.json()
    assert "id" in body
    assert body["pack_slug"] == "pr-review"


def test_uninstall_skill_pack(pact: Consumer) -> None:
    """Interaction 12: DELETE /v1/skills/install/:id → 204."""
    (
        pact.given(f"skill pack installation {INSTALL_UUID} exists")
        .upon_receiving(f"a DELETE /v1/skills/install/{INSTALL_UUID} request")
        .with_request(
            method="DELETE",
            path=f"/v1/skills/install/{INSTALL_UUID}",
            headers={"Authorization": BEARER},
        )
        .will_respond_with(status=204)
    )

    with pact:
        resp = requests.delete(
            f"{pact.uri}/v1/skills/install/{INSTALL_UUID}",
            headers={"Authorization": BEARER},
            timeout=5,
        )
    assert resp.status_code == 204
