"""Decisions behind the notebook editor widget.

pattern: Functional Core (pure; no I/O, no traits, no comm)

:class:`simlin.widget.ModelWidget` is the imperative shell that talks to the
browser over the ipywidgets comm and to a :class:`~simlin.Project`.  Every
decision it makes -- how the JS module and engine wasm are delivered
(``SIMLIN_WIDGET_ASSET``), what an incoming comm message means, what to
reply and push after a snapshot, whether a change notification is the
widget's own accepted snapshot, and what a kernel-originated change should
tell the human -- is a pure function here, so each arm can be pinned by a
test without a widget, a comm, or a kernel.

The wire protocol these functions implement is Section 3 of
``docs/design-plans/2026-08-17-pysimlin-widget.md``: state travels as
traits (``project_json``/``revision`` kernel-owned, ``selection``
widget-owned), and every request or reply is a custom message
(``wasm``, ``snapshot``, ``oversize``, ``saved``, ``rejected``, ``notice``).
"""

from __future__ import annotations

import base64
import importlib
import json
import sys
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Literal, Union

if TYPE_CHECKING:
    from collections.abc import Callable, Collection
    from types import FrameType, ModuleType

    from ._sync import ChangeEvent, ChangeSource

# ── the optional extra ──────────────────────────────────────────────────

NOTEBOOK_EXTRA = "pysimlin[notebook]"
"""The extra that installs the notebook editor's dependencies (anywidget,
and through it ipywidgets/traitlets/IPython).  Optional because that chain
is a couple of dozen packages that scripts, MCP servers, and CI installs
which never display a model should not carry."""


def install_hint() -> str:
    """The one-line install command for the running host: Colab wants the
    ``%pip`` magic (a plain ``pip`` there installs into the wrong
    interpreter often enough that Colab documents the magic), everywhere
    else plain ``pip``.  Colab is detected the way anywidget detects it
    (``anywidget._util.in_colab``): ``"google.colab.output"`` is already in
    ``sys.modules`` in a Colab kernel by the time user code runs.  Nothing is
    imported to find out -- an import attempt would be a side effect in a
    function that runs while an error message is being built."""
    if "google.colab.output" in sys.modules:
        return f'%pip install "{NOTEBOOK_EXTRA}"'
    return f'pip install "{NOTEBOOK_EXTRA}"'


def user_stacklevel() -> int:
    """The ``warnings.warn(stacklevel=)`` that attributes a warning raised
    from inside pysimlin to the user's own code -- in a notebook, the cell
    -- however deep the call chain above it: every frame of the ``simlin``
    package is skipped (``Model._repr_mimebundle_`` -> ``Model.widget`` ->
    ``ModelWidget`` -> ``_warn_if_oversize``), and so is every frame of
    IPython/ipykernel between the cell and us: a bare display reaches
    ``_repr_mimebundle_`` through IPython's display formatter, which is one
    source location for every display, and a warning attributed there would
    be shown once per kernel session by Python's once-per-location filter
    and point at ``formatters.py``.  Attributed to the cell it shows in that
    cell's output, and re-running the very same cell is what the filter
    dedupes (ipykernel names a cell's code object by a hash of its source).
    Level 1 is the caller of ``warn``; every skipped frame adds one; when
    every frame is skipped (a warning raised from a thread with no user
    code on its stack) the outermost frame is named.  Python 3.12's
    ``skip_file_prefixes`` does the package half of this and is not
    available on 3.11."""
    level = 1
    frame: FrameType | None = sys._getframe(1)  # the function about to call warnings.warn
    while frame is not None:
        if not _is_library_frame(frame.f_globals.get("__name__", "")):
            return level
        level += 1
        frame = frame.f_back
    return level - 1


_LIBRARY_PACKAGES = (__package__, "IPython", "ipykernel")


def _is_library_frame(module_name: str) -> bool:
    return any(
        module_name == package or module_name.startswith(package + ".")
        for package in _LIBRARY_PACKAGES
    )


def missing_dependency_message() -> str:
    return (
        f"the notebook editor needs the optional {NOTEBOOK_EXTRA!r} extra "
        f"(anywidget); install it with: {install_hint()}"
    )


