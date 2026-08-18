#!/usr/bin/env python3
"""Execute an example notebook headless and check what its static exports
carry: the notebook editor's display cell must survive nbconvert both as
the interactive widget (when the widget state is stored) and as the SVG
diagram (when it is not).

pattern: Functional Core (``inspect_executed``, ``inspect_html``) +
Imperative Shell (running ``jupyter nbconvert``, files)

``check_notebook_export.py NOTEBOOK --output-dir DIR``

1. ``jupyter nbconvert --to notebook --execute``: every cell runs clean (no
   ``error`` output) and at least one ``execute_result`` carries BOTH
   ``application/vnd.jupyter.widget-view+json`` and ``image/svg+xml`` -- the
   bundle ``Model._repr_mimebundle_`` produces; nbclient stores the widget
   state (default ``store_widget_state=True``) into ``metadata.widgets``, and
   that state must hold the anywidget model with a non-empty ``_esm``.
2. ``--to html`` of that executed notebook: nbconvert's display priority
   puts the widget view first and, because the state is present, renders
   the widget-view ``<div>`` for our model id and embeds the state (the
   ``_esm``, about 1.6 MB, rides along) -- the "live" export, which needs
   the ipywidgets html-manager and anywidget from a CDN when opened.
3. ``--to html`` of the same notebook with ``metadata.widgets`` removed --
   what a static renderer without widget state sees (nbconvert of a
   notebook saved without state, GitHub, a viewer offline):
   ``WidgetsDataTypeFilter`` skips the widget-view mimetype, so the display
   cell must fall back to the SVG -- nbconvert's lab template embeds it as
   an ``<img src="data:image/svg+xml;base64,...">`` in an
   ``image/svg+xml`` output area -- and carry no widget-view div.

Exit status 1 with the problems on stderr on any failure.  Needs
``jupyter`` with nbconvert and a kernel on ``PATH`` (pysimlin's ``e2e`` or
``notebooks`` extra); it is deliberately NOT a pytest (the dev extra has no
nbconvert, and a test that skips silently proves nothing) -- ``make
export-check`` and the ``pysimlin-e2e`` CI job run it.
"""

from __future__ import annotations

import argparse
import base64
import json
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

WIDGET_VIEW = "application/vnd.jupyter.widget-view+json"
WIDGET_STATE = "application/vnd.jupyter.widget-state+json"
SVG = "image/svg+xml"


# ── functional core ─────────────────────────────────────────────────────


@dataclass
class ExecutedReport:
    errors: list[str] = field(default_factory=list)
    problems: list[str] = field(default_factory=list)
    # model_id -> the SVG text in the same output, for the display cells.
    display_cells: dict[str, str] = field(default_factory=dict)


def inspect_executed(nb: dict[str, Any]) -> ExecutedReport:
    """Check an executed notebook: no error outputs, at least one output
    carrying both the widget view and the SVG, and stored widget state
    holding an anywidget model with a non-empty ``_esm`` for each of them."""
    report = ExecutedReport()
    outputs = [o for c in nb["cells"] if c["cell_type"] == "code" for o in c.get("outputs", [])]
    for out in outputs:
        if out["output_type"] == "error":
            report.errors.append(f"{out['ename']}: {out['evalue']}")
    for out in outputs:
        if out["output_type"] not in ("execute_result", "display_data"):
            continue
        data = out.get("data", {})
        if WIDGET_VIEW in data:
            if SVG not in data:
                report.problems.append(
                    f"a widget-view output has no {SVG} fallback beside it: {sorted(data)}"
                )
                continue
            svg = data[SVG]
            svg_text = "".join(svg) if isinstance(svg, list) else str(svg)
            report.display_cells[data[WIDGET_VIEW]["model_id"]] = svg_text
    if not report.display_cells:
        report.problems.append(f"no output carries both {WIDGET_VIEW} and {SVG}")
    state = nb.get("metadata", {}).get("widgets", {}).get(WIDGET_STATE, {}).get("state", {})
    for model_id in report.display_cells:
        model = state.get(model_id)
        if model is None:
            report.problems.append(f"widget state has no entry for displayed model {model_id}")
            continue
        if model.get("model_module") != "anywidget":
            report.problems.append(
                f"model {model_id} is {model.get('model_module')!r}, expected 'anywidget'"
            )
        if not model.get("state", {}).get("_esm"):
            report.problems.append(f"model {model_id} stored without its _esm")
    return report


