"""The interactive notebook editor: ``ModelWidget``.

pattern: Imperative Shell (traits, comm messages, Project subscriptions)

Displaying a :class:`~simlin.Model` in a JupyterLab / Notebook 7 / VS Code /
Colab cell shows the ``@simlin/diagram`` editor, hosted by anywidget.  The
kernel owns the model file; the browser owns interaction and undo history.
Every discrete edit in the browser arrives here as a whole-project snapshot
tagged with the revision it was edited from; the kernel accepts it through
:meth:`Project._apply_snapshot` (which writes the file) or rejects it as
stale, and every kernel-originated change (``edit()``, a reload from disk)
is pushed back so the browser remounts on it.  Protocol: Section 3 of
``docs/design-plans/2026-08-17-pysimlin-widget.md``; the pure decisions
live in :mod:`simlin._widget_core`, this module executes them.

Assets.  The JS module (``widget.js``) and the engine (``libsimlin-
browser.wasm``) ship as package data in ``simlin/_widget/``.  How they reach
the browser is chosen once, when this module is imported, from the
``SIMLIN_WIDGET_ASSET`` environment variable:

- unset / ``bundled`` (default): ``_esm`` is the module text; the browser
  asks for the wasm with a ``{type:'wasm'}`` message and receives the bytes
  as a binary comm buffer.
- ``inline``: ``_esm`` is the module text preceded by
  ``globalThis.__simlinWidgetInlineWasm = "<base64 wasm>";`` on its own
  line (the exact contract is :func:`simlin._widget_core.inline_esm`).  The
  JS may compile the engine from that global instead of asking the kernel;
  the kernel still answers ``{type:'wasm'}`` requests, so the global is an
  optimisation the JS is free to ignore.  Colab-safe; bloats every displayed
  widget's saved state by the encoded wasm.
- an ``http(s)://`` URL: ``_esm`` is that URL (a dev server or CDN serving
  the module); the wasm still goes over the comm.

Threading.  Comm messages are handled on the kernel's main thread.  Project
change notifications may originate on the poll thread; when a kernel IO
loop is present (``shell.kernel.io_loop``) they are marshalled onto it with
``add_callback`` -- the same thread the comm handlers run on -- so a widget
never touches its traits from two threads at once.  Without a kernel loop
(a script, tests) delivery is direct.  A widget never holds a Project lock
while sending; ``Project.on_change`` already guarantees callbacks run with
no lock held.
"""

from __future__ import annotations

import functools
import os
import sys
import warnings
import weakref
from dataclasses import dataclass
from importlib import resources
from pathlib import Path
from typing import TYPE_CHECKING, Any

import anywidget
import traitlets

from . import _widget_core as core
from ._widget_core import (
    ASSET_ENV,
    ASSET_PACKAGE_DIR,
    MAX_SNAPSHOT_BYTES,
    WASM_FILE,
    WIDGET_JS,
    AssetMode,
    MalformedSnapshot,
    OversizeReport,
    SnapshotOutcome,
    SnapshotRequest,
    Unrecognised,
    WasmRequest,
    dispatch_for_shell,
    is_own_change,
    notice_for_change,
    oversize_notice,
    oversize_report_warning,
    oversize_warning,
    parse_asset_mode,
    parse_incoming,
    plan_snapshot_reply,
    snapshot_wire_size,
)
from .errors import SimlinAssetError, SimlinError, SimlinWriteError

if TYPE_CHECKING:
    from collections.abc import Callable
    from types import FrameType

    from ._sync import ChangeEvent
    from .model import Model
    from .project import Project

    Dispatch = Callable[[Callable[[], None]], None]


# ── assets ──────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class WidgetAssets:
    """The resolved delivery of the widget's JS module and engine wasm.

    ``esm`` is the value for the widget's ``_esm`` trait, or ``None`` when
    it cannot be produced, in which case ``error`` says why (an actionable
    message naming the missing file or the bad ``SIMLIN_WIDGET_ASSET``).
    ``wasm_path`` is where the engine bytes are read from when a browser
    asks; ``None`` when the file is absent from ``directory`` (the request
    is then answered with an error message rather than a hang).
    """

    directory: Path
    mode: AssetMode | None
    esm: str | None
    error: str | None
    wasm_path: Path | None