def import_widget_module() -> ModuleType:
    """``simlin.widget``, imported on demand.  Raises
    :class:`~simlin.errors.SimlinDependencyError` (an ``ImportError``)
    naming the install line when anywidget is not installed."""
    from .errors import SimlinDependencyError

    try:
        return importlib.import_module(f"{__package__}.widget")
    except ImportError as exc:
        raise SimlinDependencyError(missing_dependency_message()) from exc


# ── asset delivery ──────────────────────────────────────────────────────

ASSET_ENV = "SIMLIN_WIDGET_ASSET"
"""Environment variable choosing how the widget's JS and wasm reach the
browser; read once when :mod:`simlin.widget` is imported."""

WIDGET_JS = "widget.js"
WASM_FILE = "libsimlin-browser.wasm"
ASSET_PACKAGE_DIR = "_widget"
"""The package-data directory (``simlin/_widget/``) both assets live in."""

INLINE_WASM_GLOBAL = "__simlinWidgetInlineWasm"
"""JS global the ``inline`` asset mode defines; see :func:`inline_esm`."""

AssetKind = Literal["bundled", "inline", "url"]


@dataclass(frozen=True)
class AssetMode:
    """How the JS module (and, for ``inline``, the wasm) is delivered.

    ``bundled`` (the default): ``_esm`` is the text of ``widget.js`` and the
    wasm goes over the comm on request.  ``inline``: the wasm is base64-
    embedded in ``_esm`` (Colab-safe when the comm cannot carry binary
    buffers; bloats every displayed widget's saved state).  ``url``:
    ``_esm`` is an ``http(s)`` URL the front-end loads the module from (a
    dev server or CDN); the wasm still goes over the comm.
    """

    kind: AssetKind
    url: str | None = None


def parse_asset_mode(value: str | None) -> AssetMode:
    """Interpret ``SIMLIN_WIDGET_ASSET``.

    Unset or empty means ``bundled``.  Anything other than ``bundled``,
    ``inline`` or an ``http(s)://`` URL is a configuration error, raised as
    ``ValueError`` naming the variable so the shell can turn it into an
    actionable message rather than silently falling back.
    """
    text = (value or "").strip()
    if text in ("", "bundled"):
        return AssetMode("bundled")
    if text == "inline":
        return AssetMode("inline")
    if text.startswith(("http://", "https://")):
        return AssetMode("url", url=text)
    raise ValueError(
        f"{ASSET_ENV}={value!r} is not understood; use 'bundled' (default), 'inline', "
        f"or an http(s) URL of the widget module"
    )


def inline_esm(module_text: str, wasm: bytes) -> str:
    """Build the ``inline`` mode ``_esm``: the wasm as a base64 JS global,
    then the module text.

    Contract with the JS module (``src/notebook-widget``): the statement
    ``globalThis.__simlinWidgetInlineWasm = "<base64>";`` precedes the module
    text on its own line, so when the module runs it may read that global,
    ``atob`` it into bytes, and compile the engine from them instead of
    asking the kernel.  The global is optional for the JS side: the kernel
    answers ``{type:'wasm'}`` requests in every mode, so a module that
    ignores the global still works, and one that consumes it should prefer
    it (Colab may not deliver binary comm buffers).  The value is a plain
    string, not a data: URL; the JS is free to leave or delete the global.
    """
    payload = base64.b64encode(wasm).decode("ascii")
    return f'globalThis.{INLINE_WASM_GLOBAL} = "{payload}";\n{module_text}'


def missing_asset_message(name: str, directory: str) -> str:
    """The actionable text for a widget asset that is not in the package."""
    return (
        f"the notebook widget asset {name!r} is missing from {directory}. In a released "
        f"pysimlin wheel this means the install is broken (reinstall pysimlin); in a source "
        f"checkout run `make assets` in src/pysimlin (or "
        f"`pnpm --filter @simlin/notebook-widget build`), which builds the widget and stages "
        f"the assets into simlin/{ASSET_PACKAGE_DIR}/ via scripts/stage_widget_assets.py "
        f"(see simlin/{ASSET_PACKAGE_DIR}/README.md). Set {ASSET_ENV}=<http(s) url> to load "
        f"the module from a dev server instead."
    )


# ── snapshot size ───────────────────────────────────────────────────────

