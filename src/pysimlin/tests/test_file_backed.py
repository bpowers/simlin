"""File-backed projects: ``simlin.open()``, save/save_as/reload, autosave,
revision/dirty tracking, change notification, and the disk watcher.

Test names follow the design's acceptance criteria
(``pysimlin-widget.AC1.n`` / ``AC3.n``) where one applies.

Tests that need the engine's in-place reload (``_ffi.replace_contents``) or
Vensim writer (``_ffi.serialize_mdl``) are gated on those functions being
present; they run unchanged once libsimlin provides them.
"""

from __future__ import annotations

import json
import shutil
import threading
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
from simlin import _ffi as simlin_ffi
from simlin._disk import content_hash

from .conftest import get_repo_root

if TYPE_CHECKING:
    from collections.abc import Callable

HAS_REPLACE_CONTENTS = hasattr(simlin_ffi, "replace_contents")
HAS_SERIALIZE_MDL = hasattr(simlin_ffi, "serialize_mdl")
needs_replace_contents = pytest.mark.skipif(
    not HAS_REPLACE_CONTENTS,
    reason="waiting for libsimlin simlin_project_replace_from_json / _ffi.replace_contents",
)
needs_serialize_mdl = pytest.mark.skipif(
    not HAS_SERIALIZE_MDL,
    reason="waiting for libsimlin simlin_project_serialize_mdl / _ffi.serialize_mdl",
)

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

    @needs_serialize_mdl
    def test_ac1_2_mdl_edit_rewrites_file_in_mdl_with_sketch(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.mdl", tmp_path)
        model = simlin.open(path, watch=False)
        _add_aux(model)
        text = path.read_text()
        assert "new_aux" in text
        assert "Sketch information" in text
        reopened = simlin.open(path, watch=False)
        assert reopened.get_variable("new_aux") is not None
        assert reopened.project is not None
        assert reopened.project.format == FileFormat.MDL

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

    @needs_serialize_mdl
    def test_ac1_6_save_as_mdl(self, tmp_path: Path) -> None:
        model = simlin.load(FIXTURES / "teacup.stmx")
        target = tmp_path / "teacup_out.mdl"
        model.project.save_as(target)  # type: ignore[union-attr]
        assert model.project.format == FileFormat.MDL  # type: ignore[union-attr]
        reopened = simlin.open(target, watch=False)
        assert set(reopened.get_var_names()) == set(model.get_var_names())

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
            try:
                for i in range(5):
                    _add_aux(model, f"t{k}_a{i}", str(i))
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
            # save() resolves the conflict our way and the file now echoes us.
            model.save()
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                _poll(model)
            assert content_hash(path.read_bytes()) == model.project._sync.disk_hash  # type: ignore[union-attr]
        finally:
            model.project.watch(False)  # type: ignore[union-attr]

    @needs_replace_contents
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

    @needs_replace_contents
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

    @needs_replace_contents
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

    @needs_replace_contents
    def test_reload_over_dirty_discards_local_changes(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, autosave=False, watch=False)
        _add_aux(model, "local_change")
        assert model.reload() is True
        assert model.get_variable("local_change") is None
        assert model.dirty is False
        assert model.revision == 2

    @needs_replace_contents
    def test_reload_unparsable_raises_and_keeps_project(self, tmp_path: Path) -> None:
        path = _copy(FIXTURES / "teacup.stmx", tmp_path)
        model = simlin.open(path, watch=False)
        names = set(model.get_var_names())
        path.write_bytes(b"<xmile>nope")
        with pytest.raises(SimlinRuntimeError):
            model.reload()
        assert set(model.get_var_names()) == names
        assert model.revision == 0

    @needs_replace_contents
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

    @needs_replace_contents
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
