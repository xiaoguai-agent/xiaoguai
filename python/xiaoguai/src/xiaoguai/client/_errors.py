"""HTTP client error types.

``XiaoguaiHTTPError`` wraps a non-2xx response.  Sub-classes exist for
the most common HTTP status ranges so callers can match on the specific
error class without inspecting the status code manually.
"""

from __future__ import annotations

from typing import Any, Optional


class XiaoguaiHTTPError(Exception):
    """Raised when the API returns a non-2xx status code."""

    def __init__(self, status_code: int, body: Any, message: Optional[str] = None) -> None:
        self.status_code = status_code
        self.body = body
        detail = message or (body.get("error") if isinstance(body, dict) else str(body))
        super().__init__(f"HTTP {status_code}: {detail}")


class XiaoguaiNotFoundError(XiaoguaiHTTPError):
    """Raised for 404 responses."""


class XiaoguaiValidationError(XiaoguaiHTTPError):
    """Raised for 400/422 responses (invalid request body or parameters)."""


class XiaoguaiConflictError(XiaoguaiHTTPError):
    """Raised for 409 responses (e.g. pack already installed)."""


class XiaoguaiUnavailableError(XiaoguaiHTTPError):
    """Raised for 503 responses (e.g. store not wired)."""


def _raise_for_status(status_code: int, body: Any) -> None:
    """Raise the appropriate error sub-class for a non-2xx *status_code*."""
    if 200 <= status_code < 300:
        return
    cls: type
    if status_code == 404:
        cls = XiaoguaiNotFoundError
    elif status_code in (400, 422):
        cls = XiaoguaiValidationError
    elif status_code == 409:
        cls = XiaoguaiConflictError
    elif status_code == 503:
        cls = XiaoguaiUnavailableError
    else:
        cls = XiaoguaiHTTPError
    raise cls(status_code, body)
