"""Tests for the pure half of ``scripts/check_notebook_export.py``: the
executed-notebook inspection, the widget-state stripping, and the HTML
inspection under both arms (widget state present / absent).  The shell
(running nbconvert) is exercised by ``make export-check`` and the CI
``pysimlin-e2e`` job, where nbconvert is guaranteed.
"""

from __future__ import annotations

import base64
import importlib.util
import sys
from typing import TYPE_CHECKING, Any

import pytest

from .conftest import get_repo_root

if TYPE_CHECKING:
    from types import ModuleType

SCRIPTS = get_repo_root() / "src" / "pysimlin" / "scripts"
WIDGET_VIEW = "application/vnd.jupyter.widget-view+json"
WIDGET_STATE = "application/vnd.jupyter.widget-state+json"
SVG_TEXT = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect/></svg>'
MODEL_ID = "3221344213124e5680b80ccbb8a3429b"


@pytest.fixture(scope="module")
def checker() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "check_notebook_export", SCRIPTS / "check_notebook_export.py"
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _display_output(*, svg: bool = True) -> dict[str, Any]:
    data: dict[str, Any] = {
        WIDGET_VIEW: {"model_id": MODEL_ID, "version_major": 2, "version_minor": 1},
        "text/plain": ["<simlin.Model>"],
    }
    if svg:
        # nbformat stores long strings as lists of lines.
        data["image/svg+xml"] = [SVG_TEXT]
    return {"output_type": "execute_result", "data": data, "metadata": {}, "execution_count": 2}


def _notebook(outputs: list[dict[str, Any]], *, state: dict[str, Any] | None) -> dict[str, Any]:
    metadata: dict[str, Any] = {}
    if state is not None:
        metadata["widgets"] = {
            WIDGET_STATE: {"state": state, "version_major": 2, "version_minor": 0}
        }
    return {
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": metadata,
        "cells": [{"cell_type": "code", "source": "m", "outputs": outputs, "metadata": {}}],
    }


def _anywidget_state(esm: str = "export default {}") -> dict[str, Any]:
    return {
        MODEL_ID: {
            "model_name": "AnyModel",
            "model_module": "anywidget",
            "model_module_version": "~0.11.*",
            "state": {"_esm": esm, "revision": 0},
        }
    }


class TestInspectExecuted:
    def test_clean_display_cell_with_stored_state(self, checker: ModuleType) -> None:
        report = checker.inspect_executed(_notebook([_display_output()], state=_anywidget_state()))
        assert report.errors == []
        assert report.problems == []
        assert report.display_cells == {MODEL_ID: SVG_TEXT}

    def test_error_output_is_reported(self, checker: ModuleType) -> None:
        error = {"output_type": "error", "ename": "ValueError", "evalue": "boom", "traceback": []}
        report = checker.inspect_executed(
            _notebook([error, _display_output()], state=_anywidget_state())
        )
        assert report.errors == ["ValueError: boom"]

    def test_widget_view_without_svg_is_a_problem(self, checker: ModuleType) -> None:
        report = checker.inspect_executed(
            _notebook([_display_output(svg=False)], state=_anywidget_state())
        )
        assert any("no image/svg+xml fallback" in p for p in report.problems)
        assert any("no output carries both" in p for p in report.problems)

    def test_no_display_cell_is_a_problem(self, checker: ModuleType) -> None:
        stream = {"output_type": "stream", "name": "stdout", "text": "hi"}
        report = checker.inspect_executed(_notebook([stream], state=None))
        assert report.problems == [
            "no output carries both application/vnd.jupyter.widget-view+json and image/svg+xml"
        ]

    @pytest.mark.parametrize(
        ("state", "expected"),
        [
            (None, "widget state has no entry"),
            ({}, "widget state has no entry"),
            (
                {MODEL_ID: {"model_module": "@jupyter-widgets/base", "state": {"_esm": "x"}}},
                "expected 'anywidget'",
            ),
            (_anywidget_state(esm=""), "stored without its _esm"),
        ],
    )
    def test_stored_state_must_hold_the_anywidget_model_with_its_esm(
        self, checker: ModuleType, state: dict[str, Any] | None, expected: str
    ) -> None:
        report = checker.inspect_executed(_notebook([_display_output()], state=state))
        assert any(expected in p for p in report.problems), report.problems


