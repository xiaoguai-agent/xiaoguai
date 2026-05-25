"""``xiaoguai.client`` — synchronous HTTP client for the xiaoguai REST API.

Wave-3 endpoint coverage (v1.2.x):

HotL (boundary policy CRUD)::

    client.list_hotl_policies(tenant_id="...")
    client.create_hotl_policy(tenant_id="...", scope="llm_call", window_seconds=3600, max_count=100)
    client.delete_hotl_policy(policy_id="<uuid>")

Outcomes (ROI telemetry)::

    client.record_outcome(tenant_id="...", agent_name="sales-bot", kind="revenue_usd", value=1200.0)
    client.outcomes_summary(tenant_id="...", range="7d")
    client.outcomes_timeseries(tenant_id="...", range="30d", kind="hours_saved")

Skills (pack marketplace)::

    client.list_skill_catalog()
    client.list_installed_skills(tenant_id="...")
    client.install_skill(tenant_id="...", pack_slug="rag-legal")
    client.uninstall_skill(install_id="<uuid>")

Quickstart::

    from xiaoguai.client import XiaoguaiClient

    with XiaoguaiClient("http://localhost:8080", token="my-bearer-token") as c:
        policies = c.list_hotl_policies(tenant_id="my-tenant-uuid")
        ok = c.record_outcome(
            tenant_id="my-tenant",
            agent_name="sales-bot",
            kind="revenue_usd",
            value=1500.0,
            description="Closed enterprise deal",
        )
        summary = c.outcomes_summary(tenant_id="my-tenant", range="7d")
        packs = c.list_installed_skills(tenant_id="my-tenant")

Requires ``httpx``::

    pip install httpx
"""

from __future__ import annotations

from ._client import XiaoguaiClient
from ._errors import (
    XiaoguaiConflictError,
    XiaoguaiHTTPError,
    XiaoguaiNotFoundError,
    XiaoguaiUnavailableError,
    XiaoguaiValidationError,
)
from ._models import (
    HotlPolicy,
    HotlVerdict,
    InstalledSkillPack,
    OutcomeDay,
    OutcomeRecord,
    OutcomeSummary,
    OutcomeSummaryBucket,
    OutcomeTimeseries,
    SkillPackEntry,
)

__all__ = [
    # Client
    "XiaoguaiClient",
    # Errors
    "XiaoguaiHTTPError",
    "XiaoguaiNotFoundError",
    "XiaoguaiValidationError",
    "XiaoguaiConflictError",
    "XiaoguaiUnavailableError",
    # Models
    "HotlPolicy",
    "HotlVerdict",
    "OutcomeRecord",
    "OutcomeSummaryBucket",
    "OutcomeSummary",
    "OutcomeDay",
    "OutcomeTimeseries",
    "InstalledSkillPack",
    "SkillPackEntry",
]
