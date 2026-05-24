"""Smoke tests for the Python launcher.

These run in CI after ``cibuildwheel`` builds + installs the wheel,
and locally if the developer has staged a binary under
``src/xiaoguai/_binary/<triple>/xiaoguai``. When no binary is
present we skip rather than fail — running ``pytest`` from a fresh
checkout should not be noisy.
"""

from __future__ import annotations

import os
import platform
import subprocess
import sys

import pytest

from xiaoguai._binary import current_target_triple, find_binary


@pytest.fixture(scope="module")
def binary_present() -> bool:
    """True when a usable native binary is bundled in package data."""
    return find_binary() is not None


@pytest.mark.unit
def test_target_triple_resolves_on_supported_platform() -> None:
    """The current platform must map to a known target triple.

    If this test fails on a supported CI runner, the matrix in
    ``pip-wheel.yml`` and the dispatch in ``_binary.current_target_triple``
    have drifted.
    """
    system = platform.system()
    if system not in ("Darwin", "Linux"):
        pytest.skip(f"target triple resolution is platform-specific (got {system})")

    triple = current_target_triple()
    assert triple in {
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    }


@pytest.mark.unit
def test_missing_binary_prints_helpful_error(monkeypatch: pytest.MonkeyPatch) -> None:
    """When ``find_binary`` returns ``None``, the launcher must exit
    non-zero with a guidance message — never crash on the missing file."""
    from xiaoguai import __main__ as launcher

    monkeypatch.setattr(launcher, "find_binary", lambda: None)

    with pytest.raises(SystemExit) as excinfo:
        launcher.main([])
    assert excinfo.value.code == 2


@pytest.mark.integration
def test_module_invocation_prints_help(binary_present: bool) -> None:
    """``python -m xiaoguai --help`` must succeed and mention a known
    subcommand. Skipped when no binary is bundled (dev installs)."""
    if not binary_present:
        pytest.skip("no native binary bundled — run CI build first")

    completed = subprocess.run(
        [sys.executable, "-m", "xiaoguai", "--help"],
        capture_output=True,
        text=True,
        check=False,
        env={**os.environ, "XIAOGUAI_PY_DEBUG": ""},
    )
    assert completed.returncode == 0, completed.stderr
    combined = completed.stdout + completed.stderr
    # The actual `xiaoguai --help` text lists chat / provider / mcp /
    # remote / eval as the top-level subcommands. We assert on `chat`
    # because it's the most stable and user-facing.
    assert "chat" in combined.lower(), combined


@pytest.mark.integration
def test_console_script_help(binary_present: bool) -> None:
    """The ``xiaoguai`` console script installed by setuptools must work."""
    if not binary_present:
        pytest.skip("no native binary bundled — run CI build first")

    completed = subprocess.run(
        ["xiaoguai", "--help"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert "chat" in (completed.stdout + completed.stderr).lower()
