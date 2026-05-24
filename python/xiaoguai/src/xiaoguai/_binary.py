"""Locate the bundled ``xiaoguai`` native binary.

CI stages the Rust-built binary under
``<package>/_binary/<target-triple>/xiaoguai[.exe]`` before building
the wheel. At runtime we resolve the current platform's target triple
and return the absolute path.

When the binary is absent (developer ``pip install -e`` from a fresh
checkout) :func:`find_binary` returns ``None`` so callers can degrade
gracefully — :mod:`xiaoguai.__main__` prints a helpful error and exits
with status code 2.
"""

from __future__ import annotations

import os
import platform
import sys
from pathlib import Path
from typing import Optional

__all__ = ["find_binary", "current_target_triple"]


def _binary_root() -> Path:
    """Directory containing per-target subdirectories of binaries."""
    return Path(__file__).resolve().parent / "_binary"


def current_target_triple() -> str:
    """Return the Rust target triple matching the current interpreter.

    Mirrors the matrix in ``.github/workflows/pip-wheel.yml`` —
    ``aarch64-apple-darwin``, ``x86_64-apple-darwin``,
    ``x86_64-unknown-linux-gnu``, ``aarch64-unknown-linux-gnu``.

    Raises :class:`RuntimeError` on unsupported platforms so the
    failure mode is loud rather than picking a near-match binary.
    """
    system = platform.system()
    machine = platform.machine().lower()

    if system == "Darwin":
        if machine in ("arm64", "aarch64"):
            return "aarch64-apple-darwin"
        if machine in ("x86_64", "amd64"):
            return "x86_64-apple-darwin"
    elif system == "Linux":
        if machine in ("x86_64", "amd64"):
            return "x86_64-unknown-linux-gnu"
        if machine in ("aarch64", "arm64"):
            return "aarch64-unknown-linux-gnu"

    raise RuntimeError(
        f"xiaoguai: unsupported platform {system}/{machine}. "
        "Supported: macOS arm64/x86_64, Linux x86_64/aarch64. "
        "Build from source via `cargo install --path crates/xiaoguai-cli`."
    )


def find_binary() -> Optional[Path]:
    """Return the absolute path to the bundled binary, or ``None``.

    ``None`` means package data does not include a binary for this
    platform — typically a fresh editable install before the CI step
    that stages the binary has run. Callers should print a clear
    error rather than crashing on the missing file.
    """
    try:
        triple = current_target_triple()
    except RuntimeError:
        return None

    candidate_name = "xiaoguai.exe" if os.name == "nt" else "xiaoguai"
    candidate = _binary_root() / triple / candidate_name

    if candidate.is_file() and os.access(candidate, os.X_OK):
        return candidate

    # Some package layouts (older cibuildwheel cache shapes) drop the
    # binary directly under _binary/ without the triple subdir. Try
    # that as a fallback before giving up.
    flat = _binary_root() / candidate_name
    if flat.is_file() and os.access(flat, os.X_OK):
        return flat

    return None


def _debug_log(msg: str) -> None:
    """Emit a debug line to stderr when ``XIAOGUAI_PY_DEBUG`` is set.

    Used by :mod:`xiaoguai.__main__` to help diagnose "I installed it
    but it doesn't run" reports without spamming normal output.
    """
    if os.environ.get("XIAOGUAI_PY_DEBUG"):
        print(f"[xiaoguai-py] {msg}", file=sys.stderr)