TORNADO_DEFAULT_MAX_MESSAGE_SIZE = 10 * 1024 * 1024
"""tornado's default ``websocket_max_message_size`` (``tornado.websocket.
_default_max_message_size``, 10 MiB), which jupyter_server inherits for the
kernel websocket unless ``--ServerApp.tornado_settings`` overrides it.  A
browser -> server frame above it makes tornado close the socket (code 1009,
"message too big") -- the message never reaches the kernel."""

MAX_SNAPSHOT_BYTES = 8 * 1024 * 1024
"""Default largest project snapshot the browser will send to the kernel,
measured AS IT RIDES IN THE MESSAGE: the UTF-8 bytes of the snapshot text
JSON-string-escaped (:func:`snapshot_wire_size`), which is how it sits in
the comm envelope ``{"method":"custom","content":{"type":"snapshot",
"base":n,"json":"..."}}``.  Measuring the raw text instead would under-count
by 7-39% depending on content (quotes and backslashes in equations and
names each cost an extra byte), and an 8 MiB raw snapshot could be over 11
MiB on the wire and still hang.  The widget's ``max_snapshot_bytes`` trait
carries the value the JS enforces, and the seed/push warning fires above
the same number.  8 MiB, not 10: the envelope's other fields plus the
jupyter message header are well under 2 KiB, so 8 MiB leaves ~2 MiB of
headroom under tornado's default regardless of content.  A model above
this cannot be edited from the widget: the browser reports ``oversize``
instead of hanging on a snapshot the server would drop.  Users who raise
the server limit may raise the widget's too,
``model.widget(max_snapshot_bytes=...)``.  Must equal ``MAX_SNAPSHOT_BYTES``
in ``src/notebook-widget/src/widget-core.ts`` (the JS default when the
trait is missing)."""


def snapshot_wire_size(text: str) -> int:
    """UTF-8 byte length of ``text`` as a JSON string value -- what the
    snapshot contributes to its websocket frame.  ``ensure_ascii=False``
    because ipykernel's serializer emits UTF-8, not ``\\uXXXX`` escapes;
    the JS side (``JSON.stringify`` + ``TextEncoder``) counts the same
    bytes."""
    return len(json.dumps(text, ensure_ascii=False).encode("utf-8"))


def format_size(nbytes: int) -> str:
    """``nbytes`` as a short human figure: KiB below 1 MiB, else MiB to one
    decimal (whole numbers print without it: ``8 MiB``, ``12.3 MiB``,
    ``512 KiB``).  Rounding is on integer tenths (``round`` on an integer
    numerator, so a tie like 0.25 MiB rounds to even: ``0.2 MiB`` never
    occurs since that range prints in KiB, and 8.25 MiB prints ``8.2 MiB``
    on both sides); the JS ``formatSize`` produces byte-identical output
    for the fixture list pinned in both test suites."""
    kib = nbytes / 1024
    if nbytes < 1024 * 1024:
        return f"{round(kib)} KiB"
    tenths = round(nbytes * 10 / (1024 * 1024))
    if tenths % 10 == 0:
        return f"{tenths // 10} MiB"
    return f"{tenths // 10}.{tenths % 10} MiB"


def oversize_warning(nbytes: int, limit: int) -> str | None:
    """The kernel-side warning for a project whose snapshot (``nbytes``, wire
    size) exceeds ``limit``: ``None`` when it does not.  Issued when the
    widget seeds or pushes such a project, because the browser side will
    refuse to send edits of it back (see :func:`oversize_notice`); the text
    says why, what still works, and how to raise both limits."""
    if nbytes <= limit:
        return None
    return (
        f"simlin: this model is {format_size(nbytes)} as a snapshot (JSON-escaped, as it "
        f"travels), above the {format_size(limit)} the notebook editor will send back to the "
        f"kernel, so edits made in the displayed editor will not be saved (they are refused "
        f"with a notice); the model still displays, and edits from Python (model.edit()) work "
        f"as usual. The cap exists because a Jupyter server drops browser->kernel websocket "
        f"messages above tornado's websocket_max_message_size "
        f"({format_size(TORNADO_DEFAULT_MAX_MESSAGE_SIZE)} by default; any host that reaches "
        f"the kernel through a Jupyter server, such as JupyterLab or Notebook 7, is subject to "
        f"it) by closing the connection, which would leave the editor waiting forever. To edit "
        f"models this large in the editor, raise both: start the server with a larger limit, "
        f"e.g. jupyter lab --ServerApp.tornado_settings="
        f"'{{\"websocket_max_message_size\": 104857600}}', and display with "
        f"model.widget(max_snapshot_bytes=<bytes>) below roughly 80% of that."
    )


