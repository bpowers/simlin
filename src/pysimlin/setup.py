#!/usr/bin/env python
"""Setup for simlin Python package (CFFI extension).

Besides declaring the CFFI module, this refuses to build a wheel or sdist
whose notebook-widget assets (``simlin/_widget/widget.js``,
``libsimlin-browser.wasm``, ``ASSETS.json``) are missing, empty, or do not
match their manifest. Those files are build outputs of the TypeScript
workspace (staged by ``scripts/stage_widget_assets.py``), not sources, so
nothing else in the packaging pipeline would notice their absence: setuptools
happily builds a wheel without optional package data, and the result would
be a pysimlin whose ``Model.widget()`` raises on every install.

Only ``bdist_wheel`` and ``sdist`` are guarded. ``build_ext --inplace`` (the
test suite's path) must keep working in a checkout that has never built the
widget.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.sdist import sdist as _sdist

try:
    from setuptools.command.bdist_wheel import bdist_wheel as _bdist_wheel
except ImportError:  # setuptools < 70.1 has no bdist_wheel of its own
    from wheel.bdist_wheel import bdist_wheel as _bdist_wheel  # type: ignore[no-redef]

HERE = Path(__file__).resolve().parent
ASSET_DIR = HERE / "simlin" / "_widget"
STAGING_SCRIPT = HERE / "scripts" / "stage_widget_assets.py"

# Escape hatch for deliberately building a package without the widget
# (e.g. measuring what the assets cost). Never set this in a release path.
ALLOW_MISSING_ENV = "SIMLIN_ALLOW_MISSING_WIDGET_ASSETS"


def _head_commit() -> str | None:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=HERE, check=True, capture_output=True, text=True
        )
    except (OSError, subprocess.CalledProcessError):
        # Not a git checkout (an sdist unpack): the manifest's commit is
        # informational only there.
        return None
    return proc.stdout.strip()


def _require_widget_assets(command: str) -> None:
    if os.environ.get(ALLOW_MISSING_ENV) == "1":
        print(f"warning: {ALLOW_MISSING_ENV}=1: {command} without checking widget assets")
        return
    sys.path.insert(0, str(STAGING_SCRIPT.parent))
    try:
        import stage_widget_assets as staging
    finally:
        sys.path.pop(0)

    verification = staging.verify_staged(ASSET_DIR, head_commit=_head_commit())
    for warning in verification.warnings:
        print(f"warning: widget assets: {warning}")
    if not verification.ok:
        problems = "\n".join(f"  - {e}" for e in verification.errors)
        raise SystemExit(
            f"refusing to run {command}: the notebook widget assets are not staged:\n"
            f"{problems}\n"
            f"Run `python {STAGING_SCRIPT.relative_to(HERE)}` (or `make assets`) first, "
            f"or set {ALLOW_MISSING_ENV}=1 to build a package without the widget."
        )


class bdist_wheel(_bdist_wheel):  # type: ignore[misc]
    def run(self) -> None:
        _require_widget_assets("bdist_wheel")
        super().run()


class sdist(_sdist):  # type: ignore[misc]
    def run(self) -> None:
        _require_widget_assets("sdist")
        super().run()


setup(
    name="simlin",
    cffi_modules=["simlin/_ffi_build.py:ffibuilder"],
    cmdclass={"bdist_wheel": bdist_wheel, "sdist": sdist},
)
