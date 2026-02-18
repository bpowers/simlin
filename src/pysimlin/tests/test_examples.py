"""Smoke tests that run each example script as a subprocess.

Each example is expected to exit with code 0.  The working directory is
set to the ``examples/`` folder so that relative fixture paths resolve
correctly.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

EXAMPLES_DIR = Path(__file__).resolve().parent.parent / "examples"
EXAMPLE_SCRIPTS = sorted(EXAMPLES_DIR.glob("*.py"))


@pytest.mark.parametrize(
    "script",
    EXAMPLE_SCRIPTS,
    ids=[s.stem for s in EXAMPLE_SCRIPTS],
)
def test_example_runs(script: Path) -> None:
    result = subprocess.run(
        [sys.executable, str(script)],
        cwd=str(EXAMPLES_DIR),
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert result.returncode == 0, (
        f"{script.name} failed (exit {result.returncode}):\n"
        f"--- stdout ---\n{result.stdout}\n"
        f"--- stderr ---\n{result.stderr}"
    )