def oversize_report_warning(nbytes: int, limit: int) -> str:
    """The kernel-side warning for a browser ``oversize`` report.  It goes
    to the kernel's stderr as a ``RuntimeWarning`` (JupyterLab shows it in
    the Log Console, not in a cell -- the toast is what the user sees); it
    is the record for someone reading the kernel log."""
    return (
        f"simlin: an edit made in the notebook editor was not saved: the model's snapshot is "
        f"{format_size(nbytes)} (JSON-escaped, as it travels), above the widget's "
        f"max_snapshot_bytes ({format_size(limit)}). Edit the model from Python instead, or "
        f"raise the limit together with the notebook server's websocket_max_message_size "
        f"(see simlin._widget_core.MAX_SNAPSHOT_BYTES)."
    )


def oversize_notice(nbytes: int, limit: int) -> dict[str, Any]:
    """The warn notice the kernel sends back for a browser ``oversize``
    report; the same wording as the toast the browser shows itself, so the
    two collapse into one visible message."""
    return notice_message(
        f"Edit not saved: the model is too large for the notebook connection "
        f"({format_size(nbytes)} > {format_size(limit)} limit); edit it from Python instead.",
        "warn",
    )


# ── comm messages ───────────────────────────────────────────────────────

NoticeLevel = Literal["info", "warn"]


@dataclass(frozen=True)
class WasmRequest:
    """``{type:'wasm'}``: the browser wants the engine bytes."""


@dataclass(frozen=True)
class SnapshotRequest:
    """``{type:'snapshot', base, json}``: the browser edited the project from
    revision ``base`` and offers the whole project as native JSON."""

    base: int
    json: str


@dataclass(frozen=True)
class OversizeReport:
    """``{type:'oversize', bytes}``: the browser refused to send a snapshot of
    ``bytes`` (its JSON-escaped UTF-8 size, as it would ride in the message)
    because it exceeds ``max_snapshot_bytes`` (the
    save was resolved unsaved and a toast shown).  Nothing was applied and
    no ``saved``/``rejected`` reply is owed; the kernel warns and sends the
    matching notice."""

    bytes: int


@dataclass(frozen=True)
class MalformedSnapshot:
    """A ``{type:'snapshot'}`` whose ``base``/``json`` are missing or
    mistyped.  Still a snapshot as far as the browser is concerned: it is
    waiting for its one reply, so the shell answers ``rejected`` (nothing
    was applied) plus a notice with ``reason``."""

    reason: str


@dataclass(frozen=True)
class Unrecognised:
    """A message the kernel does not understand at all (not a JSON object,
    or an unknown ``type``); ``reason`` says why.  Nothing is owed a reply."""

    reason: str


IncomingMessage = Union[
    WasmRequest, SnapshotRequest, OversizeReport, MalformedSnapshot, Unrecognised
]


def parse_incoming(content: object) -> IncomingMessage:
    """Classify a custom message from the browser.

    A malformed message is never an exception in the kernel: the browser is
    the untrusted side of this protocol.  A ``snapshot`` with a bad
    ``base``/``json`` is :class:`MalformedSnapshot` -- it gets the reply
    every snapshot is owed -- and anything else unintelligible is
    :class:`Unrecognised`, which the shell reports and otherwise ignores.
    ``bool`` is rejected as a base even though it is an ``int`` in Python:
    ``true`` is never a revision.  An ``oversize`` report with a bad
    ``bytes`` is :class:`Unrecognised`: it is owed no reply.
    """
    if not isinstance(content, dict):
        return Unrecognised(f"expected a JSON object, got {type(content).__name__}")
    kind = content.get("type")
    if kind == "wasm":
        return WasmRequest()
    if kind == "snapshot":
        base = content.get("base")
        json_text = content.get("json")
        if isinstance(base, bool) or not isinstance(base, int):
            return MalformedSnapshot(f"snapshot 'base' must be an integer revision, got {base!r}")
        if not isinstance(json_text, str):
            return MalformedSnapshot(
                f"snapshot 'json' must be the project as a JSON string, got "
                f"{type(json_text).__name__}"
            )
        return SnapshotRequest(base=base, json=json_text)
    if kind == "oversize":
        nbytes = content.get("bytes")
        if isinstance(nbytes, bool) or not isinstance(nbytes, int) or nbytes < 0:
            return Unrecognised(f"oversize 'bytes' must be a non-negative integer, got {nbytes!r}")
        return OversizeReport(bytes=nbytes)
    return Unrecognised(f"unknown message type {kind!r}")


