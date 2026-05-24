"""Pytest configuration.

Registers the ``unit`` and ``integration`` markers used by
``test_invokes_binary.py`` so ``pytest --strict-markers`` does not
warn about unknown marks.
"""

from __future__ import annotations

import pytest


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "unit: fast, in-process tests")
    config.addinivalue_line(
        "markers",
        "integration: spawns the installed xiaoguai binary as a subprocess",
    )
