#!/usr/bin/env python3
"""Sanity-check an executed LTM discovery audit notebook.

Fails loudly on any cell error, on a code cell that produced no output, and on
a missing figure -- a notebook that renders but computed nothing is exactly the
failure mode a visual skim misses. Also checks that every claim the audit rests
on is still printed by some cell: an audit whose evidence lines have silently
disappeared reads the same as one that still supports its conclusions.

Dumps the marked outputs so the findings can be read without opening Jupyter,
and saves the dominance figure beside the notebook for a quick look.

    src/pysimlin/.venv/bin/python notebooks/verify_ltm_discovery_audit.py

`--model clearn`, `--model cross_agg` or `--model wrld3` verifies one; the default verifies every
notebook the generator can produce. Exits non-zero on any failure.
"""

from __future__ import annotations

import argparse
import base64
import re
import sys
from pathlib import Path

import nbformat

NOTEBOOKS_DIR = Path(__file__).resolve().parent
MODELS = ("clearn", "cross_agg", "wrld3")

# Each key is a substring of a line the notebook prints, and its value names
# the claim that line is the evidence for. These are the audit's load-bearing
# outputs: if one stops being printed, the notebook has stopped supporting its
# own verdict even if every cell still runs green.
MARKERS = {
    "enumeration_complete": "the engine's own completeness flag",
    "pysimlin/dump completeness cross-check": (
        "pysimlin.Analysis's enumeration_complete/retained_loops/universe_loops "
        "agree with the dump's own copies"
    ),
    "elementary cycles ever simultaneously active": "the independent universe size",
    "retention survivors": "how many cycles clear the 0.1% threshold",
    "engine loops absent from the independent universe": "no fabricated loops",
    "reported-200 overlap": "the engine's list vs the independent ranking",
    "max relative difference in raw loop scores": "raw score agreement",
    "max |rel score| difference": "relative score agreement",
    "step-dominant coverage": "how often the dominant loop is reported",
    "AUDIT VERDICT": "the notebook's own machine-readable pass/fail predicate",
}


def verify(path: Path) -> bool:
    if not path.exists():
        print(f"MISSING {path} -- run build_ltm_discovery_audit.py first")
        return False

    nb = nbformat.read(path, as_version=4)
    code_cells = [c for c in nb.cells if c.cell_type == "code"]
    errors: list[str] = []
    silent: list[int] = []
    figures = 0
    found = {k: False for k in MARKERS}

    for i, cell in enumerate(code_cells):
        outs = cell.get("outputs", [])
        for out in outs:
            if out.output_type == "error":
                errors.append(f"cell {i}: {out.ename}: {out.evalue[:300]}")
            if "image/png" in out.get("data", {}):
                figures += 1
        if not outs:
            silent.append(i)

    marked_texts: list[str] = []
    for cell in code_cells:
        for out in cell.get("outputs", []):
            text = out.get("text", "") if out.output_type == "stream" else ""
            if not text:
                continue
            if any(k in text for k in MARKERS):
                marked_texts.append(text.rstrip())
            for key in MARKERS:
                if key in text:
                    found[key] = True

    print(f"\n=== {path.name} ===")
    print(f"cells: {len(nb.cells)} ({len(code_cells)} code), figures: {figures}")
    for e in errors:
        print(f"ERROR {e}")
    if silent:
        print(f"code cells with NO output: {silent}")

    print("\n--- marked outputs ---")
    for text in marked_texts:
        print(text)
        print("-" * 70)

    missing = [MARKERS[k] for k, ok in found.items() if not ok]
    if missing:
        print("\nMISSING evidence for:")
        for m in missing:
            print(f"  - {m}")

    for cell in code_cells:
        if "plt.subplots" not in cell.source:
            continue
        for out in cell.get("outputs", []):
            if "image/png" in out.get("data", {}):
                preview = path.with_suffix(".png")
                preview.write_bytes(base64.b64decode(out["data"]["image/png"]))
                print(f"\nsaved {preview}")

    # Markers only prove the notebook still PRINTS its evidence; the values
    # have to say what the verdict claims. The notebook emits one predicate
    # line (`AUDIT VERDICT: PASS|FAIL (...)`) that folds enumeration
    # completeness, set exactness, score agreement and the independent count
    # equalities together; verification fails unless it reads PASS, and also
    # re-checks the headline numbers it summarizes.
    verdict_pass = False
    value_failures: list[str] = []
    all_text = "\n".join(marked_texts)
    for line in all_text.splitlines():
        if line.startswith("AUDIT VERDICT:"):
            verdict_pass = "PASS" in line.split("(")[0]
        m = re.match(r"engine loops absent from the independent universe: (\d+)", line)
        if m and int(m.group(1)) != 0:
            value_failures.append(line)
        m = re.match(r"reported-200 overlap: (\d+)/(\d+)", line)
        if m and m.group(1) != m.group(2):
            value_failures.append(line)
        for key in ("max relative difference in raw loop scores",
                    "max |rel score| difference"):
            m = re.match(re.escape(key) + r": ([0-9.e+-]+)", line)
            if m and float(m.group(1)) > 1e-9:
                value_failures.append(line)
        if line.startswith("enumeration_complete:") and "True" not in line:
            value_failures.append(line)
        if "cross-check" in line and "DISAGREE" in line:
            value_failures.append(line)
    if not verdict_pass:
        print("\nAUDIT VERDICT line missing or not PASS")
    for f in value_failures:
        print(f"VALUE CHECK FAILED: {f}")

    ok = not (errors or missing or silent or figures == 0 or value_failures) and verdict_pass
    print("OK" if ok else "FAILED")
    return ok


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--model", choices=MODELS, action="append",
        help="model notebook to verify (repeatable); default: all",
    )
    args = parser.parse_args()
    keys = args.model or list(MODELS)
    results = {
        key: verify(NOTEBOOKS_DIR / f"ltm_discovery_audit_{key}.ipynb") for key in keys
    }
    print("\n=== summary ===")
    for key, ok in results.items():
        print(f"{key}: {'OK' if ok else 'FAILED'}")
    return 0 if all(results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
