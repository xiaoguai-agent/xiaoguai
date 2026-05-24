"""Python wrapper around the Rust ``xiaoguai`` CLI.

Installing this package places an ``xiaoguai`` console script on PATH
that forwards to a bundled, platform-specific native binary. The
Python layer carries no agent logic — it exists so ``pip install
xiaoguai`` is a viable installation route alongside the Cargo /
Homebrew / tarball channels.

For programmatic use of the underlying agent runtime, drive the
``xiaoguai`` binary as a subprocess or call the HTTP / SSE API
exposed by ``xiaoguai-api``. Native Python bindings via PyO3 are a
deferred v1.2+ item.
"""

from __future__ import annotations

from ._version import __version__

__all__ = ["__version__"]
