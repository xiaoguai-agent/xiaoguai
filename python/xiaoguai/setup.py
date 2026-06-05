"""Force a platform-specific, interpreter-agnostic wheel.

The package ships no compiled Python extension — it bundles a prebuilt
native ``xiaoguai`` binary under ``src/xiaoguai/_binary/<target>/`` as
package data (staged by ``.github/workflows/pip-wheel.yml``). Two things
must be true for the wheel that results:

1. **It must be a platform wheel, not ``py3-none-any``.** A pure wheel
   would advertise itself as installable everywhere while carrying a
   single arch's binary, and ``cibuildwheel`` aborts on it.

2. **The payload must live in ``platlib``, not ``purelib``.** On Linux
   ``cibuildwheel`` runs ``auditwheel repair``, which rejects an ELF
   binary found under ``purelib`` ("has to be platlib compliant").

Marking the distribution as having ext modules (``has_ext_modules ->
True``) gets us both: setuptools routes the package into ``platlib`` and
tags the wheel impure. We then override the wheel tag back to
``py3-none-<platform>`` because the bundled launcher is a thin ``execvp``
stub that doesn't touch the Python ABI — so one wheel per platform serves
every CPython 3.x (and Linux ``auditwheel repair`` relabels the platform
part to the matching ``manylinux_*`` tag).

Everything else (name, version, package data) stays declared in
``pyproject.toml``; this file exists only for these wheel overrides.
"""

from __future__ import annotations

from setuptools import Distribution, setup

try:
    # setuptools >= 70.1 vendors bdist_wheel; fall back to the wheel pkg.
    from setuptools.command.bdist_wheel import bdist_wheel as _bdist_wheel
except ImportError:  # pragma: no cover - older setuptools
    from wheel.bdist_wheel import bdist_wheel as _bdist_wheel


class BinaryDistribution(Distribution):
    """A distribution that ships a native binary as package data.

    Returning ``True`` here makes setuptools treat the build as impure:
    the payload goes into ``platlib`` and the wheel gets a platform tag.
    """

    def has_ext_modules(self) -> bool:  # noqa: D401 - simple override
        return True


class bdist_wheel(_bdist_wheel):
    """Emit a ``py3-none-<platform>`` tag.

    ``has_ext_modules`` would otherwise pin the wheel to the building
    interpreter (``cp310-cp310-...``); the launcher is ABI-independent,
    so a single ``py3-none-<plat>`` wheel installs on any CPython 3.x.
    """

    def get_tag(self) -> tuple[str, str, str]:
        _python, _abi, plat = super().get_tag()
        return "py3", "none", plat


setup(distclass=BinaryDistribution, cmdclass={"bdist_wheel": bdist_wheel})