class TestStripWidgetState:
    def test_removes_only_the_widgets_metadata_and_leaves_the_input_alone(
        self, checker: ModuleType
    ) -> None:
        nb = _notebook([_display_output()], state=_anywidget_state())
        nb["metadata"]["kernelspec"] = {"name": "python3"}
        stripped = checker.strip_widget_state(nb)
        assert "widgets" not in stripped["metadata"]
        assert stripped["metadata"]["kernelspec"] == {"name": "python3"}
        assert stripped["cells"] == nb["cells"]
        assert "widgets" in nb["metadata"]

    def test_no_state_is_a_no_op(self, checker: ModuleType) -> None:
        nb = _notebook([_display_output()], state=None)
        assert checker.strip_widget_state(nb) == nb


def _html(*, view: bool, state: bool, svg: str | None) -> str:
    parts = ["<html><body>"]
    if view:
        parts.append(
            '<div class="jupyter-widgets"><script type="application/vnd.jupyter.widget-view+json">'
            f'{{"model_id": "{MODEL_ID}", "version_major": 2}}</script></div>'
        )
    if state:
        parts.append(f'<script type="{WIDGET_STATE}">{{"state": {{}}}}</script>')
    if svg is not None:
        parts.append(
            '<div class="jp-RenderedSVG jp-OutputArea-output" data-mime-type="image/svg+xml">'
            f"{svg}</div>"
        )
    parts.append("</body></html>")
    return "".join(parts)


class TestInspectHtml:
    def test_with_state_expects_view_and_state(self, checker: ModuleType) -> None:
        html = _html(view=True, state=True, svg=None)
        assert checker.inspect_html(html, [MODEL_ID], [SVG_TEXT], with_state=True) == []

    def test_with_state_missing_view_or_state(self, checker: ModuleType) -> None:
        problems = checker.inspect_html(
            _html(view=False, state=False, svg=None), [MODEL_ID], [SVG_TEXT], with_state=True
        )
        assert any("no widget view" in p for p in problems)
        assert any("does not embed the widget state" in p for p in problems)

    @pytest.mark.parametrize(
        "svg_html",
        [
            SVG_TEXT,
            '<img src="data:image/svg+xml;base64,'
            + base64.b64encode(SVG_TEXT.encode()).decode()
            + '">',
        ],
        ids=["inline", "data-uri"],
    )
    def test_without_state_accepts_the_svg_inline_or_as_a_data_uri(
        self, checker: ModuleType, svg_html: str
    ) -> None:
        html = _html(view=False, state=False, svg=svg_html)
        assert checker.inspect_html(html, [MODEL_ID], [SVG_TEXT], with_state=False) == []

    def test_without_state_rejects_a_leftover_view_or_state(self, checker: ModuleType) -> None:
        problems = checker.inspect_html(
            _html(view=True, state=True, svg=SVG_TEXT), [MODEL_ID], [SVG_TEXT], with_state=False
        )
        assert any("still references widget" in p for p in problems)
        assert any("still embeds a widget state" in p for p in problems)

    def test_without_state_requires_the_display_cells_own_svg(self, checker: ModuleType) -> None:
        # An `<svg` from somewhere else (the embedded _esm's React source, a
        # different cell) does not count: the marker is the display cell's
        # own bytes.
        other = '<svg aria-hidden="true" style="display:none"></svg>'
        problems = checker.inspect_html(
            _html(view=False, state=False, svg=other), [MODEL_ID], [SVG_TEXT], with_state=False
        )
        assert problems == ["HTML without widget state does not carry the display cell's SVG"]
        problems = checker.inspect_html(
            _html(view=False, state=False, svg=None), [MODEL_ID], [SVG_TEXT], with_state=False
        )
        assert any("no image/svg+xml output area" in p for p in problems)

    def test_svg_markers(self, checker: ModuleType) -> None:
        # The inline marker ignores leading whitespace; the data URI is the
        # exact bytes, which is what nbconvert base64-encodes.
        inline, data_uri = checker.svg_markers("  " + SVG_TEXT)
        assert inline == SVG_TEXT[:60]
        assert data_uri.startswith("data:image/svg+xml;base64,")
        assert base64.b64decode(data_uri.split(",", 1)[1]).decode() == "  " + SVG_TEXT
