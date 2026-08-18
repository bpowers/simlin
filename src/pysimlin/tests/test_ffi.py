"""Tests for the ``simlin._ffi`` wrappers behind file-backed models.

These exercise the low-level bindings directly (``serialize_mdl``,
``replace_contents``, and the patch-aware ``diagram_sync``); the file-backed
``Project`` API built on top of them has its own tests.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING

import pytest

import simlin
from simlin import Aux, Compat, Flow, Stock, _ffi
from simlin.errors import ErrorCode, ErrorSeverity, SimlinRuntimeError
from simlin.json_converter import converter
from simlin.json_types import JsonProjectPatch
from simlin.json_types import Model as JsonModel
from simlin.json_types import Project as JsonProject
from simlin.json_types import SimSpecs as JsonSimSpecs
from simlin.model import ModelPatchBuilder

if TYPE_CHECKING:
    from pathlib import Path


def build_population_model() -> simlin.Model:
    """A three-variable model built from scratch (so it has no diagram view)."""
    project = simlin.Project.new(name="population", sim_start=0, sim_stop=10, dt=1)
    model = project.get_model()
    with model.edit() as (_, patch):
        patch.upsert(Stock(name="population", initial_equation="50", inflows=["net_growth"]))
        patch.upsert(Flow(name="net_growth", equation="population * rate"))
        patch.upsert(Aux(name="rate", equation="0.08"))
    return model


def project_with_only_model(name: str) -> simlin.Project:
    """A project whose single model is called ``name`` (not ``main``)."""
    project = JsonProject(
        name="renamed",
        sim_specs=JsonSimSpecs(start_time=0.0, end_time=10.0, dt="1", method="euler"),
        models=[JsonModel(name=name)],
    )
    data = json.dumps(converter.unstructure(project)).encode("utf-8")
    return simlin.Project(_ffi.open_json(data))


def upsert_patch_json(model_name: str, *variables: Stock | Flow | Aux) -> bytes:
    """The JSON project patch ``Model.edit()`` would apply for these upserts."""
    builder = ModelPatchBuilder(model_name)
    for var in variables:
        builder.upsert(var)
    return json.dumps(converter.unstructure(JsonProjectPatch(models=[builder.build()]))).encode(
        "utf-8"
    )


def view_positions(project: simlin.Project, model_name: str = "main") -> dict[str, tuple]:
    """Named diagram element -> (x, y) from the model's first persisted view.

    View elements carry display names (spaces/newlines for word wrapping);
    keys are folded back to the variable ident.
    """
    project_dict = json.loads(project.serialize_json())
    (model_dict,) = [m for m in project_dict["models"] if m["name"] == model_name]
    views = model_dict.get("views", [])
    if not views:
        return {}
    return {
        el["name"].replace("\n", "_").replace(" ", "_"): (el["x"], el["y"])
        for el in views[0]["elements"]
        if "name" in el and "x" in el
    }


class TestSerializeMdl:
    def test_roundtrips_a_vensim_model(self, mdl_model_path: Path, tmp_path: Path) -> None:
        model = simlin.load(mdl_model_path)
        text, warnings = _ffi.serialize_mdl(model.project._ptr)

        assert warnings == []
        assert text.startswith(b"{UTF-8}")
        assert b"Sketch information" in text

        written = tmp_path / "roundtrip.mdl"
        written.write_bytes(text)
        reloaded = simlin.load(written)
        assert set(reloaded.get_var_names()) == set(model.get_var_names())

    def test_returns_lossiness_warnings_without_failing(self) -> None:
        project = simlin.Project.new(name="lossy", sim_start=0, sim_stop=10, dt=1)
        model = project.get_model()
        with model.edit() as (_, patch):
            patch.upsert(
                Stock(
                    name="reservoir",
                    initial_equation="100",
                    inflows=["inflow"],
                    compat=Compat(non_negative=True),
                )
            )
            patch.upsert(Flow(name="inflow", equation="1"))

        text, warnings = _ffi.serialize_mdl(project._ptr)

        assert b"reservoir" in text, "the export must still emit the degraded variable"
        assert len(warnings) == 1
        (warning,) = warnings
        assert warning.severity == ErrorSeverity.WARNING
        assert warning.code == ErrorCode.GENERIC
        assert "reservoir" in warning.message
        assert "non-negative" in warning.message
        assert warning.details is not None
        assert "reservoir" in warning.details

    def test_hard_error_raises(self) -> None:
        project = simlin.Project.new(name="two_models")
        err_ptr = _ffi.ffi.new("SimlinError **")
        _ffi.lib.simlin_project_add_model(project._ptr, _ffi.string_to_c("second"), err_ptr)
        _ffi.check_out_error(err_ptr, "add second model")

        with pytest.raises(SimlinRuntimeError, match="single model"):
            _ffi.serialize_mdl(project._ptr)

    def test_null_project_raises(self) -> None:
        with pytest.raises(SimlinRuntimeError):
            _ffi.serialize_mdl(_ffi.ffi.NULL)


class TestReplaceContents:
    def test_existing_model_object_observes_new_contents(self, mdl_model_path: Path) -> None:
        model = simlin.load(mdl_model_path)
        project = model.project
        assert project is not None
        original_names = set(model.get_var_names())
        assert "population" not in original_names

        src_model = build_population_model()
        src_project = src_model.project
        assert src_project is not None

        _ffi.replace_contents(project._ptr, src_project._ptr)

        # The Model object we already held is the same object, still attached
        # to the same Project, and now reads the replacement's variables ...
        assert model.project is project
        assert set(model.get_var_names()) == {"population", "net_growth", "rate"}
        # ... and simulates them.
        run = model.run(analyze_loops=False)
        assert "population" in run.results.columns
        assert run.results["population"].iloc[-1] > 50

        # The source is untouched, and closing it does not affect the copy.
        assert set(src_model.get_var_names()) == {"population", "net_growth", "rate"}
        with src_project:
            pass
        assert set(model.get_var_names()) == {"population", "net_growth", "rate"}

    def test_get_errors_reflects_new_contents(self) -> None:
        clean = build_population_model()
        clean_project = clean.project
        assert clean_project is not None
        assert clean_project.get_errors() == []

        broken = simlin.Project.new(name="broken").get_model()
        with broken.edit(allow_errors=True) as (_, patch):
            patch.upsert(Aux(name="a", equation="b + 1"))
            patch.upsert(Aux(name="b", equation="a + 1"))
        broken_project = broken.project
        assert broken_project is not None

        _ffi.replace_contents(clean_project._ptr, broken_project._ptr)
        codes = {e.code for e in clean_project.get_errors()}
        assert ErrorCode.CIRCULAR_DEPENDENCY in codes

    def test_model_absent_from_replacement_errors_cleanly_and_revives(self) -> None:
        model = build_population_model()
        project = model.project
        assert project is not None
        renamed = project_with_only_model("other")
        original = build_population_model().project
        assert original is not None

        _ffi.replace_contents(project._ptr, renamed._ptr)

        with pytest.raises(SimlinRuntimeError) as excinfo:
            model.get_var_names()
        assert excinfo.value.code == ErrorCode.BAD_MODEL_NAME

        # The replacement's own model is reachable, and restoring a project
        # that has "main" again makes the old Model object work once more.
        assert project.get_model_names() == ["other"]
        _ffi.replace_contents(project._ptr, original._ptr)
        assert set(model.get_var_names()) == {"population", "net_growth", "rate"}

    def test_sim_created_before_replace_is_a_stale_snapshot(self) -> None:
        model = build_population_model()
        project = model.project
        assert project is not None
        sim = model.simulate()
        sim.run_to_end()
        before = sim.get_series("population")

        src = simlin.Project.new(name="longer", sim_start=0, sim_stop=20, dt=1)
        with src.get_model().edit() as (_, patch):
            patch.upsert(Stock(name="population", initial_equation="50", inflows=["net_growth"]))
            patch.upsert(Flow(name="net_growth", equation="population * rate"))
            patch.upsert(Aux(name="rate", equation="0.08"))
        _ffi.replace_contents(project._ptr, src._ptr)

        # The old sim keeps its results and reruns the program it compiled.
        assert list(sim.get_series("population")) == list(before)
        sim.reset()
        sim.run_to_end()
        assert len(sim.get_series("population")) == len(before)
        # A new sim through the same Model picks up the replacement.
        fresh = model.simulate()
        fresh.run_to_end()
        assert len(fresh.get_series("population")) == 21

    def test_null_pointers_raise(self) -> None:
        project = build_population_model().project
        assert project is not None
        with pytest.raises(SimlinRuntimeError):
            _ffi.replace_contents(_ffi.ffi.NULL, project._ptr)
        with pytest.raises(SimlinRuntimeError):
            _ffi.replace_contents(project._ptr, _ffi.ffi.NULL)


class TestDiagramSync:
    def test_incremental_layout_places_new_element_and_keeps_existing_positions(self) -> None:
        model = build_population_model()
        project = model.project
        assert project is not None
        _ffi.diagram_sync(project._ptr, "main")
        before = view_positions(project)
        assert set(before) == {"population", "net_growth", "rate"}

        patch_json = upsert_patch_json("main", Aux(name="carrying_capacity", equation="1000"))
        _ffi.apply_patch_json(project._ptr, patch_json, dry_run=False, allow_errors=False)
        _ffi.diagram_sync(project._ptr, "main", patch_json)

        after = view_positions(project)
        assert "carrying_capacity" in after
        for name, position in before.items():
            assert after[name] == position, f"{name} must keep its position"

    def test_full_relayout_when_patch_is_none(self) -> None:
        model = build_population_model()
        project = model.project
        assert project is not None
        _ffi.diagram_sync(project._ptr, "main")
        _ffi.apply_patch_json(
            project._ptr,
            upsert_patch_json("main", Aux(name="carrying_capacity", equation="1000")),
            dry_run=False,
            allow_errors=False,
        )

        _ffi.diagram_sync(project._ptr, "main", None)

        assert set(view_positions(project)) == {
            "population",
            "net_growth",
            "rate",
            "carrying_capacity",
        }

    def test_patch_without_ops_for_model_leaves_view_untouched(self) -> None:
        model = build_population_model()
        project = model.project
        assert project is not None
        _ffi.diagram_sync(project._ptr, "main")
        before = view_positions(project)

        _ffi.diagram_sync(project._ptr, "main", upsert_patch_json("some_other_model"))

        assert view_positions(project) == before

    def test_unparsable_patch_raises(self) -> None:
        project = build_population_model().project
        assert project is not None
        with pytest.raises(SimlinRuntimeError, match="patch_json"):
            _ffi.diagram_sync(project._ptr, "main", b"not json")
