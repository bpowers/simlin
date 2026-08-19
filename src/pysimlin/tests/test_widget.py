"""``simlin.widget.ModelWidget`` driven through a recording comm.

No browser, no kernel: ipywidgets creates its comm through
``comm.create_comm``, which is replaced with a subclass that records every
published message, and messages "from the browser" are fed through the same
``comm.handle_msg`` path a kernel uses.  The tests assert on the exact
messages sent (custom messages and trait updates, in order) and on the
project/file state, per the protocol in Section 3 of
``docs/design-plans/2026-08-17-pysimlin-widget.md``.

Message-type x state table covered here (each row is a test):

    wasm      | asset present            -> {type:wasm} + [bytes]
    wasm      | wasm missing             -> {type:wasm, error}
    wasm      | wasm unreadable          -> {type:wasm, error}
    snapshot  | base current, write ok   -> traits (exact json, base+1) in ONE
                                            update, then saved
    snapshot  | base current, same bytes -> update carries only revision
    snapshot  | base stale               -> no trait writes, rejected + warn notice,
                                            file untouched
    snapshot  | unparsable json          -> no trait writes, rejected + warn notice,
                                            project untouched
    snapshot  | write fails after apply  -> traits (exact json, base+1), saved +
                                            warn notice, project dirty
    snapshot  | handler raises anywhere  -> exactly one reply, always: raised
                                            BEFORE applying -> rejected + warn
                                            notice; raised AFTER applying (in
                                            the project's post-commit steps or
                                            in this class) -> an accept (sent
                                            bytes pushed at base+1, saved) +
                                            warn notice; raised AFTER the reply
                                            went out -> nothing more; a
                                            BaseException -> the same reply,
                                            then it propagates
    snapshot  | malformed base/json      -> RuntimeWarning, rejected + warn notice
    oversize  | well-formed              -> RuntimeWarning on the kernel's stderr (a
                                            comm handler runs outside any cell, so
                                            JupyterLab shows it in the Log Console)
                                            + the same warn notice the browser
                                            toasted; no reply, project untouched
    other     | unknown type / bad bytes -> RuntimeWarning, nothing sent

Snapshot size (kernel -> browser direction has no limit; the browser refuses
to send edits above ``max_snapshot_bytes``, measured on the JSON-escaped
text as it rides in the message): a seed above the cap warns once, in the
caller's frame (the user's cell); a push that crosses it warns once per
widget; the trait is a positive integer defaulting to ``MAX_SNAPSHOT_BYTES``
and ``Model.widget()`` passes an override through.

Change-source x delivery table (kernel -> browser):

    edit / disk / reload / other widget  -> traits pushed + notice
    own accepted snapshot                -> nothing further (no remount)
    with a dispatcher                    -> nothing until the dispatcher runs
"""

from __future__ import annotations

import gc
import json
import os
import shutil
import warnings
import weakref
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING, Any

import comm
import pytest
import traitlets
from comm.base_comm import BaseComm

import simlin
from simlin import Aux, ChangeEvent, Model, Project, SimlinAssetError
from simlin._widget_core import MAX_SNAPSHOT_BYTES, snapshot_wire_size
from simlin.widget import ModelWidget, WidgetAssets, resolve_assets

if TYPE_CHECKING:
    from collections.abc import Callable, Iterator

FIXTURES = Path(__file__).parent / "fixtures"
FAKE_MODULE = "export default { render({ model, el }) { el.textContent = 'hi'; } };\n"
FAKE_WASM = b"\x00asm\x01\x00\x00\x00" + bytes(range(64))
IDLE = 3600.0


# ── harness ─────────────────────────────────────────────────────────────


class RecordingComm(BaseComm):
    """The kernel side of the widget comm with the wire replaced by a list.

    ``messages`` holds ``(msg_type, data, buffers)`` for everything the
    widget published (``comm_open``, ``comm_msg``, ``comm_close``).
    """

    def __init__(self, **kwargs: Any) -> None:
        self.messages: list[tuple[str, dict[str, Any] | None, list[Any] | None]] = []
        super().__init__(**kwargs)

    def publish_msg(
        self,
        msg_type: str,
        data: dict[str, Any] | None = None,
        metadata: dict[str, Any] | None = None,
        buffers: list[Any] | None = None,
        **keys: Any,
    ) -> None:
        self.messages.append((msg_type, data, list(buffers) if buffers else None))


