#!/usr/bin/env python3
"""Sanity-check the executed notebook: no errors, key outputs present.

Prints the dominance headline numbers and saves the dominance plot beside the
notebook as dominance_preview.png for a quick visual check.
"""

import base64
from pathlib import Path

import nbformat

NOTEBOOKS_DIR = Path(__file__).resolve().parent
nb = nbformat.read(NOTEBOOKS_DIR / "clearn_ltm_experience.ipynb", as_version=4)

n_errors = 0
for cell in nb.cells:
    if cell.cell_type != "code":
        continue
    for out in cell.get("outputs", []):
        if out.output_type == "error":
            n_errors += 1
            print(f"ERROR: {out.ename}: {out.evalue[:200]}")

print(f"cells: {len(nb.cells)}, errors: {n_errors}")

for cell in nb.cells:
    if cell.cell_type != "code":
        continue
    if "b_total" in cell.source:
        for out in cell.get("outputs", []):
            if out.output_type == "stream":
                print("\n--- dominance headline numbers ---")
                print(out.text)
    if "stackplot" in cell.source:
        for out in cell.get("outputs", []):
            if "image/png" in out.get("data", {}):
                preview = NOTEBOOKS_DIR / "dominance_preview.png"
                preview.write_bytes(base64.b64decode(out["data"]["image/png"]))
                print(f"saved {preview}")