def _asset_dir() -> Path:
    """``simlin/_widget/`` as an on-disk path.

    ``importlib.resources`` is the portable way to find package data; the
    package is a normal directory (a wheel or a checkout), never a zip, so
    the traversable is a real path we can hand to the ``open`` builtin.
    """
    return Path(str(resources.files(__package__) / ASSET_PACKAGE_DIR))


def resolve_assets(mode_value: str | None, directory: Path) -> WidgetAssets:
    """Turn ``SIMLIN_WIDGET_ASSET`` (``mode_value``) and the asset directory
    into a :class:`WidgetAssets`.  Never raises: a missing file or a bad
    mode becomes ``error`` so that importing this module always succeeds
    and the failure surfaces when a widget is created."""
    try:
        mode = parse_asset_mode(mode_value)
    except ValueError as exc:
        return WidgetAssets(directory, mode=None, esm=None, error=str(exc), wasm_path=None)
    wasm_candidate = directory / WASM_FILE
    wasm_path = wasm_candidate if wasm_candidate.is_file() else None
    if mode.kind == "url":
        return WidgetAssets(directory, mode=mode, esm=mode.url, error=None, wasm_path=wasm_path)
    js_path = directory / WIDGET_JS
    if not js_path.is_file():
        error = core.missing_asset_message(WIDGET_JS, str(directory))
        return WidgetAssets(directory, mode=mode, esm=None, error=error, wasm_path=wasm_path)
    module_text = js_path.read_text(encoding="utf-8")
    if mode.kind == "bundled":
        return WidgetAssets(directory, mode=mode, esm=module_text, error=None, wasm_path=wasm_path)
    if wasm_path is None:
        error = core.missing_asset_message(WASM_FILE, str(directory))
        return WidgetAssets(directory, mode=mode, esm=None, error=error, wasm_path=None)
    return WidgetAssets(
        directory,
        mode=mode,
        esm=core.inline_esm(module_text, wasm_path.read_bytes()),
        error=None,
        wasm_path=wasm_path,
    )


_ASSETS: WidgetAssets = resolve_assets(os.environ.get(ASSET_ENV), _asset_dir())
"""Chosen at import time, like rerun's ``RERUN_NOTEBOOK_ASSET``: every
widget created in this process delivers its assets the same way."""


@functools.cache
def _read_wasm(path: Path) -> bytes:
    """The engine bytes, read once per process; several megabytes that
    every displayed widget on a fresh page asks for."""
    return path.read_bytes()


def _kernel_dispatch() -> Dispatch | None:
    """The kernel IO-loop dispatcher for the current process, if any."""
    try:
        from IPython.core.getipython import get_ipython
    except ImportError:  # pragma: no cover - IPython is an anywidget dependency
        return None
    return dispatch_for_shell(get_ipython())  # type: ignore[no-untyped-call]


def _stacklevel_outside_package() -> int:
    """The ``warnings.warn(stacklevel=)`` that attributes a warning raised
    from inside pysimlin to the first caller frame outside the ``simlin``
    package (the user's cell), however deep the internal call chain
    (``Model._repr_mimebundle_`` -> ``Model.widget`` -> ``ModelWidget``
    -> ``_warn_if_oversize``).  Level 1 is the caller of ``warn``; every
    ``simlin.*`` frame between it and the outside adds one.  Python 3.12's
    ``skip_file_prefixes`` does the same and is not available on 3.11."""
    level = 1
    frame: FrameType | None = sys._getframe(1)  # the function about to call warnings.warn
    package_prefix = __package__ + "."
    while frame is not None:
        name = frame.f_globals.get("__name__", "")
        if name != __package__ and not name.startswith(package_prefix):
            return level
        level += 1
        frame = frame.f_back
    return level


# ── the widget ──────────────────────────────────────────────────────────


