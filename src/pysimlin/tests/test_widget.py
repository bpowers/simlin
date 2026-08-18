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
    snapshot  | base stale               -> rejected + warn notice, file untouched
    snapshot  | unparsable json          -> rejected + warn notice, project untouched
    snapshot  | write fails after apply  -> traits from project, rejected + warn
                                            notice, project dirty
    snapshot  | malformed message        -> RuntimeWarning, nothing sent
    other     | unknown type             -> RuntimeWarning, nothing sent

Change-source x delivery table (kernel -> browser):

    edit / disk / reload / other widget  -> traits pushed + notice
    own accepted snapshot                -> nothing further (no remount)
    with a dispatcher                    -> nothing until the dispatcher runs
"""

from __future__ import annotations

import gc
import json
import shutil
import weakref
from pathlib import Path
from typing import TYPE_CHECKING, Any

import comm
import pytest
import traitlets
from comm.base_comm import BaseComm

import simlin
from simlin import Aux, ChangeEvent, Model, Project, SimlinAssetError
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


def _snapshot_with(model_path: Path, name: str) -> str:
    """A snapshot the way a browser would produce one: the whole project as
    native JSON, edited by adding an aux -- built with the engine so it is
    exactly the kind of bytes the Editor sends."""
    scratch = simlin.load(model_path)
    with scratch.edit() as (_, patch):
        patch.upsert(Aux(name=name, equation="42"))
    return _project(scratch).serialize_json().decode("utf-8")


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
        scratch = Project.new()  # the browser's edited copy of the same project
        with scratch.get_model().edit() as (_, patch):
            patch.upsert(Aux(name="added", equation="1"))
        text = scratch.serialize_json().decode("utf-8")
        _from_browser(w, {"type": "snapshot", "base": 0, "json": text})
        assert model.revision == 1
        assert model.dirty is True
        assert model.get_variable("added") is not None
        assert _sent(w)[-1] == ("custom", {"type": "saved", "revision": 1}, None)


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
        # The traits already held revision 1 (pushed by the edit), so no
        # update goes out; the browser remounts from them.
        assert [kind for kind, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 1}
        notice = sent[1][1]
        assert notice["type"] == "notice"
        assert notice["level"] == "warn"
        assert "older version" in notice["text"]
        assert w.revision == 1

    def test_reject_reasserts_the_pair_when_its_notification_is_still_queued(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        # A kernel edit whose notification is queued behind the browser's
        # message (a dispatcher that has not run yet): the reject must not
        # leave the browser on revision 0 until the queue drains.
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
        assert sent[0][0] == "update"
        assert sent[0][1]["revision"] == 1
        assert sent[1][1] == {"type": "rejected", "revision": 1}
        assert sent[2][1]["type"] == "notice"
        assert w.revision == 1
        _drain(w)
        for fn in queue:
            fn()
        # The queued edit notification finds the traits already current:
        # only its notice goes out.
        assert _sent(w) == [
            ("custom", {"type": "notice", "text": "Updated from Python", "level": "info"}, None)
        ]

    def test_unparsable_snapshot_is_rejected_with_the_error(
        self, model: Model, model_path: Path, assets: WidgetAssets
    ) -> None:
        w = _widget(model, assets)
        before = model_path.read_bytes()
        _drain(w)
        with pytest.warns(RuntimeWarning, match="could not be saved"):
            _from_browser(w, {"type": "snapshot", "base": 0, "json": "{not json"})
        assert model.revision == 0
        assert model_path.read_bytes() == before
        sent = _sent(w)
        assert [kind for kind, _, _ in sent] == ["custom", "custom"]
        assert sent[0][1] == {"type": "rejected", "revision": 0}
        assert sent[1][1]["level"] == "warn"
        assert "could not be saved" in sent[1][1]["text"]

    def test_write_failure_after_apply_reports_and_leaves_project_dirty(
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
        # ...and the browser is told: pair re-asserted from the project
        # (exactly once -- the own notification did not push a second time),
        # then rejected and a warning naming the error.
        sent = _sent(w)
        assert [kind for kind, _, _ in sent] == ["update", "custom", "custom"]
        assert sent[0][1]["revision"] == 1
        assert json.loads(sent[0][1]["project_json"]) == json.loads(
            _project(model).serialize_json()
        )
        assert sent[1][1] == {"type": "rejected", "revision": 1}
        assert sent[2][1]["level"] == "warn"
        assert "disk full" in sent[2][1]["text"]
        assert "model.save()" in sent[2][1]["text"]
        # Recovery is the documented one.
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


class TestMalformed:
    @pytest.mark.parametrize(
        "content",
        [
            {"type": "snapshot", "base": "0", "json": "{}"},
            {"type": "snapshot", "base": 0},
            {"type": "saved", "revision": 1},
            "hello",
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
        # foreign (its notice goes out; the pair was already re-asserted).
        queue: list[Callable[[], None]] = []
        a = _widget(model, assets)
        b = _widget(model, assets, dispatch=queue.append)
        _from_browser(a, {"type": "snapshot", "base": 0, "json": _snapshot_with(model_path, "a")})
        assert len(queue) == 1
        _drain(b)
        _from_browser(b, {"type": "snapshot", "base": 0, "json": _snapshot_with(model_path, "b")})
        assert [c["type"] for k, c, _ in _sent(b) if k == "custom"] == ["rejected", "notice"]
        assert b.revision == 1
        _drain(b)
        for fn in queue:
            fn()
        assert _sent(b) == [
            (
                "custom",
                {"type": "notice", "text": "Updated in another view", "level": "info"},
                None,
            )
        ]

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
        with pytest.raises(SimlinAssetError):
            model._repr_mimebundle_()
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

    def test_module_level_resolution_reads_the_environment(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        import importlib

        import simlin.widget as widget_module

        monkeypatch.setenv("SIMLIN_WIDGET_ASSET", "https://cdn.example/widget.js")
        try:
            reloaded = importlib.reload(widget_module)
            assert reloaded._ASSETS.esm == "https://cdn.example/widget.js"
        finally:
            monkeypatch.delenv("SIMLIN_WIDGET_ASSET")
            importlib.reload(widget_module)

    def test_repo_checkout_without_built_assets_imports_fine(self) -> None:
        # Whatever the state of simlin/_widget/ on this machine, importing
        # the package must succeed and the resolution must be well-formed.
        import simlin.widget as widget_module

        resolved = widget_module._ASSETS
        assert (resolved.esm is None) == (resolved.error is not None)
        assert resolved.directory.name == "_widget"