def wasm_reply() -> dict[str, Any]:
    """The reply whose first binary buffer carries the wasm."""
    return {"type": "wasm"}


def wasm_error_reply(error: str) -> dict[str, Any]:
    return {"type": "wasm", "error": error}


def saved_message(revision: int) -> dict[str, Any]:
    return {"type": "saved", "revision": revision}


def rejected_message(revision: int) -> dict[str, Any]:
    return {"type": "rejected", "revision": revision}


def notice_message(text: str, level: NoticeLevel = "info") -> dict[str, Any]:
    return {"type": "notice", "text": text, "level": level}


# ── snapshot outcome ────────────────────────────────────────────────────


@dataclass(frozen=True)
class SnapshotOutcome:
    """What the shell observed after handing a snapshot to the project.

    ``applied`` says whether the project took the change: ``True`` when
    :meth:`Project._apply_snapshot` returned ``True``, AND when it raised
    after applying (``SimlinWriteError``: the change is real in memory and
    the revision advanced by one) -- ``False`` for a stale base or a parse
    failure, which leave the project untouched.  ``error`` is the exception
    text when it raised; with ``applied``, ``write_failed`` says whether
    that was the autosave write (``dirty`` is set and ``model.save()``
    retries) rather than a later step after the file was written.
    ``revision`` is the project's revision afterwards.
    """

    applied: bool
    revision: int
    error: str | None = None
    write_failed: bool = False


@dataclass(frozen=True)
class SnapshotPlan:
    """What the shell must do in reply to a snapshot, in order: push the
    trait pair (``None`` = do not touch the traits), then send ``messages``
    -- exactly one of which is the ``saved``/``rejected`` reply."""

    push: tuple[str, int] | None
    messages: tuple[dict[str, Any], ...]


def plan_snapshot_reply(request: SnapshotRequest, outcome: SnapshotOutcome) -> SnapshotPlan:
    """Decide the reply to a snapshot.  Every arm sends exactly one reply
    (``saved`` or ``rejected``): the browser resolves its save on the reply
    and runs one save at a time, so a snapshot without its reply would hang
    every later save of that view.

    Arms (each pinned by ``tests/test_widget.py``):

    - applied, no error: the traits become ``(the exact JSON received,
      base + 1)`` -- the same bytes so the browser can recognise its own
      snapshot by string equality and keep its live editor -- then
      ``saved``.  An accept bumps the revision by exactly one under the
      project's lock, so the pair is derived from the request rather than
      read back: a concurrent change landing after the accept is announced
      by its own notification.
    - applied, write failed: the change is real (revision base + 1, dirty),
      so the browser is told the same ``saved`` -- its acknowledged version
      must match the kernel's or every later save would be rejected as
      stale -- plus a warning notice naming the error and ``model.save()``.
    - applied, a later step failed (the file was written; notifying
      subscribers raised): the same ``saved``, plus a warning notice that
      names the error but does not send the user to ``model.save()`` --
      there is nothing to retry.
    - not applied (stale base, or the snapshot did not parse): the traits
      are NOT touched (they already hold the authoritative state; a
      notification still queued behind this message pushes it), then
      ``rejected`` plus a warning notice.
    """
    if outcome.applied:
        accepted_revision = request.base + 1
        messages: list[dict[str, Any]] = [saved_message(accepted_revision)]
        if outcome.error is not None and outcome.write_failed:
            messages.append(
                notice_message(
                    f"Your edit was applied but could not be written to the file: "
                    f"{outcome.error}. The model is marked dirty; call model.save() to "
                    f"retry the write.",
                    "warn",
                )
            )
        elif outcome.error is not None:
            messages.append(
                notice_message(
                    f"Your edit was applied, but the kernel could not finish handling it: "
                    f"{outcome.error}. Other views of this model may be out of date until "
                    f"the next change.",
                    "warn",
                )
            )
        return SnapshotPlan(push=(request.json, accepted_revision), messages=tuple(messages))
    if outcome.error is not None:
        text = f"Your edit could not be applied: {outcome.error}."
    else:
        text = (
            f"Your edit was based on an older version of the model (revision "
            f"{request.base}; the model is at {outcome.revision}); the editor was "
            f"reloaded from the current model."
        )
    return SnapshotPlan(
        push=None,
        messages=(rejected_message(outcome.revision), notice_message(text, "warn")),
    )