class ModelWidget(anywidget.AnyWidget):
    """Interactive diagram editor for one :class:`~simlin.Model`.

    Create one with :meth:`Model.widget` (or by displaying a model), then
    ``display`` it or return it from a cell.  Every edit made in the browser
    is applied to the model's project through the same path as
    ``Model.edit()`` -- written to the file, revision bumped, subscribers
    notified with ``source == "widget"`` -- so ``model.run()`` in the next
    cell reflects it.  Every kernel-originated change (a Python ``edit()``,
    an external write picked up from disk, ``reload()``) is pushed to the
    browser, which remounts its editor on the new contents (its undo history
    resets; a widget's own accepted edits never remount).  Several widgets
    on one project -- the same model displayed in two cells -- all stay in
    step, each through its own subscription.

    Traits (synced): ``project_json`` and ``revision`` are the kernel's
    authoritative snapshot and never written by the browser; ``selection``
    is written by the browser (mirrored to :attr:`Model.selection`);
    ``height`` (px), ``theme`` (``auto`` follows the notebook, else
    ``light``/``dark``) and ``read_only`` configure the editor;
    ``max_snapshot_bytes`` is the largest edit (the whole project as native
    JSON, in UTF-8 bytes) the browser will send back -- above it an edit is
    refused with a notice instead of being lost to the notebook server's
    websocket message limit (see ``simlin._widget_core.MAX_SNAPSHOT_BYTES``
    for the default and why).  Seeding or pushing a project above that size
    warns once per widget.

    Lifetime: like every ipywidget, a widget stays alive until
    :meth:`close` (ipywidgets keeps a registry of open widgets), and it
    keeps its model's project alive with it; ``close()`` unsubscribes from
    the project.
    """

    project_json = traitlets.Unicode("").tag(sync=True)
    revision = traitlets.Int(0).tag(sync=True)
    selection = traitlets.List(traitlets.Unicode()).tag(sync=True)
    height = traitlets.Int(600).tag(sync=True)
    theme = traitlets.Enum(("auto", "light", "dark"), default_value="auto").tag(sync=True)
    read_only = traitlets.Bool(False).tag(sync=True)
    max_snapshot_bytes = traitlets.Int(MAX_SNAPSHOT_BYTES, min=1).tag(sync=True)

    @traitlets.validate("max_snapshot_bytes")
    def _validate_max_snapshot_bytes(self, proposal: dict[str, Any]) -> int:
        # traitlets.Int accepts bool (a Python int subclass); True is never
        # a byte count, so refuse it explicitly rather than cap at 1 byte.
        value = proposal["value"]
        if isinstance(value, bool):
            raise traitlets.TraitError(
                f"max_snapshot_bytes must be a positive integer number of bytes, got {value!r}"
            )
        return int(value)

    # Declared explicitly (rather than as a class-level string, which
    # anywidget also accepts) so the module text is supplied per instance:
    # the process-wide choice lives in ``_ASSETS`` and tests can supply their
    # own without reloading the module.
    _esm = traitlets.Unicode("").tag(sync=True)

    def __init__(
        self,
        model: Model,
        *,
        height: int = 600,
        theme: str = "auto",
        read_only: bool = False,
        max_snapshot_bytes: int = MAX_SNAPSHOT_BYTES,
        dispatch: Dispatch | None = None,
        assets: WidgetAssets | None = None,
        **kwargs: Any,
    ) -> None:
        """Wrap ``model`` (which must belong to a :class:`Project`).

        ``max_snapshot_bytes`` caps the edits the browser sends back (see the
        class docstring); raise it only together with the notebook server's
        ``websocket_max_message_size``.  ``dispatch`` marshals project change
        notifications; by default the kernel's IO loop when running under
        ipykernel, else direct delivery.  ``assets`` overrides the
        process-wide asset resolution (tests; embedding hosts that supply
        their own build).

        Raises:
            SimlinAssetError: if the widget's JS module cannot be supplied
                (missing from the package, or a bad ``SIMLIN_WIDGET_ASSET``).
            SimlinRuntimeError: if ``model`` is not attached to a project.
        """
        resolved = _ASSETS if assets is None else assets
        if resolved.esm is None:
            raise SimlinAssetError(resolved.error or "widget assets unavailable")
        project = model._require_project()
        self._model = model
        self._project: Project = project
        self._assets = resolved
        self._dispatch = _kernel_dispatch() if dispatch is None else dispatch
        # The revision the widget's own in-flight snapshot will produce if
        # accepted; ``is_own_change`` uses it to skip re-pushing (and thereby
        # remounting) the browser's own edit.  Set before ``_apply_snapshot``
        # because, without a dispatcher, the notification fires inside it.
        self._own_revision: int | None = None
        self._unsubscribe: Callable[[], None] | None = None
        # The oversize warning is per widget, not per push: a Python edit()
        # loop on a too-large model would otherwise warn on every iteration.
        self._warned_oversize = False

        # The Editor mounts the model's first view and is a blank canvas
        # without one, so a diagram-less model (``Project.new()`` built up
        # through ``edit()``) is laid out first: a committed change exactly
        # like ``project.auto_layout()`` (revision bump, autosave when
        # file-backed), made BEFORE this widget subscribes so it is not
        # pushed back to itself.  Failing to lay out or to write the layout
        # must not stop the display: the seed goes out as it is (the JS side
        # mounts an empty view for a viewless model) and the warning says
        # what did not happen.
        try:
            project._ensure_view(model._name or "main")
        except Exception as exc:  # see above: nothing here may block a display
            warnings.warn(
                f"simlin: could not lay out (or save) a diagram for the editor: {exc}",
                RuntimeWarning,
                stacklevel=_stacklevel_outside_package(),
            )
        json_bytes, revision = project._snapshot()
        super().__init__(
            _esm=resolved.esm,
            project_json=json_bytes.decode("utf-8"),
            revision=revision,
            height=height,
            theme=theme,
            read_only=read_only,
            max_snapshot_bytes=max_snapshot_bytes,
            **kwargs,
        )
        self.on_msg(self._on_custom_msg)
        self._warn_if_oversize(json_bytes.decode("utf-8"))
        # A weak reference: the project holds its listeners strongly, and a
        # closed widget must not be kept alive by a project it no longer
        # watches (unsubscribing is the normal path; this is the backstop).
        ref = weakref.ref(self)

        def deliver(event: ChangeEvent) -> None:
            widget = ref()
            if widget is not None:
                widget._on_project_change(event)

        self._unsubscribe = project.on_change(deliver, dispatch=self._dispatch)

    # ── lifecycle ───────────────────────────────────────────────────

    @property
    def model(self) -> Model:
        """The model this widget edits."""
        return self._model

    def close(self) -> None:
        """Stop following the project and close the comm (idempotent)."""
        unsubscribe = getattr(self, "_unsubscribe", None)
        if unsubscribe is not None:
            self._unsubscribe = None
            unsubscribe()
        super().close()

    # ── kernel -> browser ───────────────────────────────────────────

    def _push(self, json_text: str, revision: int) -> None:
        """Assign the authoritative pair in one sync message, so the browser
        never observes a revision without its contents (or vice versa)."""
        with self.hold_sync():
            self.project_json = json_text
            self.revision = revision
        self._warn_if_oversize(json_text)

    def _warn_if_oversize(self, json_text: str) -> None:
        """Warn (once per widget) when the project the browser now holds is
        too large for the browser to send edits of it back -- measured as
        the browser will measure it, JSON-escaped (:func:`snapshot_wire_size`).
        The kernel -> browser direction has no such limit, so the model
        still displays; the warning is what tells the Python user why editor
        edits will be refused, and how to lift both limits.  Attributed to
        the first frame outside the ``simlin`` package -- the user's cell
        for a display, ``Model.widget()`` call or ``edit()`` block -- so the
        warning shows in that cell's output rather than pointing at
        ``model.py``."""
        if self._warned_oversize:
            return
        text = oversize_warning(snapshot_wire_size(json_text), int(self.max_snapshot_bytes))
        if text is None:
            return
        self._warned_oversize = True
        warnings.warn(text, RuntimeWarning, stacklevel=_stacklevel_outside_package())

    def _push_from_project(self) -> None:
        json_bytes, revision = self._project._snapshot()
        self._push(json_bytes.decode("utf-8"), revision)

    def _notice(self, text: str, level: core.NoticeLevel = "info") -> None:
        self.send(core.notice_message(text, level))

    def _on_project_change(self, event: ChangeEvent) -> None:
        """A revision bump from any source: skip the widget's own accepted
        snapshot (already pushed as the exact bytes it sent), otherwise push
        the project's current pair -- read at delivery time, not from the
        event, since a later change may already have landed -- and say
        where the change came from."""
        if is_own_change(event, self._own_revision):
            return
        self._push_from_project()
        text, level = notice_for_change(event.source)
        self._notice(text, level)

    # ── browser -> kernel ───────────────────────────────────────────

    def _on_custom_msg(self, _widget: object, content: object, _buffers: object) -> None:
        message = parse_incoming(content)
        match message:
            case WasmRequest():
                self._reply_wasm()
            case SnapshotRequest():
                self._handle_snapshot(message)
            case OversizeReport(bytes=nbytes):
                # The browser already resolved the save unsaved and showed a
                # toast; the kernel's part is the same notice plus a warning
                # on the kernel's stderr as the record (a comm handler runs
                # outside any cell, so JupyterLab shows it in the Log Console
                # rather than a cell output -- the toast is what the user
                # sees).
                limit = int(self.max_snapshot_bytes)
                warnings.warn(oversize_report_warning(nbytes, limit), RuntimeWarning, stacklevel=2)
                self.send(oversize_notice(nbytes, limit))
            case MalformedSnapshot(reason=reason):
                # Still owed its one reply: nothing was applied, so rejected
                # at the current revision, and the notice says what was wrong.
                warnings.warn(
                    f"simlin: ModelWidget rejected a malformed snapshot: {reason}",
                    RuntimeWarning,
                    stacklevel=2,
                )
                self.send(core.rejected_message(int(self._project.revision)))
                self._notice(f"Your edit could not be applied: {reason}.", "warn")
            case Unrecognised(reason=reason):
                warnings.warn(
                    f"simlin: ModelWidget ignored a message from the browser: {reason}",
                    RuntimeWarning,
                    stacklevel=2,
                )

    def _reply_wasm(self) -> None:
        path = self._assets.wasm_path
        if path is None:
            message = core.missing_asset_message(WASM_FILE, str(self._assets.directory))
            self.send(core.wasm_error_reply(message))
            return
        try:
            data = _read_wasm(path)
        except OSError as exc:
            self.send(core.wasm_error_reply(f"could not read {path}: {exc}"))
            return
        self.send(core.wasm_reply(), buffers=[data])

    def _handle_snapshot(self, request: SnapshotRequest) -> None:
        """Every snapshot gets exactly one reply.  The browser resolves its
        save only on ``saved``/``rejected`` and runs one save at a time, so
        a snapshot whose reply never comes hangs every later save of that
        view; hence the belt-and-braces: any exception anywhere in the
        handling -- including a bug in this class -- ends in ``rejected``."""
        try:
            self._apply_and_reply(request)
        except Exception as exc:  # the reply is owed regardless
            warnings.warn(
                f"simlin: ModelWidget failed while handling a snapshot: {exc!r}",
                RuntimeWarning,
                stacklevel=2,
            )
            self._own_revision = None
            self.send(core.rejected_message(int(self._project.revision)))
            self._notice(f"Your edit could not be applied: {exc}.", "warn")

    def _apply_and_reply(self, request: SnapshotRequest) -> None:
        project = self._project
        # The revision this snapshot produces if applied.  Set before the
        # call because, without a dispatcher, the project's notification
        # fires inside ``_apply_snapshot``; cleared below if nothing was
        # produced so someone else's change at that revision is not skipped.
        self._own_revision = request.base + 1
        error: str | None = None
        try:
            applied = project._apply_snapshot(request.json.encode("utf-8"), request.base)
        except SimlinWriteError as exc:  # applied in memory; only the file lags
            applied = True
            error = str(exc.__cause__ or exc)
            warnings.warn(
                f"simlin: a widget edit was applied but could not be written: {error}",
                RuntimeWarning,
                stacklevel=3,
            )
        except SimlinError as exc:  # the snapshot did not parse; nothing changed
            applied = False
            error = str(exc)
            warnings.warn(
                f"simlin: a widget edit could not be applied: {exc}", RuntimeWarning, stacklevel=3
            )
        if not applied:
            self._own_revision = None
        outcome = SnapshotOutcome(applied=applied, revision=int(project.revision), error=error)
        plan = plan_snapshot_reply(request, outcome)
        if plan.push is not None:
            # hold_sync exits (state message sent) before the reply goes out,
            # so the browser sees the state before ``saved``.
            self._push(*plan.push)
        for message in plan.messages:
            self.send(message)

    @traitlets.observe("selection")
    def _selection_changed(self, change: dict[str, Any]) -> None:
        self._model.selection = tuple(str(name) for name in change["new"])


__all__ = ["ModelWidget", "WidgetAssets", "resolve_assets"]
