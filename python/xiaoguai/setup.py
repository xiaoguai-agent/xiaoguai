"""Force a platform-specific wheel tag.

The package ships no compiled Python extension — it bundles a prebuilt
native ``xiaoguai`` binary under ``src/xiaoguai/_binary/<target>/`` as
package data (staged by ``.github/workflows/pip-wheel.yml``). With only
package data, setuptools would emit a *pure-Python* ``py3-none-any``
wheel, which is wrong: the wheel is platform-specific (it carries a
single arch's binary) and ``cibuildwheel`` aborts with "a pure-Python
wheel was generated".

We override ``bdist_wheel`` to:

* mark the build impure (``root_is_pure = False``) so the wheel gets a
  platform tag instead of ``any``; and
* keep the interpreter tag broad (``py3``/``none``) — the bundled
  launcher is a thin ``execvp`` stub that doesn't touch the Python ABI,
  so one wheel per platform serves every CPython 3.x.

Result: ``xiaoguai-<ver>-py3-none-<platform>.whl`` — exactly the shape
the cibuildwheel comment in ``pyproject.toml`` describes.

Everything else (name, version, package data) stays declared in
``pyproject.toml``; this file exists only for the wheel-tag override.
"""

from __future__ import annotations

from setuptools import setup

try:
    # setuptools >= 70.1 vendors bdist_wheel; fall back to the wheel pkg.
    from setuptools.command.bdist_wheel import bdist_wheel as _bdist_wheel
except ImportError:  # pragma: no cover - older setuptools
    from wheel.bdist_wheel import bdist_wheel as _bdist_wheel


class bdist_wheel(_bdist_wheel):
    """Emit a platform-tagged, interpreter-agnostic wheel."""

    def finalize_options(self) -> None:
        super().finalize_options()
        # Impure -> the wheel platform tag becomes macosx_*/manylinux_*
        # instead of "any".
        self.root_is_pure = False

    def get_tag(self) -> tuple[str, str, str]:
        # Drop the interpreter/ABI specificity that root_is_pure=False
        # would otherwise introduce (cp310-cp310-...): the binary
        # launcher is ABI-independent, so py3-none-<plat> is correct and
        # lets a single wheel install on any CPython 3.x for the platform.
        _python, _abi, plat = super().get_tag()
        return "py3", "none", plat


setup(cmdclass={"bdist_wheel": bdist_wheel})
