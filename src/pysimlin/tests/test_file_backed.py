"""File-backed projects: ``simlin.open()``, save/save_as/reload, autosave,
revision/dirty tracking, change notification, and the disk watcher.

Test names follow the design's acceptance criteria
(``pysimlin-widget.AC1.n`` / ``AC3.n``) where one applies.

"""

from __future__ import annotations

import gc
import json
import re
import shutil
import threading
import time
import warnings
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

import simlin
from simlin import (
    Aux,
    ChangeEvent,
    Diagram,
    FileFormat,
    Flow,
    Model,
    Project,
    SimlinImportError,
    SimlinRuntimeError,
    Stock,
)
from simlin._disk import _UNKNOWN, content_hash

from .conftest import get_repo_root

if TYPE_CHECKING:
    from collections.abc import Callable

FIXTURES = Path(__file__).parent / "fixtures"
SDAI_FIXTURE = get_repo_root() / "test" / "sd-ai-simple.sd.json"

# A watch interval long enough that the poll thread never fires on its own
# during a test; the tests drive polls deterministically via poll_once().
IDLE = 3600.0


def _copy(src: Path, tmp_path: Path, name: str | None = None) -> Path:
    dst = tmp_path / (name or src.name)
    shutil.copyfile(src, dst)
    return dst


def _add_aux(model: Model, name: str = "new_aux", equation: str = "42") -> None:
    with model.edit() as (_, patch):
        patch.upsert(Aux(name=name, equation=equation))


def _watch_idle(model: Model) -> None:
    """Attach a watcher whose thread stays asleep; tests call poll_once()."""
    project = model.project
    assert project is not None
    project.watch(True, interval=IDLE)


def _poll(model: Model) -> None:
    project = model.project
    assert project is not None
    watcher = project._watcher
    assert watcher is not None
    watcher.poll_once()


class _Events:
    def __init__(self) -> None:
        self.events: list[ChangeEvent] = []
        self.arrived = threading.Event()

    def __call__(self, event: ChangeEvent) -> None:
        self.events.append(event)
        self.arrived.set()

    @property
    def sources(self) -> list[str]:
        return [e.source for e in self.events]


# ── AC1.1: open() ───────────────────────────────────────────────────────