def strip_widget_state(nb: dict[str, Any]) -> dict[str, Any]:
    """The notebook as a host that did not save widget state would hold it."""
    stripped = json.loads(json.dumps(nb))
    stripped.get("metadata", {}).pop("widgets", None)
    return stripped


def svg_markers(svg_text: str) -> tuple[str, str]:
    """The two forms nbconvert may embed an ``image/svg+xml`` output in: the
    document inline, or (the lab template's default) as a base64 data URI
    ``<img>``.  Matching either on the SVG's own bytes is what distinguishes
    the display cell's diagram from the ``<svg`` strings inside the
    embedded ``_esm``'s React source."""
    inline = svg_text.strip()[:60]
    data_uri = "data:image/svg+xml;base64," + base64.b64encode(svg_text.encode("utf-8")).decode()
    return inline, data_uri


def inspect_html(
    html: str, model_ids: list[str], svg_texts: list[str], *, with_state: bool
) -> list[str]:
    """Problems with an HTML export.  ``with_state``: the widget-view divs
    for ``model_ids`` and the embedded state must be present.  Without: no
    widget-view div, and each display cell's SVG must be present in an
    ``image/svg+xml`` output area."""
    problems: list[str] = []
    for model_id in model_ids:
        view_marker = f'"model_id": "{model_id}"'
        has_view = view_marker in html
        if with_state and not has_view:
            problems.append(f"HTML with widget state has no widget view for {model_id}")
        if not with_state and has_view:
            problems.append(f"HTML without widget state still references widget {model_id}")
    has_state = f'<script type="{WIDGET_STATE}">' in html
    if with_state and not has_state:
        problems.append("HTML with widget state does not embed the widget state script")
    if not with_state and has_state:
        problems.append("HTML without widget state still embeds a widget state script")
    if not with_state:
        if 'data-mime-type="image/svg+xml"' not in html:
            problems.append("HTML without widget state has no image/svg+xml output area")
        for svg_text in svg_texts:
            inline, data_uri = svg_markers(svg_text)
            if inline not in html and data_uri not in html:
                problems.append("HTML without widget state does not carry the display cell's SVG")
    return problems


# ── imperative shell ────────────────────────────────────────────────────


def nbconvert(*args: str) -> None:
    cmd = [sys.executable, "-m", "nbconvert", *args]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise SystemExit(f"nbconvert failed: {' '.join(cmd)}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("notebook", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args(argv)
    notebook: Path = args.notebook.resolve()
    out: Path = args.output_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)

    executed = out / f"{notebook.stem}.executed.ipynb"
    nbconvert(
        "--to",
        "notebook",
        "--execute",
        "--output",
        str(executed),
        str(notebook),
        "--ExecutePreprocessor.cwd",
        str(notebook.parent),
    )
    nb = json.loads(executed.read_text(encoding="utf-8"))
    report = inspect_executed(nb)
    problems = [*(f"cell raised {e}" for e in report.errors), *report.problems]

    model_ids = list(report.display_cells)
    svg_texts = list(report.display_cells.values())

    with_state_html = out / f"{notebook.stem}.with-widget-state.html"
    nbconvert("--to", "html", "--output", str(with_state_html), str(executed))
    problems += inspect_html(
        with_state_html.read_text(encoding="utf-8"), model_ids, svg_texts, with_state=True
    )

    stripped = out / f"{notebook.stem}.no-widget-state.ipynb"
    stripped.write_text(json.dumps(strip_widget_state(nb), indent=1), encoding="utf-8")
    no_state_html = out / f"{notebook.stem}.no-widget-state.html"
    nbconvert("--to", "html", "--output", str(no_state_html), str(stripped))
    problems += inspect_html(
        no_state_html.read_text(encoding="utf-8"), model_ids, svg_texts, with_state=False
    )

    if problems:
        for problem in problems:
            sys.stderr.write(f"error: {problem}\n")
        return 1
    with_size = with_state_html.stat().st_size
    without_size = no_state_html.stat().st_size
    print(
        f"{notebook.name}: {len(model_ids)} display cell(s); "
        f"with widget state -> widget view + embedded state ({with_size:,} bytes); "
        f"without -> the SVG diagram ({without_size:,} bytes)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
