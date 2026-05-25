"""Typed dataclasses for wave-3 API responses.

All classes are frozen dataclasses so callers get immutable value objects.
Shapes are derived from the Rust wire types in:
  - crates/xiaoguai-api/src/hotl/policy.rs      (HotlPolicy, CreateHotlPolicyRequest)
  - crates/xiaoguai-api/src/hotl/enforcer.rs    (HotlVerdict)
  - crates/xiaoguai-api/src/outcomes.rs         (RecordOutcomeRequest, summaries)
  - crates/xiaoguai-api/src/skills.rs           (SkillPackEntry, InstalledPackRow)
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


# ---------------------------------------------------------------------------
# HotL
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class HotlPolicy:
    """One row from ``GET /v1/hotl/policies``."""

    id: str
    tenant_id: str
    scope: str
    window_seconds: int
    max_count: Optional[int]
    max_usd: Optional[float]
    escalate_to: Optional[str]

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "HotlPolicy":
        return cls(
            id=data["id"],
            tenant_id=data["tenant_id"],
            scope=data["scope"],
            window_seconds=int(data["window_seconds"]),
            max_count=data.get("max_count"),
            max_usd=data.get("max_usd"),
            escalate_to=data.get("escalate_to"),
        )


@dataclass(frozen=True)
class HotlVerdict:
    """Decision returned by ``POST /v1/hotl/check``.

    ``verdict`` is one of ``"allow"``, ``"escalate"``, or ``"deny"``.
    ``reason`` is populated for ``escalate`` / ``deny`` outcomes.
    """

    verdict: str
    reason: Optional[str]

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "HotlVerdict":
        return cls(
            verdict=data["verdict"],
            reason=data.get("reason"),
        )

    @property
    def allowed(self) -> bool:
        return self.verdict == "allow"

    @property
    def denied(self) -> bool:
        return self.verdict == "deny"


# ---------------------------------------------------------------------------
# Outcomes
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class OutcomeRecord:
    """One outcome attribution record."""

    tenant_id: str
    session_id: Optional[str]
    agent_name: str
    kind: str
    value: float
    unit: Optional[str]
    description: Optional[str]
    metadata: Dict[str, Any]

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "OutcomeRecord":
        return cls(
            tenant_id=data["tenant_id"],
            session_id=data.get("session_id"),
            agent_name=data["agent_name"],
            kind=data["kind"],
            value=float(data["value"]),
            unit=data.get("unit"),
            description=data.get("description"),
            metadata=data.get("metadata") or {},
        )


@dataclass(frozen=True)
class OutcomeSummaryBucket:
    """Aggregated totals for one outcome kind."""

    kind: str
    count: int
    sum: float
    avg: float

    @classmethod
    def from_dict(cls, kind: str, data: Dict[str, Any]) -> "OutcomeSummaryBucket":
        return cls(
            kind=kind,
            count=int(data["count"]),
            sum=float(data["sum"]),
            avg=float(data["avg"]),
        )


@dataclass(frozen=True)
class OutcomeSummary:
    """Response from ``GET /v1/outcomes/summary``."""

    tenant_id: str
    range: str
    by_kind: Dict[str, OutcomeSummaryBucket]

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "OutcomeSummary":
        raw = data.get("summary", {}).get("by_kind") or {}
        by_kind = {k: OutcomeSummaryBucket.from_dict(k, v) for k, v in raw.items()}
        return cls(
            tenant_id=data["tenant_id"],
            range=data["range"],
            by_kind=by_kind,
        )


@dataclass(frozen=True)
class OutcomeDay:
    """One day bucket from ``GET /v1/outcomes/timeseries``."""

    date: str
    kind: str
    count: int
    sum: float

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "OutcomeDay":
        return cls(
            date=data["date"],
            kind=data.get("kind", ""),
            count=int(data["count"]),
            sum=float(data["sum"]),
        )


@dataclass(frozen=True)
class OutcomeTimeseries:
    """Response from ``GET /v1/outcomes/timeseries``."""

    tenant_id: str
    range: str
    days: List[OutcomeDay]

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "OutcomeTimeseries":
        days = [OutcomeDay.from_dict(d) for d in (data.get("days") or [])]
        return cls(
            tenant_id=data["tenant_id"],
            range=data["range"],
            days=days,
        )


# ---------------------------------------------------------------------------
# Skills
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class InstalledSkillPack:
    """One row from ``GET /v1/skills/installed``."""

    id: str
    tenant_id: str
    pack_slug: str
    version: str
    config: Dict[str, Any]
    installed_at: str

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "InstalledSkillPack":
        return cls(
            id=data["id"],
            tenant_id=data["tenant_id"],
            pack_slug=data["pack_slug"],
            version=data["version"],
            config=data.get("config") or {},
            installed_at=data["installed_at"],
        )


@dataclass(frozen=True)
class SkillPackEntry:
    """One entry from ``GET /v1/skills/catalog``."""

    slug: str
    name: str
    description: str
    version: str
    category: str
    requires: Dict[str, Any]
    knobs: Dict[str, Any]
    screenshot_url: Optional[str]

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "SkillPackEntry":
        return cls(
            slug=data["slug"],
            name=data["name"],
            description=data["description"],
            version=data["version"],
            category=data["category"],
            requires=data.get("requires") or {},
            knobs=data.get("knobs") or {},
            screenshot_url=data.get("screenshot_url"),
        )