class TestOpen:
    @pytest.mark.parametrize(
        ("fixture", "expected"),
        [
            (FIXTURES / "teacup.stmx", FileFormat.XMILE),
            (FIXTURES / "teacup.xmile", FileFormat.XMILE),
            (FIXTURES / "teacup.mdl", FileFormat.MDL),
            (Path(__file__).parent / "logistic-growth.sd.json", FileFormat.NATIVE_JSON),
            (FIXTURES / "simple.json", FileFormat.NATIVE_JSON),
            (SDAI_FIXTURE, FileFormat.SDAI_JSON),
        ],
    )
    def test_ac1_1_open_sets_path_format_revision(
        self, tmp_path: Path, fixture: Path, expected: FileFormat
    ) -> None:
        path = _copy(fixture, tmp_path)
        model = simlin.open(path, watch=False)
        assert isinstance(model, Model)
        assert model.project is not None
        assert model.project.path == Path(path)
        assert model.path == Path(path)
        assert model.project.format == expected
        assert model.revision == 0
        assert model.dirty is False
        assert model.project.autosave is True
        assert model.project.watching is False
        assert len(model.get_var_names()) > 0

    def test_ac1_1_xml_suffix_is_xmile(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.xmile", tmp_path, "teacup.xml")
        model = simlin.open(path, watch=False)
        assert model.project is not None
        assert model.project.format == FileFormat.XMILE

    def test_ac1_1_unknown_suffix_sniffs_content(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path, "teacup.model")
        model = simlin.open(path, watch=False)
        assert model.project is not None
        assert model.project.format == FileFormat.XMILE

    def test_ac1_1_unknown_suffix_unrecognisable_raises_naming_path(self, tmp_path: Path) -> None:
        path = tmp_path / "mystery.dat"
        path.write_bytes(b"\x00\x01 nothing recognisable")
        with pytest.raises(SimlinImportError, match=r"mystery\.dat"):
            simlin.open(path)

    def test_missing_file_raises_import_error(self, tmp_path: Path) -> None:
        with pytest.raises(SimlinImportError, match="not found"):
            simlin.open(tmp_path / "absent.stmx")

    def test_malformed_known_suffix_is_engine_error(self, tmp_path: Path) -> None:
        # Same class simlin.load raises for a bad file: the engine's parse
        # error, with the format-specific message.
        path = tmp_path / "bad.stmx"
        path.write_bytes(b"not xml at all")
        with pytest.raises(SimlinRuntimeError, match="XMILE"):
            simlin.open(path)

    def test_open_accepts_str_and_watch_flag(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(str(path), watch=True)
        project = model.project
        assert project is not None
        try:
            assert project.watching is True
            assert project.path == path
        finally:
            project.watch(False)
        assert project.watching is False

    def test_load_stays_in_memory(self, tmp_path: Path) -> None:
        # AC1.6: load() keeps its semantics; the project has no path.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.load(path)
        assert model.path is None
        assert model.project is not None
        assert model.project.format is None
        assert model.revision == 0

    def test_load_reads_sdai_json(self) -> None:
        # The table fixes load()'s missing SD-AI case: content-sniffed.
        model = simlin.load(SDAI_FIXTURE)
        assert "population" in {n.lower() for n in model.get_var_names()}

    def test_load_unknown_suffix_unrecognisable_raises(self, tmp_path: Path) -> None:
        path = tmp_path / "blob.dat"
        path.write_bytes(b"\x00garbage")
        with pytest.raises(SimlinImportError, match=r"blob\.dat"):
            simlin.load(path)


# ── AC1.2: edit() autosave / dirty / save() ─────────────────────────────


class TestEditAutosave:
    def test_ac1_2_xmile_edit_rewrites_file_in_xmile(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        before = path.read_bytes()
        model = simlin.open(path, watch=False)
        _add_aux(model)
        assert model.revision == 1
        assert model.dirty is False
        after = path.read_bytes()
        assert after != before
        assert after.lstrip().startswith(b"<?xml")
        assert b"<xmile" in after
        assert b"new_aux" in after
        reopened = simlin.load(path)
        assert reopened.get_variable("new_aux") is not None

    def test_ac1_2_native_json_edit_rewrites_pretty_json(self, tmp_path: Path) -> None:
        path = _copy(Path(__file__).parent / "logistic-growth.sd.json", tmp_path)
        model = simlin.open(path, watch=False)
        _add_aux(model)
        data = path.read_bytes()
        doc = json.loads(data)
        assert "models" in doc
        names = {a["name"] for a in doc["models"][0]["auxiliaries"]}
        assert "new_aux" in names
        # Pretty-printed like simlin-serve / simlin-mcp-core write it.
        assert data.startswith(b"{\n  ")
        assert simlin.load(path).get_variable("new_aux") is not None

    def test_ac1_2_sdai_json_edit_rewrites_sdai(self, tmp_path: Path) -> None:
        path = _copy(SDAI_FIXTURE, tmp_path, "sdai.json")
        model = simlin.open(path, watch=False)
        assert model.project is not None
        assert model.project.format == FileFormat.SDAI_JSON
        _add_aux(model)
        doc = json.loads(path.read_bytes())
        assert "variables" in doc
        assert "models" not in doc
        assert any(v["name"] == "new_aux" for v in doc["variables"])
        assert simlin.load(path).get_variable("new_aux") is not None

    def test_ac1_2_protobuf_round_trip(self, tmp_path: Path) -> None:
        source = simlin.load(FIXTURES / "teacup.stmx").project
        assert source is not None
        path = tmp_path / "teacup.pb"
        source.save_as(path)
        model = simlin.open(path, watch=False)
        assert model.project is not None
        assert model.project.format == FileFormat.PROTOBUF
        _add_aux(model)
        assert simlin.open(path, watch=False).get_variable("new_aux") is not None

    def test_ac1_2_mdl_edit_rewrites_file_in_mdl_with_sketch(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.mdl", tmp_path)
        model = simlin.open(path, watch=False)
        _add_aux(model)
        text = path.read_text()
        # Vensim text uses display names ("new aux"); the sketch section must
        # survive the rewrite and carry an element for the new variable.
        assert re.search(r"^new[ _]aux\s*=", text, re.MULTILINE)
        assert "Sketch information" in text
        assert re.search(r"^10,\d+,new[ _]aux,", text, re.MULTILINE)
        reopened = simlin.open(path, watch=False)
        assert reopened.get_variable("new_aux") is not None
        assert reopened.project is not None
        assert reopened.project.format == FileFormat.MDL

    def test_mdl_lossiness_warnings_fire_once_per_distinct_message(self, tmp_path: Path) -> None:
        # teacup.stmx carries non-negative flags the MDL writer cannot
        # express; a notebook autosaving an .mdl on every edit must not repeat
        # that warning each time.
        model = simlin.load(FIXTURES / "teacup.stmx")
        path = tmp_path / "teacup.mdl"
        with warnings.catch_warnings(record=True) as first:
            warnings.simplefilter("always")
            model.project.save_as(path)  # type: ignore[union-attr]
        messages = [str(w.message) for w in first if w.category is RuntimeWarning]
        assert messages, "fixture must trigger at least one lossiness warning"
        assert len(messages) == len(set(messages))
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            _add_aux(model, "a1")
            _add_aux(model, "a2")
            model.project.to_mdl()  # type: ignore[union-attr]
        # A different project has its own memory (the .mdl on disk has
        # already lost the flags, so use the XMILE source again).
        with pytest.warns(RuntimeWarning, match="Vensim export"):
            simlin.load(FIXTURES / "teacup.stmx").project.to_mdl()  # type: ignore[union-attr]

    def test_ac1_2_revision_increments_by_exactly_one_per_edit(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        for i in range(3):
            _add_aux(model, f"aux_{i}")
            assert model.revision == i + 1

    def test_ac1_2_autosave_false_leaves_file_until_save(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        before = path.read_bytes()
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model)
        assert model.revision == 1
        assert model.dirty is True
        assert path.read_bytes() == before
        model.save()
        assert model.dirty is False
        assert model.revision == 1, "save() writes; it is not a change"
        assert path.read_bytes() != before
        assert simlin.load(path).get_variable("new_aux") is not None

    def test_autosave_is_a_live_switch(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        project = model.project
        assert project is not None
        project.autosave = False
        _add_aux(model, "a")
        assert model.dirty is True
        project.autosave = True
        _add_aux(model, "b")
        assert model.dirty is False
        assert simlin.load(path).get_variable("a") is not None

    def test_rejected_edit_changes_nothing(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        before = path.read_bytes()
        model = simlin.open(path, watch=False)
        with pytest.raises(SimlinRuntimeError), model.edit() as (_, patch):
            patch.upsert(Aux(name="broken", equation="no_such_var * 2"))
        assert model.revision == 0
        assert model.dirty is False
        assert path.read_bytes() == before

    def test_dry_run_edit_changes_nothing(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        before = path.read_bytes()
        model = simlin.open(path, watch=False)
        with model.edit(dry_run=True) as (_, patch):
            patch.upsert(Aux(name="fine", equation="1"))
        assert model.revision == 0
        assert path.read_bytes() == before

    def test_set_sim_specs_is_an_autosaved_change(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        base = model.base_case
        model.project.set_sim_specs(stop=5.0)  # type: ignore[union-attr]
        assert model.revision == 1
        assert model.dirty is False
        assert simlin.load(path).time_spec.stop == 5.0
        # The cached base case must not survive a sim-spec change.
        assert model.base_case is not base
        assert model.base_case.results.index[-1] == 5.0

    def test_auto_layout_is_an_autosaved_change(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        model.project.auto_layout()  # type: ignore[union-attr]
        assert model.revision == 1
        assert model.dirty is False

    def test_save_failure_raises_and_keeps_change_dirty(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        before = path.read_bytes()
        model = simlin.open(path, watch=False)
        events = _Events()
        model.project.on_change(events)  # type: ignore[union-attr]

        def fail(*args: object, **kwargs: object) -> None:
            raise OSError("disk full")

        monkeypatch.setattr("simlin.project.atomic_write", fail)
        with pytest.raises(OSError, match="disk full"):
            _add_aux(model)
        # The in-memory change is real: revision advanced, listeners heard
        # about it, the variable exists, and dirty says the file lags.
        assert model.revision == 1
        assert events.sources == ["edit"]
        assert model.get_variable("new_aux") is not None
        assert model.dirty is True
        assert path.read_bytes() == before
        with pytest.raises(OSError, match="disk full"):
            model.save()
        assert model.dirty is True
        monkeypatch.undo()
        model.save()
        assert model.dirty is False
        assert simlin.load(path).get_variable("new_aux") is not None


# ── AC1.3: incremental layout ───────────────────────────────────────────


def _view_positions(path: Path) -> dict[str, tuple[float, float]]:
    """name -> (x, y) for the named elements of the first model's first view."""
    project = simlin.load(path).project
    assert project is not None
    doc = json.loads(project.serialize_json())
    views = doc["models"][0].get("views") or []
    assert views, "fixture must carry a view"
    return {
        e["name"]: (e["x"], e["y"])
        for e in views[0]["elements"]
        if "name" in e and e["type"] in ("stock", "flow", "aux", "module")
    }


class TestIncrementalLayout:
    def test_ac1_3_new_variable_gets_a_view_element_and_others_keep_positions(
        self, tmp_path: Path
    ) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        before = _view_positions(path)
        assert before, "teacup.stmx has a diagram"
        model = simlin.open(path, watch=False)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="Insulation", equation="0.5"))
        after = _view_positions(path)
        assert "Insulation" in after
        for name, xy in before.items():
            assert after[name] == xy, f"{name} moved during incremental layout"

    def test_ac1_3_model_without_view_gets_a_full_layout(self, tmp_path: Path) -> None:
        project = Project.new(name="scratch")
        path = tmp_path / "scratch.sd.json"
        project.save_as(path)
        model = project.get_model()
        with model.edit() as (_, patch):
            patch.upsert(Stock(name="population", initial_equation="50", inflows=["births"]))
            patch.upsert(Flow(name="births", equation="population * rate"))
            patch.upsert(Aux(name="rate", equation="0.1"))
        positions = _view_positions(path)
        assert set(positions) == {"population", "births", "rate"}

    def test_deleted_variable_loses_its_element(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="Insulation", equation="0.5"))
        assert "Insulation" in _view_positions(path)
        with model.edit() as (_, patch):
            patch.delete_variable("Insulation")
        assert "Insulation" not in _view_positions(path)

    def test_in_memory_edits_do_not_persist_a_layout(self) -> None:
        # Documented contract for in-memory projects (see test_rendering):
        # views stay empty until auto_layout(); only file-backed projects
        # keep their diagram in step automatically.
        project = Project.new(name="scratch")
        model = project.get_model()
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="rate", equation="0.1"))
        doc = json.loads(project.serialize_json())
        assert doc["models"][0].get("views", []) == []

    def test_view_only_edit_does_not_relayout(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        with model.edit() as (_, patch):
            patch.set_loop_name("cooling", ["teacup_temperature", "heat_loss_to_room"])
        assert model.revision == 1
        assert _view_positions(path) == _view_positions(FIXTURES / "teacup.stmx")


# ── AC1.5 / AC1.6: reload idempotence, in-memory projects, save_as ──────


class TestInMemoryAndSaveAs:
    def test_ac1_6_save_without_path_raises_clear_error(self) -> None:
        project = Project.new(name="p")
        with pytest.raises(SimlinRuntimeError, match="save_as"):
            project.save()
        with pytest.raises(SimlinRuntimeError, match="save_as"):
            project.get_model().save()

    def test_ac1_6_reload_without_path_raises(self) -> None:
        project = Project.new(name="p")
        with pytest.raises(SimlinRuntimeError, match="no file path"):
            project.reload()

    def test_ac1_6_watch_without_path_raises(self) -> None:
        project = Project.new(name="p")
        with pytest.raises(SimlinRuntimeError, match="save_as"):
            project.watch(True)

    def test_ac1_6_in_memory_edits_bump_revision_and_are_dirty(self) -> None:
        project = Project.new(name="p")
        model = project.get_model()
        _add_aux(model)
        assert model.revision == 1
        assert model.dirty is True
        assert model.path is None

    @pytest.mark.parametrize(
        ("name", "expected"),
        [
            ("m.stmx", FileFormat.XMILE),
            ("m.xmile", FileFormat.XMILE),
            ("m.sd.json", FileFormat.NATIVE_JSON),
            ("m.json", FileFormat.NATIVE_JSON),
            ("m.pb", FileFormat.PROTOBUF),
        ],
    )
    def test_ac1_6_save_as_adopts_path_and_format(
        self, tmp_path: Path, name: str, expected: FileFormat
    ) -> None:
        model = simlin.load(FIXTURES / "teacup.stmx")
        project = model.project
        assert project is not None
        target = tmp_path / name
        project.save_as(target)
        assert project.path == target
        assert project.format == expected
        assert project.dirty is False
        assert project.revision == 0
        assert target.exists()
        reopened = simlin.open(target, watch=False)
        assert reopened.project is not None
        assert reopened.project.format == expected
        assert set(reopened.get_var_names()) == set(model.get_var_names())
        # And it is now file-backed: the next edit autosaves there.
        _add_aux(model)
        assert simlin.open(target, watch=False).get_variable("new_aux") is not None

    def test_ac1_6_save_as_mdl(self, tmp_path: Path) -> None:
        model = simlin.load(FIXTURES / "teacup.stmx")
        target = tmp_path / "teacup_out.mdl"
        model.project.save_as(target)  # type: ignore[union-attr]
        assert model.project.format == FileFormat.MDL  # type: ignore[union-attr]
        reopened = simlin.open(target, watch=False)
        assert set(reopened.get_var_names()) == set(model.get_var_names())

    def test_m7_save_as_read_only_suffix_raises(self, tmp_path: Path) -> None:
        project = simlin.load(FIXTURES / "teacup.stmx").project
        assert project is not None
        with pytest.raises(SimlinRuntimeError, match="read but not written"):
            project.save_as(tmp_path / "teacup.vpm")
        with pytest.raises(SimlinRuntimeError, match="read but not written"):
            project.save_as(tmp_path / "teacup.proto")
        assert project.path is None

    def test_save_as_explicit_format_overrides_suffix(self, tmp_path: Path) -> None:
        project = simlin.load(FIXTURES / "teacup.stmx").project
        assert project is not None
        target = tmp_path / "teacup.txt"
        project.save_as(target, format=FileFormat.XMILE)
        assert project.format == FileFormat.XMILE
        assert target.read_bytes().lstrip().startswith(b"<?xml")

    def test_save_as_unknown_suffix_without_format_raises(self, tmp_path: Path) -> None:
        project = simlin.load(FIXTURES / "teacup.stmx").project
        assert project is not None
        with pytest.raises(ValueError, match="format="):
            project.save_as(tmp_path / "teacup.txt")
        assert project.path is None

    def test_save_as_moves_the_watcher(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=True)
        project = model.project
        assert project is not None
        try:
            target = tmp_path / "moved.stmx"
            project.save_as(target)
            assert project.watching is True
            assert project._watcher is not None
            assert project._watcher.path == target
        finally:
            project.watch(False)

    def test_ac1_5_reload_is_idempotent_when_unchanged(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        events = _Events()
        model.project.on_change(events)  # type: ignore[union-attr]
        assert model.reload() is False
        _add_aux(model)
        assert model.reload() is False, "our own write is not a change to reload"
        assert model.revision == 1
        assert events.sources == ["edit"]

    def test_close_stops_watching_and_drops_listeners(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        project = Project.open(path)
        events = _Events()
        project.on_change(events)
        assert project.watching is True
        with project:
            pass
        assert project.watching is False
        assert project._listeners == {}


class TestConcurrency:
    def test_concurrent_edits_serialise_and_the_file_stays_valid(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        events = _Events()
        model.project.on_change(events)  # type: ignore[union-attr]
        errors: list[BaseException] = []

        def worker(k: int) -> None:
            # Edits that raced another thread's commit are rejected as stale
            # (their `current` snapshot no longer describes the model); the
            # correct client response is to re-run the edit.
            try:
                for i in range(5):
                    while True:
                        try:
                            _add_aux(model, f"t{k}_a{i}", str(i))
                            break
                        except SimlinRuntimeError as exc:
                            if "changed during edit" not in str(exc):
                                raise
            except BaseException as exc:  # pragma: no cover - reported below
                errors.append(exc)

        threads = [threading.Thread(target=worker, args=(k,)) for k in range(4)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        assert errors == []
        assert model.revision == 20
        assert sorted(e.revision for e in events.events) == list(range(1, 21))
        assert model.dirty is False
        reopened = simlin.load(path)
        names = set(reopened.get_var_names())
        assert {f"t{k}_a{i}" for k in range(4) for i in range(5)} <= names


# ── on_change ───────────────────────────────────────────────────────────


class TestOnChange:
    def test_edit_fires_with_source_and_revision(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        events = _Events()
        unsubscribe = model.project.on_change(events)  # type: ignore[union-attr]
        _add_aux(model, "a")
        _add_aux(model, "b")
        assert events.events == [ChangeEvent("edit", 1), ChangeEvent("edit", 2)]
        unsubscribe()
        _add_aux(model, "c")
        assert len(events.events) == 2
        unsubscribe()  # idempotent

    def test_dispatch_marshals_the_call(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        queued: list[Callable[[], None]] = []
        events = _Events()
        model.project.on_change(events, dispatch=queued.append)  # type: ignore[union-attr]
        _add_aux(model)
        assert events.events == [], "callback must not run until dispatched"
        assert len(queued) == 1
        queued[0]()
        assert events.events == [ChangeEvent("edit", 1)]

    def test_callback_exception_is_a_warning_not_an_edit_failure(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)

        def bad(event: ChangeEvent) -> None:
            raise RuntimeError("listener bug")

        model.project.on_change(bad)  # type: ignore[union-attr]
        with pytest.warns(RuntimeWarning, match="listener bug"):
            _add_aux(model)
        assert model.revision == 1
        assert model.get_variable("new_aux") is not None

    def test_callbacks_run_outside_the_project_locks(self, tmp_path: Path) -> None:
        # A listener that itself edits the project must not deadlock, and
        # nested edits are ordinary changes.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        seen: list[int] = []

        def follow_up(event: ChangeEvent) -> None:
            seen.append(event.revision)
            if event.revision == 1:
                _add_aux(model, "from_listener")

        model.project.on_change(follow_up)  # type: ignore[union-attr]
        _add_aux(model, "first")
        assert seen == [1, 2]
        assert model.revision == 2


# ── watcher wiring (echo suppression, unparsable content, dirty conflicts)


class TestWatcherWiring:
    def test_ac1_5_own_writes_are_echoes(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        events = _Events()
        model.project.on_change(events)  # type: ignore[union-attr]
        try:
            _add_aux(model)
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)
                _poll(model)
            assert events.sources == ["edit"]
            assert model.revision == 1
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_watch_is_idempotent_and_toggleable(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        project = Project.open(path, watch=False)
        assert project.watching is False
        project.watch(True, interval=IDLE)
        first = project._watcher
        project.watch(True, interval=IDLE)
        assert project._watcher is first, "same path and interval reuses the watcher"
        project.watch(True, interval=IDLE / 2)
        assert project._watcher is not first, "a new interval restarts it"
        project.watch(False)
        assert project.watching is False
        project.watch(False)

    def test_external_change_while_dirty_is_held_back_with_one_warning(
        self, tmp_path: Path
    ) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _watch_idle(model)
        try:
            _add_aux(model, "local_change")
            assert model.dirty is True
            external = FIXTURES / "teacup.xmile"
            shutil.copyfile(external, path)
            with pytest.warns(RuntimeWarning, match="unsaved local changes") as record:
                _poll(model)
            assert len(record) == 1
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)  # same bytes again: silent
            assert model.revision == 1
            assert model.get_variable("local_change") is not None
            # save() refuses to clobber the external change; force=True
            # resolves the conflict our way and the file now echoes us.
            with pytest.raises(SimlinRuntimeError, match="changed on disk"):
                model.save()
            model.project.save(force=True)  # type: ignore[union-attr]
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)
            assert content_hash(path.read_bytes()) == model.project._sync.disk_hash  # type: ignore[union-attr]
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_ac1_4_external_write_reloads_in_place(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        events = _Events()
        model.project.on_change(events)  # type: ignore[union-attr]
        try:
            base = model.base_case
            assert model.get_variable("Insulation") is None
            # An external writer: a second, independent project edits the file.
            other = simlin.open(path, watch=False)
            with other.edit() as (_, patch):
                patch.upsert(Aux(name="Insulation", equation="0.5"))
            _poll(model)
            assert model.revision == 1
            assert events.events == [ChangeEvent("disk", 1)]
            assert model.get_variable("Insulation") is not None, (
                "the pre-existing Model handle must see the reloaded contents"
            )
            assert model.base_case is not base
            assert "insulation" in model.run().results.columns
            assert model.dirty is False
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_ac1_4_reload_invalidates_every_model_handle(self, tmp_path: Path) -> None:
        # Several handles to the same model (get_model() called more than
        # once) must all drop their cached base_case, not just the one that
        # happens to trigger the reload.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        project = Project.open(path, watch=False)
        handles = [project.get_model() for _ in range(3)]
        runs = [h.base_case for h in handles]
        writer = simlin.open(path, watch=False)
        writer.project.set_sim_specs(stop=writer.time_spec.stop / 2)  # type: ignore[union-attr]
        assert handles[0].reload() is True
        for handle, before in zip(handles, runs, strict=True):
            assert handle.base_case is not before
            assert handle.base_case.results.index[-1] == writer.time_spec.stop

    def test_ac1_4_unparsable_content_keeps_last_known_good(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        try:
            names = set(model.get_var_names())
            path.write_bytes(b"<xmile>this is not a model")
            with pytest.warns(RuntimeWarning, match="could not be loaded") as record:
                _poll(model)
            assert len(record) == 1
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)  # same bad bytes: no second warning
            assert model.revision == 0
            assert set(model.get_var_names()) == names
            # A subsequent valid write is picked up.
            shutil.copyfile(FIXTURES / "teacup.xmile", path)
            _poll(model)
            assert model.revision == 1
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_ac1_4_reload_reads_external_change_and_is_then_idempotent(
        self, tmp_path: Path
    ) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        other = simlin.open(path, watch=False)
        _add_aux(other, "from_other")
        assert model.get_variable("from_other") is None
        assert model.reload() is True
        assert model.revision == 1
        assert model.get_variable("from_other") is not None
        assert model.reload() is False
        assert model.revision == 1

    def test_reload_over_dirty_discards_local_changes(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model, "local_change")
        assert model.reload() is True
        assert model.get_variable("local_change") is None
        assert model.dirty is False
        assert model.revision == 2

    def test_reload_unparsable_raises_and_keeps_project(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        names = set(model.get_var_names())
        path.write_bytes(b"<xmile>nope")
        with pytest.raises(SimlinRuntimeError):
            model.reload()
        assert set(model.get_var_names()) == names
        assert model.revision == 0

    def test_ac1_4_poll_thread_delivers_within_interval(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        project = model.project
        assert project is not None
        project.watch(True, interval=0.05)
        events = _Events()
        project.on_change(events)
        try:
            other = simlin.open(path, watch=False)
            _add_aux(other, "external")
            assert events.arrived.wait(5.0), "poll thread never reported the change"
            assert events.events == [ChangeEvent("disk", 1)]
            assert model.get_variable("external") is not None
        finally:
            project.watch(False)

    def test_widget_snapshot_accept_and_reject(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        project = model.project
        assert project is not None
        events = _Events()
        project.on_change(events)
        # Build a snapshot the way a widget would: the whole project as
        # native JSON, edited by adding a variable through the engine.
        scratch = simlin.load(path)
        _add_aux(scratch, "from_widget")
        snapshot = scratch.project.serialize_json()  # type: ignore[union-attr]
        assert project._apply_snapshot(snapshot, base_revision=0) is True
        assert model.revision == 1
        assert events.events == [ChangeEvent("widget", 1)]
        assert model.get_variable("from_widget") is not None
        assert simlin.load(path).get_variable("from_widget") is not None
        # A stale snapshot (base 0, kernel is at 1) is rejected and not written.
        before = path.read_bytes()
        assert project._apply_snapshot(snapshot, base_revision=0) is False
        assert model.revision == 1
        assert path.read_bytes() == before


# ── review findings: lifecycle, staleness, conflicts ────────────────────


class TestWatcherLifecycle:
    def test_i1_dropping_the_project_stops_the_thread_without_a_file_change(
        self, tmp_path: Path
    ) -> None:
        # The thread must exit because the project is gone, not because a
        # tick happened to run the weak-ref handler: let the first tick
        # complete (so the handler path has already fired and returned True)
        # and only then drop the project with the file idle.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        baseline = {t.name for t in threading.enumerate()}
        model = simlin.open(path, watch=False)
        model.project.watch(True, interval=0.02)  # type: ignore[union-attr]
        watcher = model.project._watcher  # type: ignore[union-attr]
        assert watcher is not None
        thread = watcher._thread
        assert thread is not None
        deadline = time.monotonic() + 5.0
        while watcher._last_signature is _UNKNOWN and time.monotonic() < deadline:
            time.sleep(0.01)
        assert watcher._last_signature is not _UNKNOWN, "first tick never ran"
        assert thread.is_alive()
        del model
        gc.collect()
        thread.join(timeout=5.0)
        assert not thread.is_alive(), "poll thread outlived its project"
        assert {t.name for t in threading.enumerate()} - baseline == set()

    def test_i2_watch_started_after_an_external_change_picks_it_up(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        other = simlin.open(path, watch=False)
        _add_aux(other, "external")
        _watch_idle(model)
        try:
            _poll(model)
            assert model.get_variable("external") is not None
            assert model.revision == 1
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_i2_watch_after_own_write_is_still_an_echo(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _add_aux(model)
        _watch_idle(model)
        try:
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)
            assert model.revision == 1
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_i5_stale_watcher_delivery_after_our_own_write_is_ignored(self, tmp_path: Path) -> None:
        # The watcher read bytes X (an external edit) but blocked on the file
        # lock while we wrote W (here: save(force=True), the one write that
        # may legitimately overwrite an external change); when the handler
        # finally runs, X is stale and must not be loaded over W.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _watch_idle(model)
        project = model.project
        assert project is not None
        watcher = project._watcher
        assert watcher is not None
        try:
            ext = simlin.open(path, watch=False)
            _add_aux(ext, "from_ext")
            stale = path.read_bytes()
            _add_aux(model, "mine")
            project.save(force=True)
            watcher._handler(watcher, stale, content_hash(stale))
            assert model.get_variable("mine") is not None
            assert model.get_variable("from_ext") is None
            assert model.revision == 1
            assert model.dirty is False
            # The next real tick sees our own bytes: an echo, nothing happens.
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)
            assert model.revision == 1
        finally:
            project.watch(False)

    def test_i5_delivery_from_a_watcher_retired_by_watch_false_is_ignored(
        self, tmp_path: Path
    ) -> None:
        # Same path, so only the identity check (watcher is self._watcher)
        # can reject it: an in-flight delivery from a stopped watcher must
        # not load bytes the user asked us to stop watching for.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        project = model.project
        assert project is not None
        retired = project._watcher
        assert retired is not None
        project.watch(False)
        _add_aux(simlin.open(path, watch=False), "after_unwatch")
        data = path.read_bytes()
        retired._handler(retired, data, content_hash(data))
        assert model.get_variable("after_unwatch") is None
        assert model.revision == 0
        # The change is still there for an explicit reload().
        assert model.reload() is True
        assert model.get_variable("after_unwatch") is not None

    def test_i5_delivery_from_a_retired_watcher_is_ignored(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        project = model.project
        assert project is not None
        old = project._watcher
        assert old is not None
        try:
            # save_as moves the watcher; the old one's path is no longer ours.
            project.save_as(tmp_path / "moved.stmx")
            ext = simlin.open(path, watch=False)
            _add_aux(ext, "on_old_path")
            data = path.read_bytes()
            old._handler(old, data, content_hash(data))
            assert model.get_variable("on_old_path") is None
            assert model.revision == 0
        finally:
            project.watch(False)


class TestSaveConflicts:
    def test_model_save_passes_force_through(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model, "local")
        _add_aux(simlin.open(path, watch=False), "claude_wrote_this")
        with pytest.raises(SimlinRuntimeError, match="changed on disk"):
            model.save()
        model.save(force=True)
        assert model.dirty is False
        assert simlin.load(path).get_variable("local") is not None

    def test_i3_save_over_an_external_change_raises_unless_forced(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model, "local")
        ext = simlin.open(path, watch=False)
        _add_aux(ext, "claude_wrote_this")
        with pytest.raises(SimlinRuntimeError, match="changed on disk") as info:
            model.save()
        assert "reload()" in str(info.value)
        assert "force=True" in str(info.value)
        assert model.dirty is True
        assert simlin.load(path).get_variable("claude_wrote_this") is not None
        model.project.save(force=True)  # type: ignore[union-attr]
        assert model.dirty is False
        on_disk = simlin.load(path)
        assert on_disk.get_variable("local") is not None
        assert on_disk.get_variable("claude_wrote_this") is None

    def test_i3_save_after_holdback_warning_also_raises(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _watch_idle(model)
        try:
            _add_aux(model, "local")
            ext = simlin.open(path, watch=False)
            _add_aux(ext, "claude_wrote_this")
            with pytest.warns(RuntimeWarning, match="unsaved local changes"):
                _poll(model)
            with pytest.raises(SimlinRuntimeError, match="changed on disk"):
                model.save()
            # reload() takes the on-disk version and clears the conflict.
            assert model.reload() is True
            assert model.get_variable("claude_wrote_this") is not None
            assert model.get_variable("local") is None
            model.save()  # nothing to write, and no conflict
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_i3_save_as_to_a_different_path_is_unaffected(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model, "local")
        _add_aux(simlin.open(path, watch=False), "claude_wrote_this")
        model.project.save_as(tmp_path / "elsewhere.stmx")  # type: ignore[union-attr]
        assert model.dirty is False
        assert simlin.load(path).get_variable("claude_wrote_this") is not None

    def test_i3_save_as_to_the_same_path_conflicts_like_save(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model, "local")
        _add_aux(simlin.open(path, watch=False), "claude_wrote_this")
        with pytest.raises(SimlinRuntimeError, match="changed on disk"):
            model.project.save_as(path)  # type: ignore[union-attr]

    def test_i3_autosave_conflict_raises_from_edit_and_keeps_change(self, tmp_path: Path) -> None:
        # With autosave on, an external write that slipped in unnoticed
        # (no watcher) is detected by the autosave itself.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _add_aux(simlin.open(path, watch=False), "claude_wrote_this")
        with pytest.raises(SimlinRuntimeError, match="changed on disk"):
            _add_aux(model, "local")
        assert model.revision == 1
        assert model.dirty is True
        assert model.get_variable("local") is not None
        assert simlin.load(path).get_variable("claude_wrote_this") is not None

    def test_save_recreates_a_deleted_file_without_a_conflict(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model, "local")
        path.unlink()
        model.save()  # a missing file is not someone else's change
        assert path.exists()
        assert model.dirty is False
        assert simlin.load(path).get_variable("local") is not None

    def test_m2_failed_save_of_a_clean_project_stays_clean(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)

        def fail(*args: object, **kwargs: object) -> None:
            raise OSError("disk full")

        monkeypatch.setattr("simlin.project.atomic_write", fail)
        with pytest.raises(OSError, match="disk full"):
            model.save()
        assert model.dirty is False


class TestEditStaleness:
    def test_i4_edit_spanning_a_reload_is_rejected(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        try:
            ext = simlin.open(path, watch=False)

            def stale_edit() -> None:
                with model.edit() as (current, patch):
                    with ext.edit() as (ext_current, ext_patch):
                        room = ext_current["room temperature"]
                        ext_patch.upsert(replace(room, equation="99"))
                        ext_patch.upsert(Aux(name="claude", equation="1"))
                    _poll(model)  # the reload lands mid-edit
                    patch.upsert(replace(current["room temperature"], documentation="doc"))

            with pytest.raises(SimlinRuntimeError, match=r"changed during edit.*0 -> 1"):
                stale_edit()
            # Nothing from the stale edit was applied; the reload stands.
            assert model.revision == 1
            room_after = model.get_variable("room_temperature")
            assert isinstance(room_after, Aux)
            assert room_after.equation == "99"
            assert room_after.documentation != "doc"
            assert model.get_variable("claude") is not None
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    def test_i4_edit_spanning_another_edit_is_rejected(self, tmp_path: Path) -> None:
        model = simlin.load(FIXTURES / "teacup.stmx")

        def nested_edit() -> None:
            with model.edit() as (_, patch):
                _add_aux(model, "inner")
                patch.upsert(Aux(name="outer", equation="1"))

        with pytest.raises(SimlinRuntimeError, match="changed during edit"):
            nested_edit()
        assert model.get_variable("inner") is not None
        assert model.get_variable("outer") is None
        assert model.revision == 1

    def test_i4_empty_edit_spanning_a_change_is_fine(self, tmp_path: Path) -> None:
        # No ops means nothing to apply; a no-op block must not raise.
        model = simlin.load(FIXTURES / "teacup.stmx")
        with model.edit() as (_, _patch):
            _add_aux(model, "inner")
        assert model.revision == 1


class TestBaseCaseCache:
    def test_m5_base_case_computed_across_a_change_is_not_cached(self, tmp_path: Path) -> None:
        model = simlin.load(FIXTURES / "teacup.stmx")
        original_run = model.run

        def run_and_edit(*args: object, **kwargs: object):  # type: ignore[no-untyped-def]
            result = original_run(*args, **kwargs)
            _add_aux(model, "during_run")
            return result

        model.run = run_and_edit  # type: ignore[method-assign]
        stale = model.base_case
        model.run = original_run  # type: ignore[method-assign]
        assert model.base_case is not stale
        assert "during_run" in model.base_case.results.columns

    def test_edit_invalidates_base_case(self, tmp_path: Path) -> None:
        model = simlin.load(FIXTURES / "teacup.stmx")
        before = model.base_case
        _add_aux(model)
        assert model.base_case is not before
        assert "new_aux" in model.base_case.results.columns


class TestClosedProject:
    def test_m6_close_clears_path_and_refuses_watch(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        project = Project.open(path, watch=False)
        project.close()
        assert project.path is None
        assert project.watching is False
        with pytest.raises(SimlinRuntimeError, match="closed"):
            project.watch(True)
        with pytest.raises(SimlinRuntimeError):
            project.save()


class TestSnapshotEdgeCases:
    def test_m9_identical_snapshot_bumps_revision_exactly_once(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        project = Project.open(path, watch=False)
        events = _Events()
        project.on_change(events)
        snapshot = project.serialize_json()
        assert project._apply_snapshot(snapshot, base_revision=0) is True
        assert project.revision == 1
        assert events.events == [ChangeEvent("widget", 1)]

    def test_snapshot_pair_is_read_under_one_lock(self, tmp_path: Path) -> None:
        # The widget seeds and re-seeds the browser from _snapshot(); the
        # contents and the revision must belong together even while another
        # thread edits.  Hammer it: every pair observed must be one the
        # project actually passed through (revision r <=> r auxes added).
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        project = model.project
        assert project is not None
        data, revision = project._snapshot()
        assert revision == 0
        assert data == project.serialize_json()
        stop = threading.Event()
        pairs: list[tuple[int, int]] = []

        def read() -> None:
            while not stop.is_set():
                d, r = project._snapshot()  # type: ignore[union-attr]
                auxes = json.loads(d)["models"][0].get("auxiliaries") or []
                added = sum(1 for v in auxes if v["name"].startswith("aux_"))
                pairs.append((r, added))

        reader = threading.Thread(target=read)
        reader.start()
        try:
            for i in range(20):
                _add_aux(model, f"aux_{i}")
        finally:
            stop.set()
            reader.join()
        assert pairs
        assert all(r == added for r, added in pairs), pairs

    def test_m9_unparsable_snapshot_leaves_revision_unchanged(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        before = path.read_bytes()
        project = Project.open(path, watch=False)
        with pytest.raises(SimlinRuntimeError):
            project._apply_snapshot(b'{"models": [{"name": 5}]}', base_revision=0)
        assert project.revision == 0
        assert project.dirty is False
        assert path.read_bytes() == before

    def test_m9_format_follows_a_reload_that_flips_native_and_sdai(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "simple.json", tmp_path)
        project = Project.open(path, watch=False)
        assert project.format == FileFormat.NATIVE_JSON
        shutil.copyfile(SDAI_FIXTURE, path)
        assert project.reload() is True
        assert project.format == FileFormat.SDAI_JSON
        _add_aux(project.get_model())
        assert "variables" in json.loads(path.read_bytes())
        shutil.copyfile(FIXTURES / "simple.json", path)
        assert project.reload() is True
        assert project.format == FileFormat.NATIVE_JSON

    def test_m3_unknown_format_bad_bytes_warn_once(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "simple.json", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        try:
            path.write_text('{"neither": 1}')
            with pytest.warns(RuntimeWarning, match="could not be loaded") as record:
                _poll(model)
            assert len(record) == 1
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)
                # Even a re-touch of the same bad content stays quiet.
                path.write_text('{"neither": 1}')
                _poll(model)
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    @pytest.mark.parametrize("arm", ["edit", "auto_layout", "widget", "disk", "reload"])
    def test_m9_notify_runs_with_no_project_lock_held(self, tmp_path: Path, arm: str) -> None:
        # Every _notify call site: a listener that touches the project (or
        # is marshalled onto another thread that does) must not deadlock.
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        _watch_idle(model)
        project = model.project
        assert project is not None
        results: list[tuple[str, bool]] = []

        def probe(event: ChangeEvent) -> None:
            def try_locks() -> None:
                got_file = project._file_lock.acquire(blocking=False)
                if got_file:
                    project._file_lock.release()
                got_ptr = project._lock.acquire(blocking=False)
                if got_ptr:
                    project._lock.release()
                results.append((event.source, got_file and got_ptr))

            t = threading.Thread(target=try_locks)
            t.start()
            t.join()

        project.on_change(probe)
        try:
            match arm:
                case "edit":
                    _add_aux(model)
                    expected = "edit"
                case "auto_layout":
                    project.auto_layout("main")
                    expected = "edit"
                case "widget":
                    project._apply_snapshot(project.serialize_json(), base_revision=0)
                    expected = "widget"
                case "disk":
                    _add_aux(simlin.open(path, watch=False), "ext")
                    _poll(model)
                    expected = "disk"
                case "reload":
                    _add_aux(simlin.open(path, watch=False), "ext")
                    assert project.reload() is True
                    expected = "reload"
                case _:
                    raise AssertionError(arm)
        finally:
            project.watch(False)
        assert results == [(expected, True)]


class TestOpenOnDirectory:
    def test_m10_open_and_load_on_a_directory_raise_import_error(self, tmp_path: Path) -> None:
        with pytest.raises(SimlinImportError, match="not a file"):
            simlin.open(tmp_path)
        with pytest.raises(SimlinImportError, match="not a file"):
            simlin.load(tmp_path)


# ── AC3: read-only display ──────────────────────────────────────────────


class TestDiagram:
    def test_ac3_1_diagram_repr_svg_is_engine_svg(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        diagram = model.diagram()
        assert isinstance(diagram, Diagram)
        svg = diagram._repr_svg_()
        assert svg.lstrip().startswith("<svg") or "<svg" in svg[:200]
        assert svg == model.project.render_svg_string()  # type: ignore[union-attr]
        assert diagram._repr_mimebundle_() == {"image/svg+xml": svg}
        assert model._svg_mimebundle() == {"image/svg+xml": svg}

    def test_ac3_1_model_without_view_renders_transient_layout(self) -> None:
        project = Project.new(name="scratch")
        model = project.get_model()
        _add_aux(model, "lonely", "1")
        svg = model.diagram()._repr_svg_()
        assert "<svg" in svg
        assert "lonely" in svg
        doc = json.loads(project.serialize_json())
        assert doc["models"][0].get("views", []) == [], "diagram() persists nothing"

    def test_diagram_reflects_edits(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        assert "Insulation" not in model.diagram().svg
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="Insulation", equation="0.5"))
        assert "Insulation" in model.diagram().svg

    def test_selection_defaults_empty_and_is_settable(self, tmp_path: Path) -> None:
        model = simlin.load(FIXTURES / "teacup.stmx")
        assert model.selection == ()
        model.selection = ["teacup_temperature"]
        assert model.selection == ("teacup_temperature",)


class TestModelProxies:
    def test_unattached_model_proxies_raise(self, tmp_path: Path) -> None:
        # Model(ptr) without a project is a legacy construction path; the
        # proxies must fail loudly rather than dereference None.
        model = simlin.load(FIXTURES / "teacup.stmx")
        model._project = None
        with pytest.raises(SimlinRuntimeError, match="not attached"):
            _ = model.path
        with pytest.raises(SimlinRuntimeError, match="not attached"):
            model.diagram()

    def test_edit_replace_round_trip_through_open(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        room = model.get_variable("room_temperature")
        assert isinstance(room, Aux)
        with model.edit() as (current, patch):
            patch.upsert(replace(current[room.name], equation="60"))
        reopened = simlin.open(path, watch=False)
        aux = reopened.get_variable("room_temperature")
        assert isinstance(aux, Aux)
        assert aux.equation == "60"
