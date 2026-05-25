"""Synchronous HTTP client for the xiaoguai REST API — wave-3 endpoints.

Requires the optional ``httpx`` dependency::

    pip install 'xiaoguai[client]'   # once pyproject.toml exposes the extra
    # — or —
    pip install httpx

Usage::

    from xiaoguai.client import XiaoguaiClient

    with XiaoguaiClient("http://localhost:8080", token="bearer-token") as c:
        policies = c.list_hotl_policies(tenant_id="<uuid>")
        c.record_outcome(
            tenant_id="my-tenant",
            agent_name="sales-bot",
            kind="revenue_usd",
            value=1200.0,
        )
        packs = c.list_installed_skills(tenant_id="my-tenant")

API surface matches the wave-3 REST handlers documented in
``crates/xiaoguai-api/src/routes/{hotl,outcomes}.rs`` and
``crates/xiaoguai-api/src/skills.rs``.

Design notes
------------
* Synchronous (blocking) — mirrors the existing SDK style (no asyncio usage
  in the launcher layer).
* All methods raise sub-classes of :class:`XiaoguaiHTTPError` on non-2xx
  responses; callers should not inspect raw status codes.
* ``httpx.Client`` is used as the underlying transport so tests can inject an
  ``httpx.MockTransport`` without any monkey-patching.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional

try:
    import httpx
except ImportError as exc:  # pragma: no cover
    raise ImportError(
        "The xiaoguai HTTP client requires 'httpx'. "
        "Install it with: pip install 'xiaoguai[client]' or pip install httpx"
    ) from exc

from ._errors import _raise_for_status
from ._models import (
    HotlPolicy,
    HotlVerdict,
    InstalledSkillPack,
    OutcomeSummary,
    OutcomeTimeseries,
    SkillPackEntry,
)

__all__ = ["XiaoguaiClient"]


class XiaoguaiClient:
    """Synchronous REST client for the xiaoguai API.

    Parameters
    ----------
    base_url:
        Root URL of the running ``xiaoguai-api`` server, e.g.
        ``"http://localhost:8080"`` or ``"https://api.example.com"``.
        Must not include a trailing ``/v1`` path segment.
    token:
        Bearer token for the ``Authorization: Bearer <token>`` header.
        Pass ``None`` when the server has auth disabled (dev / tests).
    timeout:
        Per-request timeout in seconds.  Defaults to 30.
    transport:
        Optional ``httpx.BaseTransport`` override — used by tests to inject
        a ``MockTransport`` without starting a real server.
    """

    def __init__(
        self,
        base_url: str,
        token: Optional[str] = None,
        timeout: float = 30.0,
        transport: Optional[httpx.BaseTransport] = None,
    ) -> None:
        headers: Dict[str, str] = {"Content-Type": "application/json", "Accept": "application/json"}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        self._http = httpx.Client(
            base_url=base_url,
            headers=headers,
            timeout=timeout,
            transport=transport,
        )

    # ------------------------------------------------------------------
    # Context-manager protocol
    # ------------------------------------------------------------------

    def __enter__(self) -> "XiaoguaiClient":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    def close(self) -> None:
        """Close the underlying HTTP connection pool."""
        self._http.close()

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _get(self, path: str, params: Optional[Dict[str, Any]] = None) -> Any:
        resp = self._http.get(path, params=params)
        body = _try_json(resp)
        _raise_for_status(resp.status_code, body)
        return body

    def _post(self, path: str, json: Optional[Dict[str, Any]] = None) -> Any:
        resp = self._http.post(path, json=json or {})
        body = _try_json(resp)
        _raise_for_status(resp.status_code, body)
        return body

    def _delete(self, path: str) -> Any:
        resp = self._http.delete(path)
        body = _try_json(resp)
        _raise_for_status(resp.status_code, body)
        return body

    # ------------------------------------------------------------------
    # HotL — boundary policy CRUD  (v1.2.3)
    # ------------------------------------------------------------------

    def list_hotl_policies(
        self,
        tenant_id: str,
        scope: Optional[str] = None,
    ) -> List[HotlPolicy]:
        """Return HOTL policies for *tenant_id*, optionally filtered by *scope*.

        Wraps ``GET /v1/hotl/policies?tenant_id=<uuid>[&scope=<str>]``.
        """
        params: Dict[str, Any] = {"tenant_id": tenant_id}
        if scope is not None:
            params["scope"] = scope
        rows = self._get("/v1/hotl/policies", params=params)
        return [HotlPolicy.from_dict(r) for r in rows]

    def create_hotl_policy(
        self,
        tenant_id: str,
        scope: str,
        window_seconds: int,
        max_count: Optional[int] = None,
        max_usd: Optional[float] = None,
        escalate_to: Optional[str] = None,
    ) -> HotlPolicy:
        """Create a new HOTL policy.

        At least one of *max_count* or *max_usd* must be provided (validated
        server-side; raises :class:`XiaoguaiValidationError` otherwise).

        Wraps ``POST /v1/hotl/policies``.
        """
        payload: Dict[str, Any] = {
            "tenant_id": tenant_id,
            "scope": scope,
            "window_seconds": window_seconds,
        }
        if max_count is not None:
            payload["max_count"] = max_count
        if max_usd is not None:
            payload["max_usd"] = max_usd
        if escalate_to is not None:
            payload["escalate_to"] = escalate_to
        data = self._post("/v1/hotl/policies", json=payload)
        return HotlPolicy.from_dict(data)

    def get_hotl_policy(self, policy_id: str) -> HotlPolicy:
        """Fetch a single HOTL policy by *policy_id* (UUID string).

        Note: the server currently exposes list+create+delete but not a
        dedicated GET-by-id endpoint. This method lists all policies for the
        tenant embedded in the id lookup (client-side filter) and raises
        :class:`XiaoguaiNotFoundError` when no match is found.

        To avoid N+1 round-trips, prefer :meth:`list_hotl_policies` when you
        already know the tenant_id.
        """
        raise NotImplementedError(
            "GET /v1/hotl/policies/:id is not yet exposed by the server. "
            "Use list_hotl_policies(tenant_id=...) and filter client-side."
        )

    def update_hotl_policy(self, policy_id: str, **_kwargs: Any) -> HotlPolicy:
        """Update an existing HOTL policy.

        Note: the server does not yet expose a PATCH/PUT endpoint for policies.
        Delete the existing policy and create a new one as a workaround.
        """
        raise NotImplementedError(
            "PATCH /v1/hotl/policies/:id is not yet exposed by the server. "
            "Delete and re-create the policy instead."
        )

    def delete_hotl_policy(self, policy_id: str) -> None:
        """Delete a HOTL policy by *policy_id*.

        Wraps ``DELETE /v1/hotl/policies/:id``.
        Raises :class:`XiaoguaiNotFoundError` when the id is unknown.
        """
        self._delete(f"/v1/hotl/policies/{policy_id}")

    def check_hotl(self, scope: str, amount: float, tenant_id: Optional[str] = None) -> HotlVerdict:
        """Check whether *amount* in *scope* is within budget.

        The server's HOTL enforcer is invoked internally when agents submit
        messages (``POST /v1/sessions/:id/messages``).  This method provides
        a direct check without triggering an actual LLM call — useful for
        pre-flight budget validation from orchestration code.

        Note: a dedicated ``POST /v1/hotl/check`` endpoint is not yet wired
        into the router (the enforcer runs in-process on the message path).
        This method is provided as a placeholder and raises
        ``NotImplementedError`` until the server exposes the endpoint.
        """
        raise NotImplementedError(
            "POST /v1/hotl/check is not yet exposed by the server. "
            "Budget checks run in-process when sending messages to a session."
        )

    # ------------------------------------------------------------------
    # Outcomes — ROI telemetry  (v1.2.4)
    # ------------------------------------------------------------------

    def record_outcome(
        self,
        tenant_id: str,
        agent_name: str,
        kind: str,
        value: float,
        session_id: Optional[str] = None,
        unit: Optional[str] = None,
        description: Optional[str] = None,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> bool:
        """Record a business outcome attribution.

        Wraps ``POST /v1/outcomes``. Returns ``True`` on success.
        Raises :class:`XiaoguaiValidationError` for invalid payloads (e.g.
        negative *value*, empty *kind* or *agent_name*).
        """
        payload: Dict[str, Any] = {
            "tenant_id": tenant_id,
            "agent_name": agent_name,
            "kind": kind,
            "value": value,
            "metadata": metadata or {},
        }
        if session_id is not None:
            payload["session_id"] = session_id
        if unit is not None:
            payload["unit"] = unit
        if description is not None:
            payload["description"] = description
        data = self._post("/v1/outcomes", json=payload)
        return bool(data.get("ok", False))

    def list_outcomes(self, filter: Optional[Dict[str, Any]] = None) -> List[Dict[str, Any]]:
        """List outcome records with optional *filter* params.

        Note: the server exposes ``/v1/outcomes/summary`` and
        ``/v1/outcomes/timeseries`` for aggregated reads; a raw list endpoint
        is not yet exposed. Use :meth:`outcomes_summary` or
        :meth:`outcomes_timeseries` instead.
        """
        raise NotImplementedError(
            "GET /v1/outcomes (raw list) is not yet exposed. "
            "Use outcomes_summary() or outcomes_timeseries() instead."
        )

    def outcomes_summary(
        self,
        tenant_id: str,
        range: Optional[str] = None,
    ) -> OutcomeSummary:
        """Aggregated ROI summary — one bucket per outcome kind.

        *range* accepts ``"24h"``, ``"7d"``, or ``"30d"`` (default ``"30d"``).
        Wraps ``GET /v1/outcomes/summary``.
        """
        params: Dict[str, Any] = {"tenant_id": tenant_id}
        if range is not None:
            params["range"] = range
        data = self._get("/v1/outcomes/summary", params=params)
        return OutcomeSummary.from_dict(data)

    def outcomes_timeseries(
        self,
        tenant_id: str,
        range: Optional[str] = None,
        kind: Optional[str] = None,
    ) -> OutcomeTimeseries:
        """Daily time-series breakdown.

        *range* accepts ``"24h"``, ``"7d"``, or ``"30d"`` (default ``"30d"``).
        *kind* optionally filters to a single outcome kind (e.g. ``"revenue_usd"``).
        Wraps ``GET /v1/outcomes/timeseries``.
        """
        params: Dict[str, Any] = {"tenant_id": tenant_id}
        if range is not None:
            params["range"] = range
        if kind is not None:
            params["kind"] = kind
        data = self._get("/v1/outcomes/timeseries", params=params)
        return OutcomeTimeseries.from_dict(data)

    # ------------------------------------------------------------------
    # Skills — pack marketplace  (v1.2.28)
    # ------------------------------------------------------------------

    def list_installed_skills(self, tenant_id: Optional[str] = None) -> List[InstalledSkillPack]:
        """List skill packs installed for *tenant_id*.

        Wraps ``GET /v1/skills/installed?tenant=<tenant_id>``.
        """
        params: Dict[str, Any] = {}
        if tenant_id is not None:
            params["tenant"] = tenant_id
        rows = self._get("/v1/skills/installed", params=params)
        return [InstalledSkillPack.from_dict(r) for r in rows]

    def list_skill_catalog(self) -> List[SkillPackEntry]:
        """List all available skill packs from the built-in catalog.

        Wraps ``GET /v1/skills/catalog`` (public, no auth required).
        """
        data = self._get("/v1/skills/catalog")
        packs = data.get("packs") or []
        return [SkillPackEntry.from_dict(p) for p in packs]

    def install_skill(
        self,
        tenant_id: str,
        pack_slug: str,
        config: Optional[Dict[str, Any]] = None,
    ) -> InstalledSkillPack:
        """Install a skill pack for *tenant_id*.

        *pack_slug* must be a slug that exists in the built-in catalog
        (e.g. ``"rag-legal"``); unknown slugs return 404.

        Raises :class:`XiaoguaiConflictError` when the pack is already
        installed for the tenant.
        Wraps ``POST /v1/skills/install``.
        """
        payload: Dict[str, Any] = {
            "tenant_id": tenant_id,
            "pack_slug": pack_slug,
            "config": config or {},
        }
        data = self._post("/v1/skills/install", json=payload)
        return InstalledSkillPack.from_dict(data)

    def uninstall_skill(self, install_id: str) -> str:
        """Uninstall a skill pack by its installation row *install_id*.

        Returns the deleted *install_id* on success.
        Raises :class:`XiaoguaiNotFoundError` when the row is absent.
        Wraps ``DELETE /v1/skills/install/:id``.
        """
        data = self._delete(f"/v1/skills/install/{install_id}")
        return str(data.get("deleted", install_id))


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _try_json(resp: httpx.Response) -> Any:
    """Return parsed JSON if the response body is JSON; otherwise raw text."""
    ct = resp.headers.get("content-type", "")
    if "application/json" in ct or "json" in ct:
        try:
            return resp.json()
        except Exception:
            pass
    text = resp.text
    return {"error": text} if text else {}