# ── change notifications ────────────────────────────────────────────────


def is_own_change(event: ChangeEvent, own_revisions: Collection[int]) -> bool:
    """Whether a project change notification is one of this widget's own
    accepted snapshots (which the widget already pushed itself, so
    re-pushing would remount the browser's editor and lose its undo
    history).

    ``own_revisions`` holds every revision this widget's accepted snapshots
    produced whose notification has not been delivered yet -- a set, not
    one slot, because a kernel that handles comm messages while a cell runs
    (ipykernel 7 subshells) can accept several snapshots before the IO loop
    drains their notifications, and each of them is ours.  Only
    ``widget``-sourced events can be ours; a disk reload or a Python
    ``edit()`` that happens to land at a remembered revision is impossible
    because revisions are unique per project, but the source check keeps
    the rule readable.
    """
    return event.source == "widget" and event.revision in own_revisions


def notice_for_change(source: ChangeSource) -> tuple[str, NoticeLevel]:
    """The toast for a kernel-originated change the browser is about to be
    remounted on.  Every source has a text (the arm table is the
    ``ChangeSource`` literal); external sources say so explicitly."""
    match source:
        case "disk":
            return ("Updated on disk", "info")
        case "reload":
            return ("Reloaded from disk", "info")
        case "edit":
            return ("Updated from Python", "info")
        case "widget":
            return ("Updated in another view", "info")
    raise ValueError(f"unknown change source {source!r}")


# ── kernel dispatch ─────────────────────────────────────────────────────


def dispatch_for_shell(shell: object) -> Callable[[Callable[[], None]], None] | None:
    """The thread-safe "run this on the kernel's IO loop" callable for an
    IPython shell, or ``None`` when there is no kernel loop (plain IPython,
    tests, a script).

    ipykernel exposes the loop as ``shell.kernel.io_loop`` (a tornado
    ``IOLoop`` whose ``add_callback`` is thread-safe).  Duck-typed on
    purpose: pysimlin does not depend on ipykernel, and the widget must
    degrade to direct delivery anywhere the chain is missing.
    """
    kernel = getattr(shell, "kernel", None)
    loop = getattr(kernel, "io_loop", None)
    add_callback = getattr(loop, "add_callback", None)
    if callable(add_callback):
        return add_callback  # type: ignore[no-any-return]
    return None


__all__ = [
    "ASSET_ENV",
    "ASSET_PACKAGE_DIR",
    "INLINE_WASM_GLOBAL",
    "MAX_SNAPSHOT_BYTES",
    "NOTEBOOK_EXTRA",
    "TORNADO_DEFAULT_MAX_MESSAGE_SIZE",
    "WASM_FILE",
    "WIDGET_JS",
    "AssetKind",
    "AssetMode",
    "IncomingMessage",
    "MalformedSnapshot",
    "NoticeLevel",
    "OversizeReport",
    "SnapshotOutcome",
    "SnapshotPlan",
    "SnapshotRequest",
    "Unrecognised",
    "WasmRequest",
    "dispatch_for_shell",
    "format_size",
    "import_widget_module",
    "inline_esm",
    "install_hint",
    "is_own_change",
    "missing_asset_message",
    "missing_dependency_message",
    "notice_for_change",
    "notice_message",
    "oversize_notice",
    "oversize_report_warning",
    "oversize_warning",
    "parse_asset_mode",
    "parse_incoming",
    "plan_snapshot_reply",
    "rejected_message",
    "saved_message",
    "snapshot_wire_size",
    "user_stacklevel",
    "wasm_error_reply",
    "wasm_reply",
]
