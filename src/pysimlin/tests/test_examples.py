"""Smoke tests that run each example script (and notebook) as a subprocess.

Each example is expected to exit with code 0.  The working directory is
set to the ``examples/`` folder so that relative fixture paths resolve
correctly.
"""

from __future__ import annotations

import json
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


NOTEBOOKS = sorted(EXAMPLES_DIR.glob("*.ipynb"))


@pytest.mark.parametrize("notebook", NOTEBOOKS, ids=[n.stem for n in NOTEBOOKS])
def test_example_notebook_is_clean_and_runs(notebook: Path) -> None:
    """Example notebooks are committed without outputs (so diffs stay
    readable) and their code cells run top to bottom as a plain script.

    Executing the cells as one script -- rather than through nbclient and a
    kernel -- keeps this inside the dev extra; a bare ``m`` expression cell
    is then a no-op instead of a display, and the interactive rendering is
    covered by the JupyterLab journey (``make e2e``), which also executes
    the notebook headless in CI.
    """
    nb = json.loads(notebook.read_text(encoding="utf-8"))
    assert nb["nbformat"] == 4, f"{notebook.name}: expected nbformat 4"
    code_cells = [cell for cell in nb["cells"] if cell["cell_type"] == "code"]
    assert code_cells, f"{notebook.name}: no code cells"
    for cell in code_cells:
        assert cell["outputs"] == [], f"{notebook.name}: commit notebooks without outputs"
        assert cell["execution_count"] is None, (
            f"{notebook.name}: commit notebooks without execution counts"
        )
    script = "\n\n".join("".join(cell["source"]) for cell in code_cells)
    result = subprocess.run(
        [sys.executable, "-c", script],
        cwd=str(EXAMPLES_DIR),
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert result.returncode == 0, (
        f"{notebook.name} failed (exit {result.returncode}):\n"
        f"--- stdout ---\n{result.stdout}\n"
        f"--- stderr ---\n{result.stderr}"
    )
    assert "Traceback" not in result.stderr