@pytest.fixture(autouse=True)
def recording_comm(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(comm, "create_comm", lambda **kwargs: RecordingComm(**kwargs))


@pytest.fixture(autouse=True)
def close_widgets() -> Iterator[None]:
    """ipywidgets keeps every open widget in a registry; close them so one
    test's widgets never observe another test's project changes."""
    yield
    ModelWidget.close_all()


@pytest.fixture
def asset_dir(tmp_path: Path) -> Path:
    directory = tmp_path / "_widget"
    directory.mkdir()
    (directory / "widget.js").write_text(FAKE_MODULE)
    (directory / "libsimlin-browser.wasm").write_bytes(FAKE_WASM)
    return directory


@pytest.fixture
def assets(asset_dir: Path) -> WidgetAssets:
    resolved = resolve_assets(None, asset_dir)
    assert resolved.error is None
    return resolved


@pytest.fixture
def use_assets(assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch) -> WidgetAssets:
    """Make the process-wide resolution the fake one, so ``Model.widget()``
    and ``_repr_mimebundle_`` (which take no ``assets`` argument) work."""
    monkeypatch.setattr("simlin.widget._ASSETS", assets)
    return assets


@pytest.fixture
def model_path(tmp_path: Path) -> Path:
    dst = tmp_path / "teacup.stmx"
    shutil.copyfile(FIXTURES / "teacup.stmx", dst)
    return dst


@pytest.fixture
def model(model_path: Path) -> Model:
    return simlin.open(model_path, watch=False)


def _project(model: Model) -> Project:
    project = model.project
    assert project is not None
    return project


def _widget(model: Model, assets: WidgetAssets, **kwargs: Any) -> ModelWidget:
    return ModelWidget(model, assets=assets, **kwargs)


def _comm(widget: ModelWidget) -> RecordingComm:
    assert isinstance(widget.comm, RecordingComm)
    return widget.comm


def _sent(widget: ModelWidget) -> list[tuple[str, Any, list[Any] | None]]:
    """What the widget published since the last :func:`_drain`, in order,
    normalised to ``("update", state, None)`` for trait syncs and
    ``("custom", content, buffers)`` for custom messages."""
    out: list[tuple[str, Any, list[Any] | None]] = []
    for msg_type, data, buffers in _comm(widget).messages:
        if msg_type != "comm_msg" or data is None:
            continue
        if data["method"] == "update":
            out.append(("update", data["state"], None))
        elif data["method"] == "custom":
            out.append(("custom", data["content"], buffers))
        elif data["method"] == "echo_update":
            # ipywidgets' own acknowledgement of a front-end trait write.
            out.append(("echo", data["state"], None))
        else:  # pragma: no cover - the widget never sends anything else
            raise AssertionError(f"unexpected message {data!r}")
    return out


def _drain(widget: ModelWidget) -> None:
    _comm(widget).messages.clear()


def _from_browser(widget: ModelWidget, content: object, buffers: list[Any] | None = None) -> None:
    """Deliver a custom message the way the kernel would: through the comm."""
    _comm(widget).handle_msg(
        {"content": {"data": {"method": "custom", "content": content}}, "buffers": buffers or []}
    )


def _browser_sets(widget: ModelWidget, **state: Any) -> None:
    """A trait update from the front-end (the browser writing ``selection``)."""
    _comm(widget).handle_msg(
        {
            "content": {"data": {"method": "update", "state": state, "buffer_paths": []}},
            "buffers": [],
        }
    )


def _browser_shaped(engine_json: bytes) -> str:
    """Re-serialise engine JSON the way the browser does (``JSON.stringify``
    of its own object graph): the same document, but NOT the same bytes --
    keys sorted, ``", "``/``": "`` separators, non-ASCII left as UTF-8.  A
    test that only ever sends the engine's own bytes cannot tell "pushed
    the exact string received" from "pushed serialize_json()"."""
    text = json.dumps(json.loads(engine_json), sort_keys=True, ensure_ascii=False)
    assert text != engine_json.decode("utf-8")
    return text


def _snapshot_with(model_path: Path, name: str) -> str:
    """A snapshot the way a browser would produce one: the whole project as
    native JSON (browser-shaped, see :func:`_browser_shaped`), edited by
    adding an aux whose documentation carries a non-ASCII character."""
    scratch = simlin.load(model_path)
    with scratch.edit() as (_, patch):
        patch.upsert(Aux(name=name, equation="42", documentation="Ünïcode ok"))
    return _browser_shaped(_project(scratch).serialize_json())


class _Events:
    def __init__(self) -> None:
        self.events: list[ChangeEvent] = []

    def __call__(self, event: ChangeEvent) -> None:
        self.events.append(event)


# ── construction and seeding ────────────────────────────────────────────


class TestConstruction:
    def test_seeds_traits_from_the_project(self, model: Model, assets: WidgetAssets) -> None:
        w = _widget(model, assets)
        assert w.revision == 0
        assert w.project_json == _project(model).serialize_json().decode("utf-8")
        assert w.height == 600
        assert w.theme == "auto"
        assert w.read_only is False
        assert w.selection == []
        assert w._esm == FAKE_MODULE
        assert w.model is model
        # The comm was opened with the full state (what a browser seeds from).
        opened = [d for t, d, _ in _comm(w).messages if t == "comm_open"]
        assert len(opened) == 1
        assert opened[0] is not None
        state = opened[0]["state"]
        assert state["revision"] == 0
        assert state["project_json"] == w.project_json
        assert state["_esm"] == FAKE_MODULE

    def test_options_are_traits(self, model: Model, assets: WidgetAssets) -> None:
        w = _widget(model, assets, height=320, theme="dark", read_only=True)
        assert (w.height, w.theme, w.read_only) == (320, "dark", True)

    def test_bad_theme_is_rejected(self, model: Model, assets: WidgetAssets) -> None:
        with pytest.raises(traitlets.TraitError):
            _widget(model, assets, theme="sepia")

    def test_unattached_model_raises(self, assets: WidgetAssets) -> None:
        detached = Model.__new__(Model)
        detached._project = None  # a Model constructed without a project
        with pytest.raises(simlin.SimlinRuntimeError, match="not attached"):
            ModelWidget(detached, assets=assets)

    def test_in_memory_project_works_and_snapshot_marks_dirty(self, assets: WidgetAssets) -> None:
        project = Project.new()
        model = project.get_model()
        w = _widget(model, assets)
        _drain(w)
        base = w.revision
        scratch = Project.new()  # the browser's edited copy of the same project
        with scratch.get_model().edit() as (_, patch):
            patch.upsert(Aux(name="added", equation="1"))
        text = scratch.serialize_json().decode("utf-8")
        _from_browser(w, {"type": "snapshot", "base": base, "json": text})
        assert model.revision == base + 1
        assert model.dirty is True
        assert model.get_variable("added") is not None
        assert _sent(w)[-1] == ("custom", {"type": "saved", "revision": base + 1}, None)


class TestSeedHasAView:
    """The Editor mounts a model's first view; a model without one renders
    as a dead, blank editor.  Seeding therefore lays out a diagram-less model
    first (``Project.new() -> edit() -> display`` is the headline path of
    the example notebooks), through the same committed change as
    ``auto_layout()``: the revision bumps, a file-backed project autosaves
    it, and other subscribers hear about it.  A model that already has a
    view is seeded as it is."""

    @staticmethod
    def _views(project_json: str) -> list[Any]:
        doc = json.loads(project_json)
        return doc["models"][0].get("views") or []

    def test_viewless_in_memory_model_is_laid_out_first(self, assets: WidgetAssets) -> None:
        project = Project.new()
        model = project.get_model()
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="rate", equation="0.1"))
        assert self._views(project.serialize_json().decode()) == []
        revision = project.revision
        w = _widget(model, assets)
        views = self._views(w.project_json)
        assert len(views) == 1
        assert {e["name"] for e in views[0]["elements"] if "name" in e} == {"rate"}
        assert w.revision == revision + 1 == project.revision
        # The layout is the project's, not just the seed's.
        assert self._views(project.serialize_json().decode()) == views
        # The widget did not hear about its own seeding layout as a change.
        assert [k for k, _, _ in _sent(w) if k == "custom"] == []

    def test_empty_model_gets_an_empty_view_once(self, assets: WidgetAssets) -> None:
        project = Project.new()
        model = project.get_model()
        w = _widget(model, assets)
        views = self._views(w.project_json)
        assert len(views) == 1
        assert views[0]["elements"] == []
        assert views[0]["kind"] == "stock_flow"
        assert project.revision == 1
        # An empty view over an EMPTY model is left alone by every later
        # display: there is nothing to place, so a second widget (the same
        # model shown in another cell) commits no layout and no revision.
        w2 = _widget(model, assets)
        assert project.revision == 1
        assert self._views(w2.project_json) == views

    def test_viewless_file_backed_model_autosaves_the_layout(
        self, tmp_path: Path, assets: WidgetAssets
    ) -> None:
        project = Project.new()
        path = tmp_path / "scratch.sd.json"
        project.save_as(path)
        model = project.get_model()
        events = _Events()
        project.on_change(events)
        w = _widget(model, assets)
        assert w.revision == 1
        assert model.dirty is False
        on_disk = json.loads(path.read_text())
        assert len(on_disk["models"][0].get("views") or []) == 1
        assert [(e.source, e.revision) for e in events.events] == [("edit", 1)]

    def test_viewless_read_only_suffix_lays_out_in_memory_and_still_edits(
        self, tmp_path: Path, assets: WidgetAssets
    ) -> None:
        # A sketch-less .vpm (read as MDL) is opened without write permission:
        # the display's layout is a committed change that stays in memory (the
        # packaged file is never regenerated as MDL text), the widget seeds the
        # laid-out project, and a browser edit is accepted the same way --
        # dirty, file untouched, `saved` replied.
        text = (FIXTURES / "teacup.mdl").read_text(encoding="utf-8")
        body, marker, _ = text.partition("\\\\\\---///")
        assert marker
        path = tmp_path / "teacup.vpm"
        path.write_text(body, encoding="utf-8")
        before = path.read_bytes()
        model = simlin.open(path, watch=False)
        project = _project(model)
        assert project.writable is False
        w = _widget(model, assets)
        assert w.revision == 1
        assert self._views(w.project_json)[0]["elements"]
        assert model.dirty is True
        assert path.read_bytes() == before
        _drain(w)
        snapshot = _browser_shaped(project.serialize_json())
        _from_browser(w, {"type": "snapshot", "base": 1, "json": snapshot})
        assert model.revision == 2
        assert model.dirty is True
        assert path.read_bytes() == before
        assert [c for k, c, _ in _sent(w) if k == "custom"] == [{"type": "saved", "revision": 2}]

    def test_empty_view_over_variables_is_laid_out(self, assets: WidgetAssets) -> None:
        # An empty view over a model that HAS variables is a blank editor
        # too (a JSON project written before its variables existed, or a
        # sidecar-era file): the display lays it out.  An empty view over an
        # empty model is left alone -- there is nothing to place.
        project = Project.new()
        model = project.get_model()
        project.auto_layout("main")  # an empty stock_flow view, revision 1
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="rate", equation="0.1"))
        # (in-memory + a view -> the edit kept it in step, so unplace it
        # again to build the fixture the arm is about)
        doc = json.loads(project.serialize_json())
        doc["models"][0]["views"][0]["elements"] = []
        scratch = Project._from_bytes(json.dumps(doc).encode(), simlin.FileFormat.NATIVE_JSON)
        scratch_model = scratch.get_model()
        assert self._views(scratch.serialize_json().decode())[0]["elements"] == []
        revision = scratch.revision
        w = _widget(scratch_model, assets)
        views = self._views(w.project_json)
        assert {e["name"] for e in views[0]["elements"] if "name" in e} == {"rate"}
        assert w.revision == revision + 1

    def test_model_with_a_view_is_seeded_as_is(self, model: Model, assets: WidgetAssets) -> None:
        before = _project(model).serialize_json().decode("utf-8")
        w = _widget(model, assets)
        assert w.revision == 0
        assert w.project_json == before

    def test_layout_failure_still_seeds_with_a_warning(
        self, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        project = Project.new()
        model = project.get_model()

        def boom(*args: object, **kwargs: object) -> None:
            raise simlin.SimlinRuntimeError("layout exploded")

        monkeypatch.setattr("simlin.project._ffi_diagram_sync", boom)
        with pytest.warns(RuntimeWarning, match="layout exploded"):
            w = _widget(model, assets)
        assert w.revision == 0
        assert self._views(w.project_json) == []

    def test_write_failure_after_layout_still_seeds_the_laid_out_project(
        self, tmp_path: Path, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        project = Project.new()
        path = tmp_path / "scratch.sd.json"
        project.save_as(path)
        model = project.get_model()

        def fail(*args: object, **kwargs: object) -> None:
            raise OSError("disk full")

        monkeypatch.setattr("simlin.project.atomic_write", fail)
        with pytest.warns(RuntimeWarning, match="disk full"):
            w = _widget(model, assets)
        # The layout is real in memory (and seeded); only the file lags.
        assert w.revision == 1
        assert len(self._views(w.project_json)) == 1
        assert model.dirty is True


# ── wasm ────────────────────────────────────────────────────────────────


class TestWasm:
    def test_request_is_answered_with_the_bytes_as_a_buffer(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        _drain(w)
        _from_browser(w, {"type": "wasm"})
        assert _sent(w) == [("custom", {"type": "wasm"}, [FAKE_WASM])]

    def test_missing_wasm_is_an_error_reply_not_a_hang(self, model: Model, asset_dir: Path) -> None:
        (asset_dir / "libsimlin-browser.wasm").unlink()
        resolved = resolve_assets(None, asset_dir)
        assert resolved.error is None  # the JS module alone is enough to display
        w = _widget(model, resolved)
        _drain(w)
        _from_browser(w, {"type": "wasm"})
        [(kind, content, buffers)] = _sent(w)
        assert (kind, buffers) == ("custom", None)
        assert content["type"] == "wasm"
        assert "libsimlin-browser.wasm" in content["error"]
        assert str(asset_dir) in content["error"]

    def test_unreadable_wasm_is_an_error_reply(
        self, model: Model, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        def fail(path: Path) -> bytes:
            raise OSError("permission denied")

        monkeypatch.setattr("simlin.widget._read_wasm", fail)
        w = _widget(model, assets)
        _drain(w)
        _from_browser(w, {"type": "wasm"})
        [(_, content, _)] = _sent(w)
        assert content["type"] == "wasm"
        assert "permission denied" in content["error"]


# ── snapshots ───────────────────────────────────────────────────────────


class TestSnapshotAccept:
    def test_ac2_2_accept_writes_file_pushes_pair_once_then_saved(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        events = _Events()
        _project(model).on_change(events)
        before = model_path.read_bytes()
        text = _snapshot_with(model_path, "from_widget")
        _drain(w)

        _from_browser(w, {"type": "snapshot", "base": 0, "json": text})

        # File written in its own format, revision advanced by exactly one,
        # subscribers told it came from a widget, run-visible.
        assert model_path.read_bytes() != before
        assert simlin.load(model_path).get_variable("from_widget") is not None
        assert model.revision == 1
        assert model.dirty is False
        assert events.events == [ChangeEvent("widget", 1)]
        assert model.get_variable("from_widget") is not None
        # Exactly one trait update carrying BOTH keys (the exact string the
        # browser sent, so it recognises its own snapshot), then saved; the
        # widget's own on_change delivery pushed nothing further.
        assert _sent(w) == [
            ("update", {"project_json": text, "revision": 1}, None),
            ("custom", {"type": "saved", "revision": 1}, None),
        ]
        assert w.project_json == text
        assert w.revision == 1
        # The exact string received, not the engine's re-serialisation of it
        # (the browser matches its own snapshot by string equality).
        assert text != _project(model).serialize_json().decode("utf-8")
        assert json.loads(text) == json.loads(_project(model).serialize_json())

    def test_identical_snapshot_bumps_revision_and_syncs_only_the_revision(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        text = w.project_json
        _drain(w)
        _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        assert model.revision == 1
        # traitlets does not re-send an equal value; the browser's pair
        # check reads the current project_json, which already matches.
        assert _sent(w) == [
            ("update", {"revision": 1}, None),
            ("custom", {"type": "saved", "revision": 1}, None),
        ]

    def test_consecutive_accepts_chain_on_the_acknowledged_base(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        first = _snapshot_with(model_path, "a")
        _from_browser(w, {"type": "snapshot", "base": 0, "json": first})
        second = _snapshot_with(model_path, "b")  # built from the file, which now has "a"
        _drain(w)
        _from_browser(w, {"type": "snapshot", "base": 1, "json": second})
        assert model.revision == 2
        assert _sent(w) == [
            ("update", {"project_json": second, "revision": 2}, None),
            ("custom", {"type": "saved", "revision": 2}, None),
        ]
        assert model.get_variable("a") is not None
        assert model.get_variable("b") is not None

    def test_own_accept_with_a_dispatcher_pushes_nothing_when_delivered(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        queue: list[Callable[[], None]] = []
        w = _widget(model, assets, dispatch=queue.append)
        text = _snapshot_with(model_path, "x")
        _drain(w)
        _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        assert len(queue) == 1  # the project's notification, not yet delivered
        assert _sent(w) == [
            ("update", {"project_json": text, "revision": 1}, None),
            ("custom", {"type": "saved", "revision": 1}, None),
        ]
        _drain(w)
        for fn in queue:
            fn()
        assert _sent(w) == []  # recognised as our own: no re-push, no notice

    def test_several_own_accepts_queued_behind_a_busy_loop_are_all_recognised(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        # ipykernel 7 handles comm messages on a subshell thread while a cell
        # runs, so three drags during a long cell are three accepted
        # snapshots whose notifications all queue on the (busy) IO loop and
        # drain together afterwards.  Every one of them is this widget's own:
        # none may be re-pushed or announced as "Updated in another view".
        queue: list[Callable[[], None]] = []
        w = _widget(model, assets, dispatch=queue.append)
        _drain(w)
        for i, name in enumerate(["a", "b", "c"]):
            text = _snapshot_with(model_path, name)
            _from_browser(w, {"type": "snapshot", "base": i, "json": text})
        assert model.revision == 3
        assert len(queue) == 3
        _drain(w)
        for fn in queue:
            fn()
        assert _sent(w) == []
        # A foreign change afterwards is still pushed.
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="from_python", equation="1"))
        for fn in queue[3:]:
            fn()
        assert [kind for kind, _, _ in _sent(w)] == ["update", "custom"]


class TestSnapshotReject:
    def test_ac2_5_stale_base_is_rejected_and_never_written(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        with model.edit() as (_, patch):  # the kernel advances to 1
            patch.upsert(Aux(name="kernel_side", equation="1"))
        stale = _snapshot_with(model_path, "stale")  # a snapshot edited from revision 0
        before = model_path.read_bytes()
        _drain(w)

        _from_browser(w, {"type": "snapshot", "base": 0, "json": stale})

        assert model_path.read_bytes() == before
        assert model.revision == 1
        assert model.get_variable("stale") is None
        sent = _sent(w)
        # No trait writes on a reject: the traits already hold revision 1
        # (pushed by the edit); only the reply and its notice go out.
        assert [kind for kind, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 1}
        notice = sent[1][1]
        assert notice["type"] == "notice"
        assert notice["level"] == "warn"
        assert "older version" in notice["text"]
        assert w.revision == 1

    def test_reject_touches_no_trait_even_with_its_notification_still_queued(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        # A kernel edit whose notification is queued behind the browser's
        # message (a dispatcher that has not run yet): the reject itself
        # writes no trait; the queued notification pushes the pair when the
        # loop runs, and the browser remounts then.
        queue: list[Callable[[], None]] = []
        w = _widget(model, assets, dispatch=queue.append)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="kernel_side", equation="1"))
        assert w.revision == 0
        assert len(queue) == 1
        stale = _snapshot_with(model_path, "stale")
        _drain(w)
        _from_browser(w, {"type": "snapshot", "base": 0, "json": stale})
        sent = _sent(w)
        assert [kind for kind, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 1}
        assert w.revision == 0
        _drain(w)
        for fn in queue:
            fn()
        sent = _sent(w)
        assert sent[0][0] == "update"
        assert sent[0][1]["revision"] == 1
        assert sent[1] == (
            "custom",
            {"type": "notice", "text": "Updated from Python", "level": "info"},
            None,
        )
        assert w.revision == 1

    def test_unparsable_snapshot_is_rejected_with_the_error(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        before = model_path.read_bytes()
        _drain(w)
        with pytest.warns(RuntimeWarning, match="could not be applied"):
            _from_browser(w, {"type": "snapshot", "base": 0, "json": "{not json"})
        assert model.revision == 0
        assert model_path.read_bytes() == before
        sent = _sent(w)
        assert [kind for kind, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 0}
        assert sent[1][1]["level"] == "warn"
        assert "could not be applied" in sent[1][1]["text"]

    def test_write_failure_after_apply_is_saved_plus_warning_and_leaves_project_dirty(
        self,
        model: Model,
        model_path: Path,
        assets: WidgetAssets,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        w = _widget(model, assets)
        events = _Events()
        _project(model).on_change(events)
        text = _snapshot_with(model_path, "unsaved")
        before = model_path.read_bytes()

        def fail(*args: object, **kwargs: object) -> None:
            raise OSError("disk full")

        monkeypatch.setattr("simlin.project.atomic_write", fail)
        _drain(w)
        with pytest.warns(RuntimeWarning, match="disk full"):
            _from_browser(w, {"type": "snapshot", "base": 0, "json": text})

        # The change is real in memory (revision 1, dirty, notified once)
        # and the file untouched...
        assert model.revision == 1
        assert model.dirty is True
        assert model.get_variable("unsaved") is not None
        assert model_path.read_bytes() == before
        assert events.events == [ChangeEvent("widget", 1)]
        # ...and the browser is told exactly what an accept says -- its
        # acknowledged version must match the kernel's or every later save
        # would be stale -- plus a warning naming the error and the fix.
        assert text != _project(model).serialize_json().decode("utf-8")
        assert _sent(w) == [
            ("update", {"project_json": text, "revision": 1}, None),
            ("custom", {"type": "saved", "revision": 1}, None),
            (
                "custom",
                {
                    "type": "notice",
                    "text": (
                        "Your edit was applied but could not be written to the file: "
                        "disk full. The model is marked dirty; call model.save() to "
                        "retry the write."
                    ),
                    "level": "warn",
                },
                None,
            ),
        ]
        # The next edit chains on the acknowledged base and is written once
        # the disk is back.
        monkeypatch.undo()
        model.save()
        assert model.dirty is False
        assert simlin.load(model_path).get_variable("unsaved") is not None

    def test_write_failure_with_a_dispatcher_does_not_push_twice(
        self,
        model: Model,
        model_path: Path,
        assets: WidgetAssets,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        queue: list[Callable[[], None]] = []
        w = _widget(model, assets, dispatch=queue.append)
        text = _snapshot_with(model_path, "unsaved")

        def fail(*args: object, **kwargs: object) -> None:
            raise OSError("disk full")

        monkeypatch.setattr("simlin.project.atomic_write", fail)
        _drain(w)
        with pytest.warns(RuntimeWarning, match="disk full"):
            _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        assert [kind for kind, _, _ in _sent(w)] == ["update", "custom", "custom"]
        _drain(w)
        for fn in queue:
            fn()
        # The queued notification is for the revision this snapshot produced.
        assert _sent(w) == []


class TestExactlyOneReply:
    """MUST: every snapshot gets exactly one ``saved``/``rejected`` with an
    integer revision, whatever happens while handling it."""

    def test_a_bug_in_the_handler_still_replies_rejected(
        self, model: Model, model_path: Path, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        w = _widget(model, assets)

        def boom(*args: object, **kwargs: object) -> bool:
            raise RuntimeError("unexpected")

        monkeypatch.setattr(_project(model), "_apply_snapshot", boom)
        text = _snapshot_with(model_path, "x")
        _drain(w)
        with pytest.warns(RuntimeWarning, match="unexpected"):
            _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        sent = _sent(w)
        assert [kind for kind, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 0}
        assert sent[1][1]["type"] == "notice"
        assert "unexpected" in sent[1][1]["text"]

    def test_a_failure_while_pushing_still_replies_saved(
        self, model: Model, model_path: Path, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        w = _widget(model, assets)
        text = _snapshot_with(model_path, "x")

        def boom(*args: object, **kwargs: object) -> None:
            raise RuntimeError("push exploded")

        monkeypatch.setattr(w, "_push", boom)
        _drain(w)
        with pytest.warns(RuntimeWarning, match="push exploded"):
            _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        # The change was applied (the file has it), so the reply is ``saved``
        # at base+1 even though the state push itself is what failed: the
        # browser adopts (sent bytes, base+1) from ``saved`` on its own, and a
        # ``rejected`` here would leave it one revision behind for good.
        assert model.revision == 1
        assert simlin.load(model_path).get_variable("x") is not None
        replies = [
            c for k, c, _ in _sent(w) if k == "custom" and c["type"] in ("saved", "rejected")
        ]
        assert replies == [{"type": "saved", "revision": 1}]

    def test_a_failure_after_apply_is_an_accept_with_the_sent_bytes(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        # The snapshot applied (revision moved, file written) and the handler
        # failed AFTER that, before the browser was told.  This is an accept
        # (obligation 5's logic): push the EXACT bytes received at base+1 and
        # reply ``saved``.  A ``rejected`` would wedge the view: in the steady
        # state the kernel's re-serialization equals the sent bytes byte for
        # byte (same engine on both sides), so a re-push classifies as the
        # browser's own ack and the following ``rejected`` re-seeds onto the
        # pair it already holds -- the browser's acknowledged base stays one
        # behind and every later save is stale.  Pinned with the engine's own
        # bytes so the fixture IS that steady state.
        w = _widget(model, assets)
        scratch = simlin.load(model_path)
        with scratch.edit() as (current, patch):
            patch.upsert(replace(current["room temperature"], equation="60"))
        text = _project(scratch).serialize_json().decode("utf-8")

        def boom(*args: object, **kwargs: object) -> None:
            raise RuntimeError("reply planning exploded")

        _drain(w)
        with pytest.MonkeyPatch.context() as broken:
            broken.setattr("simlin.widget.plan_snapshot_reply", boom)
            with pytest.warns(RuntimeWarning, match="reply planning exploded"):
                _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        assert model.revision == 1
        assert _project(model).serialize_json().decode("utf-8") == text  # steady state
        sent = _sent(w)
        assert [k for k, _, _ in sent] == ["update", "custom", "custom"]
        assert sent[0][1] == {"project_json": text, "revision": 1}
        assert sent[1][1] == {"type": "saved", "revision": 1}
        assert sent[2][1]["type"] == "notice"
        assert sent[2][1]["level"] == "warn"
        # The browser chains from the acknowledged base as after any accept.
        _drain(w)
        _from_browser(w, {"type": "snapshot", "base": 1, "json": _snapshot_with(model_path, "y")})
        assert model.revision == 2
        assert _sent(w)[-1] == ("custom", {"type": "saved", "revision": 2}, None)

    def test_a_failure_before_apply_leaves_no_own_marker_for_another_views_snapshot(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        # The handler failed BEFORE anything was applied (nothing moved): the
        # reply is ``rejected`` at the current revision and the re-push of the
        # project's pair is a no-op (unchanged traits send nothing).  The
        # revision the snapshot WOULD have produced (base + 1) was remembered
        # as this widget's own before the call and must be forgotten again:
        # the observable is a SECOND widget's accepted snapshot at that very
        # revision -- a ``widget``-sourced change, the only source
        # ``is_own_change`` ever treats as own -- which w1 must push as
        # foreign rather than skip as its own.
        w1 = _widget(model, assets)
        w2 = _widget(model, assets)
        text = _snapshot_with(model_path, "x")

        def boom(*args: object, **kwargs: object) -> bool:
            raise RuntimeError("apply exploded")

        _drain(w1)
        with pytest.MonkeyPatch.context() as broken:
            broken.setattr(_project(model), "_apply_snapshot", boom)
            with pytest.warns(RuntimeWarning, match="apply exploded"):
                _from_browser(w1, {"type": "snapshot", "base": 0, "json": text})
        assert model.revision == 0
        sent = _sent(w1)
        assert [k for k, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 0}
        _drain(w1)
        _drain(w2)
        from_w2 = _snapshot_with(model_path, "from_w2")
        _from_browser(w2, {"type": "snapshot", "base": 0, "json": from_w2})
        assert model.revision == 1
        assert _sent(w2)[-1] == ("custom", {"type": "saved", "revision": 1}, None)
        sent = _sent(w1)
        assert [k for k, _, _ in sent] == ["update", "custom"]
        assert sent[0][1]["revision"] == 1
        assert sent[1] == (
            "custom",
            {"type": "notice", "text": "Updated in another view", "level": "info"},
            None,
        )

    def test_a_notify_failure_after_apply_is_an_accept(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        # The change applied and the file was written; what failed was
        # ``Project._notify`` -- here the widget's own dispatcher, the shape
        # a closed kernel loop takes -- which the project raises as
        # ``SimlinWriteError`` (applied but ...).  The reply is ``saved`` at
        # base + 1 with the sent bytes pushed, plus a warn notice that names
        # the error but not model.save(): there is nothing to retry.
        def closed_loop(fn: Callable[[], None]) -> None:
            raise RuntimeError("IOLoop is closed")

        w = _widget(model, assets, dispatch=closed_loop)
        text = _snapshot_with(model_path, "x")
        _drain(w)
        with pytest.warns(RuntimeWarning, match="IOLoop is closed"):
            _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        assert model.revision == 1
        assert model.dirty is False
        assert simlin.load(model_path).get_variable("x") is not None
        sent = _sent(w)
        assert [k for k, _, _ in sent] == ["update", "custom", "custom"]
        assert sent[0][1] == {"project_json": text, "revision": 1}
        assert sent[1][1] == {"type": "saved", "revision": 1}
        notice = sent[2][1]
        assert notice["type"] == "notice"
        assert notice["level"] == "warn"
        assert "IOLoop is closed" in notice["text"]
        assert "model.save()" not in notice["text"]

    @pytest.mark.parametrize("arm", ["before-apply", "after-apply"])
    def test_a_base_exception_still_gets_its_one_reply_then_propagates(
        self, model: Model, model_path: Path, assets: WidgetAssets, arm: str
    ) -> None:
        # A KeyboardInterrupt landing while a comm message is handled is not
        # the widget's to swallow, but the snapshot is still owed its reply
        # first -- rejected when nothing was applied, saved when it was --
        # or the view wedges on top of the interrupt.
        w = _widget(model, assets)
        text = _snapshot_with(model_path, "x")

        def interrupt(*args: object, **kwargs: object) -> None:
            raise KeyboardInterrupt

        target = (
            (_project(model), "_apply_snapshot")
            if arm == "before-apply"
            else ("simlin.widget", "plan_snapshot_reply")
        )
        _drain(w)
        with pytest.MonkeyPatch.context() as broken:
            if isinstance(target[0], str):
                broken.setattr(f"{target[0]}.{target[1]}", interrupt)
            else:
                broken.setattr(target[0], target[1], interrupt)
            with pytest.raises(KeyboardInterrupt):
                _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        replies = [
            c for k, c, _ in _sent(w) if k == "custom" and c["type"] in ("saved", "rejected")
        ]
        if arm == "before-apply":
            assert model.revision == 0
            assert replies == [{"type": "rejected", "revision": 0}]
        else:
            assert model.revision == 1
            assert replies == [{"type": "saved", "revision": 1}]
        notice = [c for k, c, _ in _sent(w) if k == "custom" and c["type"] == "notice"]
        assert len(notice) == 1
        assert "KeyboardInterrupt" in notice[0]["text"]

    def test_a_failure_after_the_reply_went_out_does_not_reply_twice(
        self, model: Model, model_path: Path, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        # The write-failure arm sends ``saved`` and then a warn notice; if
        # sending the notice raises, the reply has already gone out and a
        # second one would be consumed by the browser's NEXT snapshot.
        w = _widget(model, assets)
        text = _snapshot_with(model_path, "unsaved")

        def fail(*args: object, **kwargs: object) -> None:
            raise OSError("disk full")

        monkeypatch.setattr("simlin.project.atomic_write", fail)
        real_send = w.send

        def flaky_send(content: Any, buffers: Any = None) -> None:
            if isinstance(content, dict) and content.get("type") == "notice":
                raise RuntimeError("comm hiccup")
            real_send(content, buffers)

        monkeypatch.setattr(w, "send", flaky_send)
        _drain(w)
        with pytest.warns(RuntimeWarning) as record:
            _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        assert any("comm hiccup" in str(r.message) for r in record)
        replies = [
            c for k, c, _ in _sent(w) if k == "custom" and c["type"] in ("saved", "rejected")
        ]
        assert replies == [{"type": "saved", "revision": 1}]
        assert model.revision == 1
        assert w.revision == 1

    @pytest.mark.parametrize(
        "arm",
        ["accept", "stale", "unparsable", "write-failure", "handler-bug", "bug-after-apply"],
    )
    def test_every_arm_replies_exactly_once_with_an_int_revision(
        self,
        model: Model,
        model_path: Path,
        assets: WidgetAssets,
        monkeypatch: pytest.MonkeyPatch,
        arm: str,
    ) -> None:
        w = _widget(model, assets)
        base = 0
        text = _snapshot_with(model_path, "x")
        if arm == "stale":
            with model.edit() as (_, patch):
                patch.upsert(Aux(name="kernel_side", equation="1"))
        elif arm == "unparsable":
            text = "{"
        elif arm == "write-failure":

            def fail(*args: object, **kwargs: object) -> None:
                raise OSError("disk full")

            monkeypatch.setattr("simlin.project.atomic_write", fail)
        elif arm == "handler-bug":

            def boom(*args: object, **kwargs: object) -> bool:
                raise RuntimeError("bug")

            monkeypatch.setattr(_project(model), "_apply_snapshot", boom)
        elif arm == "bug-after-apply":

            def boom_after(*args: object, **kwargs: object) -> None:
                raise RuntimeError("bug after apply")

            monkeypatch.setattr("simlin.widget.plan_snapshot_reply", boom_after)
        _drain(w)
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            _from_browser(w, {"type": "snapshot", "base": base, "json": text})
        replies = [
            c for k, c, _ in _sent(w) if k == "custom" and c["type"] in ("saved", "rejected")
        ]
        assert len(replies) == 1
        assert type(replies[0]["revision"]) is int
        assert replies[0]["revision"] == model.revision

    def test_state_update_leaves_before_saved(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        text = _snapshot_with(model_path, "x")
        _drain(w)
        _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        kinds = [(k, c.get("type") if k == "custom" else None) for k, c, _ in _sent(w)]
        assert kinds == [("update", None), ("custom", "saved")]


class TestMalformed:
    @pytest.mark.parametrize(
        "content",
        [
            {"type": "snapshot", "base": "0", "json": "{}"},
            {"type": "snapshot", "base": 0},
            {"type": "snapshot"},
        ],
    )
    def test_malformed_snapshot_still_gets_exactly_one_rejected_reply(
        self, model: Model, assets: WidgetAssets, content: object
    ) -> None:
        w = _widget(model, assets)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="k", equation="1"))
        _drain(w)
        with pytest.warns(RuntimeWarning, match="malformed snapshot"):
            _from_browser(w, content)
        sent = _sent(w)
        assert [kind for kind, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 1}
        assert sent[1][1]["type"] == "notice"
        assert sent[1][1]["level"] == "warn"
        assert model.revision == 1

    @pytest.mark.parametrize(
        "content",
        [
            {"type": "saved", "revision": 1},
            "hello",
            {"type": "oversize"},
            {"type": "oversize", "bytes": "big"},
        ],
    )
    def test_ignored_with_a_warning(
        self, model: Model, assets: WidgetAssets, content: object
    ) -> None:
        w = _widget(model, assets)
        _drain(w)
        with pytest.warns(RuntimeWarning, match="ignored a message"):
            _from_browser(w, content)
        assert _sent(w) == []
        assert model.revision == 0


# ── snapshot size ───────────────────────────────────────────────────────


class TestSnapshotSize:
    def test_default_cap_is_the_trait_and_is_seeded_to_the_browser(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        assert w.max_snapshot_bytes == MAX_SNAPSHOT_BYTES
        opened = [d for t, d, _ in _comm(w).messages if t == "comm_open"]
        assert opened[0] is not None
        assert opened[0]["state"]["max_snapshot_bytes"] == MAX_SNAPSHOT_BYTES

    def test_widget_method_passes_the_cap_through(
        self, model: Model, use_assets: WidgetAssets
    ) -> None:
        assert model.widget().max_snapshot_bytes == MAX_SNAPSHOT_BYTES
        assert (
            model.widget(max_snapshot_bytes=64 * 1024 * 1024).max_snapshot_bytes == 64 * 1024 * 1024
        )

    def test_seeding_a_project_above_the_cap_warns_once(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        # A tiny cap stands in for a huge model (repo rule: never build a
        # fixture large enough to trip a production threshold). The cap is
        # compared against the snapshot's WIRE size (JSON-escaped), the same
        # measure the browser applies, not the raw text length.
        raw = _project(model).serialize_json().decode("utf-8")
        size = snapshot_wire_size(raw)
        assert size > len(raw.encode("utf-8"))  # the model has quotes to escape
        with pytest.warns(RuntimeWarning, match="will not be saved") as record:
            w = _widget(model, assets, max_snapshot_bytes=size - 1)
        assert len(record) == 1
        text = str(record[0].message)
        assert "websocket_max_message_size" in text
        assert "max_snapshot_bytes" in text
        # Attributed to the caller outside pysimlin -- this test's frame --
        # so in a notebook it lands in the display cell's output.
        assert record[0].filename == __file__
        # The project still displays: the seed went out in full.
        assert w.project_json == _project(model).serialize_json().decode("utf-8")
        # A push after that (a Python edit) does not warn again for this widget.
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            with model.edit() as (_, patch):
                patch.upsert(Aux(name="grows", equation="1"))

    def test_seeding_at_the_cap_does_not_warn_and_a_push_across_it_warns_once(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        size = snapshot_wire_size(_project(model).serialize_json().decode("utf-8"))
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            w = _widget(model, assets, max_snapshot_bytes=size)
        _drain(w)
        with (
            pytest.warns(RuntimeWarning, match="will not be saved") as record,
            model.edit() as (_, patch),
        ):
            patch.upsert(Aux(name="grows", equation="1"))
        assert len(record) == 1
        assert record[0].filename == __file__
        # The push itself is unaffected: the browser gets the full pair.
        sent = _sent(w)
        assert sent[0][0] == "update"
        assert sent[0][1]["revision"] == 1
        assert "grows" in sent[0][1]["project_json"]
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            with model.edit() as (_, patch):
                patch.upsert(Aux(name="grows_more", equation="2"))

    def test_display_path_warning_is_attributed_to_the_caller_too(
        self, model: Model, use_assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        # Model._repr_mimebundle_ -> Model.widget -> ModelWidget: deeper than
        # the direct construction above, and the warning must still name the
        # user's frame (this test), not model.py -- even when a kernel's
        # display formatter sits between the cell and the repr.
        with pytest.warns(RuntimeWarning, match="will not be saved") as record:
            model.widget(max_snapshot_bytes=16)
        assert record[0].filename == __file__
        # A bare display passes no cap: route the display's widget() call
        # through a cap the fixture model trips (the constant is never
        # lowered; only the argument this display would pass).
        original_widget = Model.widget

        def small_cap_widget(self: Model, **kwargs: Any) -> ModelWidget:
            return original_widget(self, max_snapshot_bytes=16, **kwargs)

        monkeypatch.setattr(Model, "widget", small_cap_widget)
        with pytest.warns(RuntimeWarning, match="will not be saved") as record:
            model._repr_mimebundle_()
        assert record[0].filename == __file__
        namespace: dict[str, Any] = {
            "__name__": "IPython.core.formatters",
            "fn": model._repr_mimebundle_,
        }
        with pytest.warns(RuntimeWarning, match="will not be saved") as record:
            exec("fn()", namespace)
        assert record[0].filename == __file__

    @pytest.mark.parametrize("bad", [0, -1, 1.5, "4096"])
    def test_cap_must_be_a_positive_integer(
        self, model: Model, assets: WidgetAssets, bad: object
    ) -> None:
        with pytest.raises(traitlets.TraitError):
            _widget(model, assets, max_snapshot_bytes=bad)

    def test_cap_rejects_bool(self, model: Model, assets: WidgetAssets) -> None:
        # bool is an int in Python; True is never a byte count. The trait
        # class refuses it (see ModelWidget.max_snapshot_bytes).
        with pytest.raises(traitlets.TraitError):
            _widget(model, assets, max_snapshot_bytes=True)

    def test_seeding_below_the_default_cap_is_silent(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            _widget(model, assets)

    def test_browser_oversize_report_warns_and_notices_without_a_reply(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        before = model_path.read_bytes()
        _drain(w)
        with pytest.warns(RuntimeWarning, match="was not saved") as record:
            _from_browser(w, {"type": "oversize", "bytes": 9_000_000})
        assert len(record) == 1
        assert "from Python" in str(record[0].message)
        # One warn notice naming both sizes; no saved/rejected (nothing was
        # in flight on the browser side) and no trait writes.
        assert _sent(w) == [
            (
                "custom",
                {
                    "type": "notice",
                    "level": "warn",
                    "text": (
                        "Edit not saved: the model is too large for the notebook connection "
                        "(8.6 MiB > 8 MiB limit); edit it from Python instead."
                    ),
                },
                None,
            )
        ]
        assert model.revision == 0
        assert model_path.read_bytes() == before

    def test_oversize_notice_reports_the_widgets_own_cap(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets, max_snapshot_bytes=64 * 1024 * 1024)
        _drain(w)
        with pytest.warns(RuntimeWarning, match="64 MiB"):
            _from_browser(w, {"type": "oversize", "bytes": 65 * 1024 * 1024})
        assert "65 MiB > 64 MiB limit" in _sent(w)[0][1]["text"]

    def test_small_caps_read_in_kib(self, model: Model, assets: WidgetAssets) -> None:
        with pytest.warns(RuntimeWarning, match="will not be saved"):  # the seed, above 16 B
            w = _widget(model, assets, max_snapshot_bytes=16)
        _drain(w)
        with pytest.warns(RuntimeWarning, match="1 KiB"):
            _from_browser(w, {"type": "oversize", "bytes": 1024})
        assert "(1 KiB > 0 KiB limit)" in _sent(w)[0][1]["text"]


# ── kernel-originated changes ───────────────────────────────────────────


class TestKernelChanges:
    def test_ac2_3_edit_pushes_pair_and_notice(self, model: Model, assets: WidgetAssets) -> None:
        w = _widget(model, assets)
        _drain(w)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="from_python", equation="1"))
        expected = _project(model).serialize_json().decode("utf-8")
        assert _sent(w) == [
            ("update", {"project_json": expected, "revision": 1}, None),
            ("custom", {"type": "notice", "text": "Updated from Python", "level": "info"}, None),
        ]
        assert "from_python" in w.project_json

    def test_ac2_3_disk_change_pushes_pair_and_updated_on_disk(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        project = _project(model)
        project.watch(True, interval=IDLE)
        try:
            w = _widget(model, assets)
            _drain(w)
            external = simlin.load(model_path)
            with external.edit() as (_, patch):
                patch.upsert(Aux(name="external", equation="2"))
            _project(external).save_as(model_path)
            watcher = project._watcher
            assert watcher is not None
            watcher.poll_once()
            sent = _sent(w)
            assert sent[0][0] == "update"
            assert sent[0][1]["revision"] == 1
            assert "external" in sent[0][1]["project_json"]
            assert sent[1] == (
                "custom",
                {"type": "notice", "text": "Updated on disk", "level": "info"},
                None,
            )
        finally:
            project.watch(False)

    def test_reload_pushes_pair_and_reloaded_notice(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        external = simlin.load(model_path)
        with external.edit() as (_, patch):
            patch.upsert(Aux(name="external", equation="2"))
        _project(external).save_as(model_path)
        _drain(w)
        assert model.reload() is True
        sent = _sent(w)
        assert sent[0][1]["revision"] == 1
        assert sent[1][1] == {"type": "notice", "text": "Reloaded from disk", "level": "info"}

    def test_dispatcher_defers_delivery(self, model: Model, assets: WidgetAssets) -> None:
        queue: list[Callable[[], None]] = []
        w = _widget(model, assets, dispatch=queue.append)
        _drain(w)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="from_python", equation="1"))
        assert _sent(w) == []  # nothing crosses the comm until the loop runs
        assert w.revision == 0
        for fn in queue:
            fn()
        assert w.revision == 1
        assert [kind for kind, _, _ in _sent(w)] == ["update", "custom"]

    def test_deliveries_read_the_current_state_not_the_event(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        # Two edits queued behind a busy kernel: the first delivery already
        # pushes the latest pair, the second finds nothing new to push (its
        # notice still fires: it is an event, not state).
        queue: list[Callable[[], None]] = []
        w = _widget(model, assets, dispatch=queue.append)
        _drain(w)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="one", equation="1"))
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="two", equation="2"))
        for fn in queue:
            fn()
        sent = _sent(w)
        updates = [s for k, s, _ in sent if k == "update"]
        assert len(updates) == 1
        assert updates[0]["revision"] == 2
        assert "two" in updates[0]["project_json"]

    def test_default_dispatch_comes_from_the_kernel_loop(
        self, model: Model, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        calls: list[Callable[[], None]] = []

        class Loop:
            def add_callback(self, fn: Callable[[], None]) -> None:
                calls.append(fn)

        class Kernel:
            io_loop = Loop()

        class Shell:
            kernel = Kernel()

        monkeypatch.setattr("IPython.core.getipython.get_ipython", lambda: Shell())
        w = _widget(model, assets)
        _drain(w)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="from_python", equation="1"))
        assert len(calls) == 1
        assert _sent(w) == []
        calls[0]()
        assert w.revision == 1

    def test_no_kernel_means_direct_delivery(
        self, model: Model, assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        monkeypatch.setattr("IPython.core.getipython.get_ipython", lambda: None)
        w = _widget(model, assets)
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="from_python", equation="1"))
        assert w.revision == 1


# ── selection ───────────────────────────────────────────────────────────


class TestSelection:
    def test_ac2_7_browser_selection_reaches_the_model(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        _drain(w)
        _browser_sets(w, selection=["teacup_temperature", "room_temperature"])
        assert model.selection == ("teacup_temperature", "room_temperature")
        assert w.selection == ["teacup_temperature", "room_temperature"]
        # Nothing but ipywidgets' protocol-level echo goes back: the value
        # the browser just sent is not re-pushed as a change.
        assert _sent(w) == [
            ("echo", {"selection": ["teacup_temperature", "room_temperature"]}, None)
        ]
        _browser_sets(w, selection=[])
        assert model.selection == ()


# ── several widgets, one project ────────────────────────────────────────


class TestTwoWidgets:
    def test_ac2_4_accept_in_one_view_updates_the_other(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        a = _widget(model, assets)
        b = _widget(model, assets)
        text = _snapshot_with(model_path, "from_a")
        _drain(a)
        _drain(b)
        _from_browser(a, {"type": "snapshot", "base": 0, "json": text})
        assert _sent(a) == [
            ("update", {"project_json": text, "revision": 1}, None),
            ("custom", {"type": "saved", "revision": 1}, None),
        ]
        sent_b = _sent(b)
        assert sent_b[0][0] == "update"
        assert sent_b[0][1]["revision"] == 1
        assert json.loads(sent_b[0][1]["project_json"]) == json.loads(text)
        assert sent_b[1] == (
            "custom",
            {"type": "notice", "text": "Updated in another view", "level": "info"},
            None,
        )
        assert a.revision == b.revision == 1

    def test_rejection_never_swallows_the_other_views_queued_notification(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        # a's accept produces revision 1 and queues b's notification; b then
        # sends a snapshot from revision 0, which is rejected.  The revision
        # b's snapshot WOULD have produced is also 1 -- but b produced
        # nothing, so when the queue drains b must still treat a's change as
        # foreign: the pair is pushed and its notice goes out.
        queue: list[Callable[[], None]] = []
        a = _widget(model, assets)
        b = _widget(model, assets, dispatch=queue.append)
        _from_browser(a, {"type": "snapshot", "base": 0, "json": _snapshot_with(model_path, "a")})
        assert len(queue) == 1
        _drain(b)
        _from_browser(b, {"type": "snapshot", "base": 0, "json": _snapshot_with(model_path, "b")})
        assert [c["type"] for k, c, _ in _sent(b) if k == "custom"] == ["rejected", "notice"]
        assert b.revision == 0  # a reject writes no trait
        _drain(b)
        for fn in queue:
            fn()
        sent = _sent(b)
        assert sent[0][0] == "update"
        assert sent[0][1]["revision"] == 1
        assert sent[1] == (
            "custom",
            {"type": "notice", "text": "Updated in another view", "level": "info"},
            None,
        )
        assert b.revision == 1

    def test_stale_snapshot_from_the_other_view_is_rejected(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        a = _widget(model, assets)
        b = _widget(model, assets)
        from_a = _snapshot_with(model_path, "from_a")
        from_b = _snapshot_with(model_path, "from_b")  # both edited from revision 0
        _from_browser(a, {"type": "snapshot", "base": 0, "json": from_a})
        _drain(b)
        _from_browser(b, {"type": "snapshot", "base": 0, "json": from_b})
        assert model.get_variable("from_a") is not None
        assert model.get_variable("from_b") is None
        assert [c["type"] for k, c, _ in _sent(b) if k == "custom"] == ["rejected", "notice"]
        # b's traits already carried a's state (pushed when a was accepted).
        assert json.loads(b.project_json) == json.loads(from_a)
        assert b.revision == 1


# ── cleanup ─────────────────────────────────────────────────────────────


class TestCleanup:
    def test_close_unsubscribes_and_is_idempotent(self, model: Model, assets: WidgetAssets) -> None:
        project = _project(model)
        w = _widget(model, assets)
        assert len(project._listeners) == 1
        w.close()
        assert project._listeners == {}
        w.close()
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="after_close", equation="1"))
        assert w.revision == 0  # no longer following the project

    def test_close_all_releases_every_subscription(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        project = _project(model)
        _widget(model, assets)
        _widget(model, assets)
        assert len(project._listeners) == 2
        ModelWidget.close_all()
        assert project._listeners == {}

    def test_close_all_leaves_other_ipywidgets_open(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        # The inherited Widget.close_all() closes every widget in the kernel;
        # ours is scoped to model editors.
        import ipywidgets

        other = ipywidgets.IntSlider()
        w = _widget(model, assets)
        ModelWidget.close_all()
        assert w.comm is None
        assert other.comm is not None
        # A closed widget is not closed twice; a fresh one is.
        w2 = _widget(model, assets)
        ModelWidget.close_all()
        assert w2.comm is None
        other.close()

    def test_subscription_never_keeps_a_closed_widget_alive(
        self, model: Model, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        ref = weakref.ref(w)
        w.close()
        del w
        gc.collect()
        assert ref() is None

    def test_widget_keeps_its_project_alive_while_open(
        self, model_path: Path, assets: WidgetAssets
    ) -> None:
        # Like any displayed ipywidget it stays registered until closed, and
        # its editor must keep working after the cell's variable is dropped.
        model = simlin.open(model_path, watch=False)
        w = _widget(model, assets)
        project_ref = weakref.ref(_project(model))
        del model
        gc.collect()
        assert project_ref() is not None
        w.close()

    def test_failed_construction_leaves_no_subscription(
        self, model: Model, asset_dir: Path
    ) -> None:
        (asset_dir / "widget.js").unlink()
        with pytest.raises(SimlinAssetError):
            _widget(model, resolve_assets(None, asset_dir))
        assert _project(model)._listeners == {}


# ── Model.widget() and display ──────────────────────────────────────────


class TestModelDisplay:
    def test_widget_method_passes_options(self, model: Model, use_assets: WidgetAssets) -> None:
        w = model.widget(height=400, theme="light", read_only=True)
        assert isinstance(w, ModelWidget)
        assert (w.height, w.theme, w.read_only) == (400, "light", True)
        assert w.model is model

    def test_ac2_1_repr_mimebundle_carries_widget_and_svg(
        self, model: Model, use_assets: WidgetAssets
    ) -> None:
        data, metadata = model._repr_mimebundle_()
        assert data["application/vnd.jupyter.widget-view+json"]["version_major"] == 2
        assert data["application/vnd.jupyter.widget-view+json"]["model_id"]
        assert len(_project(model)._listeners) == 1  # the widget behind the bundle
        assert data["image/svg+xml"] == model.diagram().svg
        assert data["image/svg+xml"].lstrip().startswith("<svg")
        assert data["text/plain"] == repr(model)
        assert isinstance(metadata, dict)

    def test_each_display_is_a_fresh_widget_and_the_model_holds_none(
        self, model: Model, use_assets: WidgetAssets
    ) -> None:
        first, _ = model._repr_mimebundle_()
        second, _ = model._repr_mimebundle_()
        id1 = first["application/vnd.jupyter.widget-view+json"]["model_id"]
        id2 = second["application/vnd.jupyter.widget-view+json"]["model_id"]
        assert id1 != id2
        assert len(_project(model)._listeners) == 2
        assert not any(isinstance(v, ModelWidget) for v in vars(model).values())

    def test_missing_asset_raises_actionably_on_display_not_import(
        self, model: Model, asset_dir: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        (asset_dir / "widget.js").unlink()
        monkeypatch.setattr("simlin.widget._ASSETS", resolve_assets(None, asset_dir))
        with pytest.raises(SimlinAssetError, match=r"widget\.js") as excinfo:
            model.widget()
        assert str(asset_dir) in str(excinfo.value)
        # Displaying degrades to the static diagram plus the actionable
        # message as a warning: the notebook user sees the picture and the
        # fix, not a traceback.
        with pytest.warns(RuntimeWarning, match=r"widget\.js") as record:
            data, metadata = model._repr_mimebundle_()
        assert str(asset_dir) in str(record[0].message)
        assert "application/vnd.jupyter.widget-view+json" not in data
        assert data["image/svg+xml"] == model.diagram().svg
        # The plain-text repr carries the same message: Python shows a
        # warning once per source location (a re-run of the same cell may
        # not repeat it), the repr is printed with every display.
        assert data["text/plain"].startswith(repr(model))
        assert "interactive editor unavailable" in data["text/plain"]
        assert "widget.js" in data["text/plain"]
        assert metadata == {}
        assert _project(model)._listeners == {}  # no half-made widget left behind
        assert simlin.ModelWidget is ModelWidget  # the package itself is fine

    def test_svg_failure_still_displays_the_widget(
        self, model: Model, use_assets: WidgetAssets, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        def fail(self: Model) -> dict[str, str]:
            raise simlin.SimlinRuntimeError("no diagram")

        monkeypatch.setattr(Model, "_svg_mimebundle", fail)
        with pytest.warns(RuntimeWarning, match="no static diagram"):
            data, _ = model._repr_mimebundle_()
        assert "application/vnd.jupyter.widget-view+json" in data
        assert "image/svg+xml" not in data


# ── asset resolution (SIMLIN_WIDGET_ASSET) ──────────────────────────────


class TestAssets:
    def test_bundled_reads_the_module_and_finds_the_wasm(self, asset_dir: Path) -> None:
        resolved = resolve_assets(None, asset_dir)
        assert resolved.mode is not None
        assert resolved.mode.kind == "bundled"
        assert resolved.esm == FAKE_MODULE
        assert resolved.wasm_path == asset_dir / "libsimlin-browser.wasm"
        assert resolved.error is None
        assert resolve_assets("bundled", asset_dir) == resolved

    def test_inline_embeds_the_wasm_before_the_module(self, asset_dir: Path) -> None:
        resolved = resolve_assets("inline", asset_dir)
        assert resolved.error is None
        assert resolved.esm is not None
        shim, module = resolved.esm.split("\n", 1)
        assert shim.startswith('globalThis.__simlinWidgetInlineWasm = "')
        assert shim.endswith('";')
        assert module == FAKE_MODULE
        assert resolved.wasm_path is not None  # the kernel still answers wasm requests

    def test_url_is_the_esm(self, asset_dir: Path) -> None:
        resolved = resolve_assets("https://cdn.example/widget.js", asset_dir)
        assert resolved.esm == "https://cdn.example/widget.js"
        assert resolved.wasm_path == asset_dir / "libsimlin-browser.wasm"

    def test_url_mode_without_local_assets_still_works(self, tmp_path: Path) -> None:
        resolved = resolve_assets("http://localhost:5173/widget.js", tmp_path / "nowhere")
        assert resolved.esm == "http://localhost:5173/widget.js"
        assert resolved.wasm_path is None
        assert resolved.error is None

    def test_missing_module_is_an_error_naming_the_file(self, asset_dir: Path) -> None:
        (asset_dir / "widget.js").unlink()
        resolved = resolve_assets(None, asset_dir)
        assert resolved.esm is None
        assert resolved.error is not None
        assert "widget.js" in resolved.error
        assert str(asset_dir) in resolved.error

    def test_inline_without_wasm_is_an_error_naming_the_wasm(self, asset_dir: Path) -> None:
        (asset_dir / "libsimlin-browser.wasm").unlink()
        resolved = resolve_assets("inline", asset_dir)
        assert resolved.esm is None
        assert resolved.error is not None
        assert "libsimlin-browser.wasm" in resolved.error

    def test_bad_mode_is_an_error_not_a_crash(self, asset_dir: Path) -> None:
        resolved = resolve_assets("serve", asset_dir)
        assert resolved.mode is None
        assert resolved.esm is None
        assert resolved.error is not None
        assert "SIMLIN_WIDGET_ASSET" in resolved.error

    def test_url_mode_from_the_environment_reaches_the_widget(
        self, monkeypatch: pytest.MonkeyPatch, asset_dir: Path, model: Model
    ) -> None:
        # ``_ASSETS`` is ``resolve_assets(os.environ.get(ASSET_ENV), <pkg dir>)``
        # evaluated once at import.  Reloading the module in a test would
        # re-create the ModelWidget class and make later isinstance checks
        # order-dependent, so the same expression is evaluated here with the
        # environment set and installed as the process-wide resolution.
        import simlin.widget as widget_module

        monkeypatch.setenv(widget_module.ASSET_ENV, "https://cdn.example/widget.js")
        resolved = resolve_assets(os.environ.get(widget_module.ASSET_ENV), asset_dir)
        monkeypatch.setattr("simlin.widget._ASSETS", resolved)
        w = model.widget()
        assert w._esm == "https://cdn.example/widget.js"
        _drain(w)
        _from_browser(w, {"type": "wasm"})  # the wasm still comes from the package dir
        assert _sent(w) == [("custom", {"type": "wasm"}, [FAKE_WASM])]

    def test_repo_checkout_without_built_assets_imports_fine(self) -> None:
        # Whatever the state of simlin/_widget/ on this machine, importing
        # the package must succeed and the resolution must be well-formed.
        import simlin.widget as widget_module

        resolved = widget_module._ASSETS
        assert (resolved.esm is None) == (resolved.error is not None)
        assert resolved.directory.name == "_widget"
