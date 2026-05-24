"""``python -m xiaoguai`` and ``xiaoguai`` console-script entry point.

The launcher does exactly one thing: locate the bundled native binary
and ``execvp`` (POSIX) / ``subprocess`` (Windows) into it, forwarding
every argument from :data:`sys.argv` and inheriting the current
environment.

We use ``os.execvp`` on POSIX so:

* Signal handling stays clean — ``Ctrl+C`` reaches the Rust process
  directly without a Python wrapper eating the ``SIGINT``.
* The Python interpreter is replaced (no double process tree).
* Exit codes are propagated naturally.

On Windows ``execvp`` semantics are different (the parent process
exits before the child finishes when launched from cmd.exe), so we
fall back to ``subprocess.run`` and forward the return code.
"""

from __future__ import annotations

import os
import subprocess
import sys
from typing import NoReturn, Sequence

from ._binary import _debug_log, find_binary

__all__ = ["main"]

_MISSING_BINARY_MSG = (
    "xiaoguai: native binary not bundled in this wheel.\n"
    "This usually means you installed the package from source without\n"
    "the CI step that stages the Rust-built binary. Options:\n"
    "  1. Install a published wheel: pip install --upgrade xiaoguai\n"
    "  2. Build from source: cargo install --path crates/xiaoguai-cli\n"
    "  3. Stage a binary manually under\n"
    "     <site-packages>/xiaoguai/_binary/<target>/xiaoguai\n"
    "Set XIAOGUAI_PY_DEBUG=1 to see resolution details.\n"
)


def _run(argv: Sequence[str]) -> int:
    """Invoke the bundled binary with ``argv``. Returns its exit code."""
    binary = find_binary()
    if binary is None:
        sys.stderr.write(_MISSING_BINARY_MSG)
        return 2

    _debug_log(f"resolved binary: {binary}")
    _debug_log(f"forwarding argv: {list(argv)}")

    if os.name == "nt":
        # subprocess.run preserves the parent on Windows where
        # execvp would prematurely return control to cmd.exe.
        completed = subprocess.run([str(binary), *argv], check=False)
        return completed.returncode

    # POSIX: hand the process off entirely. os.execvp does not return.
    os.execvp(str(binary), [str(binary), *argv])
    # Unreachable on POSIX but mypy / pylint want a return.
    return 0


def main(argv: Sequence[str] | None = None) -> NoReturn:
    """Console-script entry point.

    Args:
        argv: Optional explicit argv (mostly for tests). When ``None``
            we read :data:`sys.argv[1:]`.
    """
    args = list(sys.argv[1:] if argv is None else argv)
    code = _run(args)
    sys.exit(code)


if __name__ == "__main__":  # pragma: no cover — module entry point
    main()
