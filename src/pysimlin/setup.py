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
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.bdist_wheel import bdist_wheel as _bdist_wheel
from setuptools.command.sdist import sdist as _sdist

HERE = Path(__file__).resolve().parent
ASSET_DIR = HERE / "simlin" / "_widget"

sys.path.insert(0, str(HERE / "scripts"))
try:
    from stage_widget_assets import ALLOW_MISSING_ENV, head_commit, packaging_guard
finally:
    sys.path.pop(0)


def _require_widget_assets(command: str) -> None:
    messages, refusal = packaging_guard(
        ASSET_DIR,
        command=command,
        head_commit=head_commit(),
        allow_missing=os.environ.get(ALLOW_MISSING_ENV) == "1",
    )
    for message in messages:
        print(message)
    if refusal is not None:
        raise SystemExit(refusal)


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
