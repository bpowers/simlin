"""pysimlin without the ``[notebook]`` extra: anywidget is BLOCKED here.

The notebook editor is an optional extra so that scripts, MCP servers, and
CI installs that never display a model do not pay for the anywidget /
ipywidgets / IPython chain.  These tests run in the dev venv (which has
anywidget) by making its import fail -- ``sys.modules[name] = None`` is
the documented way to make ``import name`` raise ``ImportError`` -- and
pin what a bare ``pip install pysimlin`` user sees:

    import simlin                 -> fine
    Model.widget()                -> SimlinDependencyError (an ImportError)
                                     naming the pip line
    display(model)                -> the SVG diagram + text/plain, and ONE
                                     RuntimeWarning with the same pip line;
                                     never a traceback
    the pip line under Colab      -> the %pip magic

Everything else in the suite runs with anywidget installed.
"""

from __future__ import annotations

import importlib
import sys
import warnings
from pathlib import Path

import pytest

import simlin
from simlin import SimlinDependencyError

FIXTURES = Path(__file__).parent / "fixtures"

BLOCKED = ("anywidget", "ipywidgets", "traitlets")


@pytest.fixture
def no_anywidget(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make ``import anywidget`` (and what only it would need) fail, and
    forget the already-imported ``simlin.widget`` so its import re-runs."""
    for name in list(sys.modules):
        if name == "simlin.widget" or name.split(".")[0] in BLOCKED:
            monkeypatch.delitem(sys.modules, name, raising=False)
    # Importing a submodule also binds it on the package; drop that too or
    # `simlin.widget` would still resolve without an import.
    monkeypatch.delattr(simlin, "widget", raising=False)
    for name in BLOCKED:
        monkeypatch.setitem(sys.modules, name, None)
    return


def _model() -> simlin.Model:
    return simlin.load(FIXTURES / "teacup.stmx")


class TestWithoutTheExtra:
    def test_import_simlin_never_needs_anywidget(self, no_anywidget: None) -> None:
        importlib.reload(simlin)
        assert not any(m in sys.modules and sys.modules[m] for m in BLOCKED)

    def test_widget_raises_a_dependency_error_with_the_install_line(
        self, no_anywidget: None
    ) -> None:
        with pytest.raises(SimlinDependencyError, match=r"pysimlin\[notebook\]") as excinfo:
            _model().widget()
        assert isinstance(excinfo.value, ImportError)
        assert isinstance(excinfo.value, simlin.SimlinError)
        assert 'pip install "pysimlin[notebook]"' in str(excinfo.value)

    def test_model_widget_export_raises_the_same(self, no_anywidget: None) -> None:
        with pytest.raises(SimlinDependencyError, match=r"pysimlin\[notebook\]"):
            _ = simlin.ModelWidget

    def test_display_degrades_to_svg_with_one_warning(self, no_anywidget: None) -> None:
        model = _model()
        with pytest.warns(RuntimeWarning, match=r"pysimlin\[notebook\]") as record:
            data, metadata = model._repr_mimebundle_()
        assert len(record) == 1
        assert record[0].filename == __file__  # attributed to the display's cell
        assert set(data) == {"image/svg+xml", "text/plain"}
        assert data["image/svg+xml"].startswith("<svg")
        assert metadata == {}

    def test_display_never_raises_even_when_svg_fails(
        self, no_anywidget: None, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        model = _model()

        def boom(*args: object) -> str:
            raise simlin.SimlinRuntimeError("no diagram")

        monkeypatch.setattr(model, "_svg_mimebundle", boom)
        with warnings.catch_warnings(record=True) as record:
            warnings.simplefilter("always")
            data, _ = model._repr_mimebundle_()
        assert set(data) == {"text/plain"}
        assert len(record) == 2

    def test_install_hint_is_colab_aware(self, monkeypatch: pytest.MonkeyPatch) -> None:
        from simlin._widget_core import install_hint

        monkeypatch.setitem(sys.modules, "google.colab", None)
        assert install_hint() == 'pip install "pysimlin[notebook]"'
        monkeypatch.setitem(sys.modules, "google.colab", type(sys)("google.colab"))
        assert install_hint() == '%pip install "pysimlin[notebook]"'


def test_the_extra_is_installed_here() -> None:
    """The rest of the suite relies on the dev extra carrying anywidget."""
    assert importlib.import_module("anywidget") is not None
