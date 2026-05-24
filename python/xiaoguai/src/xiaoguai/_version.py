"""Version sourced from the ``XIAOGUAI_VERSION`` env var at build time.

The CI workflow exports ``XIAOGUAI_VERSION`` before invoking
``cibuildwheel``; in that environment setuptools imports this module
during the metadata-resolution phase and the value below is baked into
the wheel.

For local development builds (no env var) we fall back to a sentinel
version that is installable but obviously not a real release.
"""

from __future__ import annotations

import os

__all__ = ["__version__"]

__version__: str = os.environ.get("XIAOGUAI_VERSION", "0.0.0+local")
