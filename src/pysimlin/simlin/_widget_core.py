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
(``wasm``, ``snapshot``, ``saved``, ``rejected``, ``notice``).
"""

from __future__ import annotations

import base64
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Literal, Union

if TYPE_CHECKING:
    from collections.abc import Callable

    from ._sync import ChangeEvent, ChangeSource

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
        f"checkout build it with `pnpm --filter @simlin/notebook-widget build`, which "
        f"copies {WIDGET_JS} and {WASM_FILE} into simlin/{ASSET_PACKAGE_DIR}/. "
        f"Set {ASSET_ENV}=<http(s) url> to load the module from a dev server instead."
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
class Unrecognised:
    """A message the kernel does not understand; ``reason`` says why."""

    reason: str


IncomingMessage = Union[WasmRequest, SnapshotRequest, Unrecognised]


def parse_incoming(content: object) -> IncomingMessage:
    """Classify a custom message from the browser.

    A malformed message is never an exception in the kernel: the browser is
    the untrusted side of this protocol, so a bad ``snapshot`` (missing or
    mistyped ``base``/``json``) becomes :class:`Unrecognised`, which the
    shell reports and otherwise ignores.  ``bool`` is rejected as a base
    even though it is an ``int`` in Python: ``true`` is never a revision.
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
            return Unrecognised(f"snapshot 'base' must be an integer revision, got {base!r}")
        if not isinstance(json_text, str):
            return Unrecognised(
                f"snapshot 'json' must be the project as a JSON string, got "
                f"{type(json_text).__name__}"
            )
        return SnapshotRequest(base=base, json=json_text)
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
    after applying (a write failure: the change is real in memory, the
    revision advanced by one, ``dirty`` is set) -- ``False`` for a stale
    base or a parse failure, which leave the project untouched.  ``error``
    is the exception text when it raised.  ``revision`` is the project's
    revision afterwards.
    """

    applied: bool
    revision: int
    error: str | None = None


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
    - not applied (stale base, or the snapshot did not parse): the traits
      are NOT touched (they already hold the authoritative state; a
      notification still queued behind this message pushes it), then
      ``rejected`` plus a warning notice.
    """
    if outcome.applied:
        accepted_revision = request.base + 1
        messages: list[dict[str, Any]] = [saved_message(accepted_revision)]
        if outcome.error is not None:
            messages.append(
                notice_message(
                    f"Your edit was applied but could not be written to the file: "
                    f"{outcome.error}. The model is marked dirty; call model.save() to "
                    f"retry the write.",
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


def is_own_change(event: ChangeEvent, own_revision: int | None) -> bool:
    """Whether a project change notification is this widget's own accepted
    snapshot (which the widget already pushed itself, so re-pushing would
    remount the browser's editor and lose its undo history).

    Only ``widget``-sourced events can be ours; a disk reload or a Python
    ``edit()`` that happens to land at the remembered revision is impossible
    because revisions are unique per project, but the source check keeps
    the rule readable.
    """
    return event.source == "widget" and own_revision is not None and event.revision == own_revision


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
    "WASM_FILE",
    "WIDGET_JS",
    "AssetKind",
    "AssetMode",
    "IncomingMessage",
    "NoticeLevel",
    "SnapshotOutcome",
    "SnapshotPlan",
    "SnapshotRequest",
    "Unrecognised",
    "WasmRequest",
    "dispatch_for_shell",
    "inline_esm",
    "is_own_change",
    "missing_asset_message",
    "notice_for_change",
    "notice_message",
    "parse_asset_mode",
    "parse_incoming",
    "plan_snapshot_reply",
    "rejected_message",
    "saved_message",
    "wasm_error_reply",
    "wasm_reply",
]
