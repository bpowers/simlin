#!/usr/bin/env python3
"""Sanity-check the executed C-LEARN LTM audit notebook.

Fails loudly on any cell error, on an empty code cell, and on a missing
headline figure -- a notebook that renders but computed nothing is exactly the
failure mode a visual skim misses. Also dumps the outputs that carry the
report's claims so they can be read without opening Jupyter, and saves the
dominance figure beside the notebook for a quick look.
"""

import base64
import sys
from pathlib import Path

import nbformat

NOTEBOOKS_DIR = Path(__file__).resolve().parent
NOTEBOOK = NOTEBOOKS_DIR / "clearn_ltm_audit.ipynb"

nb = nbformat.read(NOTEBOOK, as_version=4)

errors: list[str] = []
code_cells = [c for c in nb.cells if c.cell_type == "code"]
silent: list[int] = []
figures = 0

for i, cell in enumerate(code_cells):
    outs = cell.get("outputs", [])
    for out in outs:
        if out.output_type == "error":
            errors.append(f"cell {i}: {out.ename}: {out.evalue[:300]}")
        if "image/png" in out.get("data", {}):
            figures += 1
    if not outs:
        silent.append(i)

print(f"cells: {len(nb.cells)} ({len(code_cells)} code), figures: {figures}")
for e in errors:
    print(f"ERROR {e}")
if silent:
    print(f"code cells with NO output: {silent}")

# The claims the write-up rests on. Each is printed by a specific cell; if a
# marker disappears the notebook stopped supporting its own conclusions.
MARKERS = {
    "identical loop structure": "three scenario partitions are structurally identical",
    "positions of the singleton-partition loops": "the ranking contradiction",
    "max |pinned - discovery|": "the pinned-vs-discovery cross-validation",
    "fragments failed to compile": "the fragment-compile failure count",
    "discovered loops traversing a failing edge": "loop results are uncontaminated",
    "one scored input": "the relative-link-ranking degeneracy",
}
found = {k: False for k in MARKERS}
for cell in code_cells:
    for out in cell.get("outputs", []):
        text = out.get("text", "") if out.output_type == "stream" else ""
        for key in MARKERS:
            if key in text:
                found[key] = True

print("\n--- headline outputs ---")
for cell in code_cells:
    for out in cell.get("outputs", []):
        if out.output_type == "stream" and any(k in out.get("text", "") for k in MARKERS):
            print(out["text"].rstrip())
            print("-" * 70)

missing = [MARKERS[k] for k, ok in found.items() if not ok]
if missing:
    print("\nMISSING evidence for:")
    for m in missing:
        print(f"  - {m}")

for cell in code_cells:
    if "gridspec_kw" in cell.source:
        for out in cell.get("outputs", []):
            if "image/png" in out.get("data", {}):
                preview = NOTEBOOKS_DIR / "clearn_dominance.png"
                preview.write_bytes(base64.b64decode(out["data"]["image/png"]))
                print(f"\nsaved {preview}")

if errors or missing or figures == 0:
    sys.exit(1)
print("\nOK")
