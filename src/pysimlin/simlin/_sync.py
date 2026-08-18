"""Sync state machine for file-backed projects.

pattern: Functional Core (pure; no I/O, no threads, no FFI)

A file-backed :class:`~simlin.Project` has three parties that can change a
model: local Python code (``edit()``), an interactive widget (whole-project
snapshots tagged with the revision they were edited from), and the file on
disk (an external writer such as Claude Code, the ``simlin`` MCP server, or
``git checkout``).  The file on disk is the authority.  This module decides,
for every incoming event, what the imperative shell (``Project``, its poll
thread, and later the widget) must do; the shell only executes decisions.

The state is deliberately tiny -- a monotonic ``revision``, the content
hash of the bytes we know are on disk (for echo suppression), a ``dirty``
flag, and the set of disk contents we have already declined -- so every
event x state arm can be enumerated in the tests.

Decisions the shell executes:

- :class:`Write` / :class:`MarkDirty` -- an accepted local or widget change;
  the revision has been bumped.  ``Write`` means "serialize and write the
  file, then feed :class:`WriteCompleted`"; ``MarkDirty`` means the change
  stays in memory only.
- :class:`RejectStale` -- a widget snapshot edited from an older revision;
  the shell re-seeds the widget from the kernel's state and never writes
  the stale snapshot.
- :class:`IgnoreEcho` -- observed disk bytes are the bytes we last wrote or
  loaded, so nothing happened.
- :class:`AttemptReload` -- novel disk bytes; the shell parses them and
  feeds :class:`DiskParsed`.
- :class:`Reload` -- the parse succeeded and the revision has been bumped;
  the shell has already replaced the project contents and now notifies.
- :class:`KeepLastKnownGood` -- the disk bytes are unparsable, or arrived
  while unsaved local changes exist; the previous project stays.  ``reason``
  is ``"declined"`` when the same bytes were already refused (so the shell
  warns once per distinct content, not once per poll).
- :class:`NoChange` -- bookkeeping only.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Literal, Union

ChangeSource = Literal["edit", "widget", "disk", "reload"]
"""Who caused a revision bump: local ``edit()``, an accepted widget
snapshot, an external change picked up from disk, or an explicit
``reload()``."""


@dataclass(frozen=True)
class ChangeEvent:
    """Delivered to :meth:`Project.on_change` subscribers after every
    accepted change.  ``revision`` is the project's revision after the
    change."""

    source: ChangeSource
    revision: int


@dataclass(frozen=True)
class SyncState:
    """The pure sync state carried by a ``Project``.

    ``disk_hash`` is the content hash of the bytes we last wrote to or
    loaded from disk (``None`` for a project that has never touched disk).
    ``dirty`` is true while the in-memory project differs from ``disk_hash``.
    ``declined_hashes`` are disk contents we already refused to load and
    warned about, so the poll thread does not re-warn on every tick.
    """

    revision: int = 0
    disk_hash: str | None = None
    dirty: bool = False
    declined_hashes: frozenset[str] = frozenset()


# ── events ──────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class LocalEdit:
    """A local mutation (``edit()``, ``set_sim_specs()``, ``auto_layout()``)
    was accepted in memory.  ``persist`` is true when the project is
    file-backed and autosaving, i.e. the change should be written now."""

    persist: bool


@dataclass(frozen=True)
class WidgetSnapshot:
    """A widget sent a whole-project snapshot edited from ``base_revision``."""

    base_revision: int
    persist: bool


@dataclass(frozen=True)
class WriteCompleted:
    """Bytes with ``content_hash`` reached disk (autosave, ``save()``,
    ``save_as()``)."""

    content_hash: str


@dataclass(frozen=True)
class DiskObserved:
    """The poll thread read the file and it hashes to ``content_hash``."""

    content_hash: str


@dataclass(frozen=True)
class DiskParsed:
    """The shell tried to load observed disk bytes; ``ok`` says whether the
    project contents were replaced."""

    content_hash: str
    ok: bool


@dataclass(frozen=True)
class ExplicitReload:
    """The user called ``reload()``; the file currently hashes to
    ``content_hash``."""

    content_hash: str


SyncEvent = Union[
    LocalEdit,
    WidgetSnapshot,
    WriteCompleted,
    DiskObserved,
    DiskParsed,
    ExplicitReload,
]


# ── decisions ───────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Write:
    revision: int


@dataclass(frozen=True)
class MarkDirty:
    revision: int


@dataclass(frozen=True)
class RejectStale:
    revision: int


@dataclass(frozen=True)
class IgnoreEcho:
    pass


@dataclass(frozen=True)
class AttemptReload:
    pass


@dataclass(frozen=True)
class Reload:
    revision: int


KeepReason = Literal["unparsable", "dirty", "declined"]


@dataclass(frozen=True)
class KeepLastKnownGood:
    reason: KeepReason


@dataclass(frozen=True)
class NoChange:
    pass


SyncDecision = Union[
    Write,
    MarkDirty,
    RejectStale,
    IgnoreEcho,
    AttemptReload,
    Reload,
    KeepLastKnownGood,
    NoChange,
]


# ── transition ──────────────────────────────────────────────────────────


def _accept_change(state: SyncState, persist: bool) -> tuple[SyncDecision, SyncState]:
    """Shared arm for accepted local edits and accepted widget snapshots:
    bump the revision and decide whether a write follows.  ``dirty`` is set
    either way -- it is cleared by :class:`WriteCompleted`, so a failed
    autosave leaves the project correctly marked dirty."""
    new_state = replace(state, revision=state.revision + 1, dirty=True)
    if persist:
        return Write(new_state.revision), new_state
    return MarkDirty(new_state.revision), new_state


def decide(state: SyncState, event: SyncEvent) -> tuple[SyncDecision, SyncState]:
    """Return the decision for ``event`` and the state after it.

    Pure: callers own the state and thread the returned value back in.
    """
    match event:
        case LocalEdit(persist=persist):
            return _accept_change(state, persist)

        case WidgetSnapshot(base_revision=base, persist=persist):
            if base != state.revision:
                return RejectStale(state.revision), state
            return _accept_change(state, persist)

        case WriteCompleted(content_hash=h):
            return NoChange(), replace(state, disk_hash=h, dirty=False, declined_hashes=frozenset())

        case DiskObserved(content_hash=h):
            if h == state.disk_hash:
                return IgnoreEcho(), state
            if h in state.declined_hashes:
                return KeepLastKnownGood("declined"), state
            if state.dirty:
                return KeepLastKnownGood("dirty"), replace(
                    state, declined_hashes=state.declined_hashes | {h}
                )
            return AttemptReload(), state

        case DiskParsed(content_hash=h, ok=ok):
            if ok:
                new_state = SyncState(revision=state.revision + 1, disk_hash=h)
                return Reload(new_state.revision), new_state
            return KeepLastKnownGood("unparsable"), replace(
                state, declined_hashes=state.declined_hashes | {h}
            )

        case ExplicitReload(content_hash=h):
            if h == state.disk_hash and not state.dirty:
                return NoChange(), state
            return AttemptReload(), state

    raise TypeError(f"unknown sync event: {event!r}")


__all__ = [
    "AttemptReload",
    "ChangeEvent",
    "ChangeSource",
    "DiskObserved",
    "DiskParsed",
    "ExplicitReload",
    "IgnoreEcho",
    "KeepLastKnownGood",
    "KeepReason",
    "LocalEdit",
    "MarkDirty",
    "NoChange",
    "RejectStale",
    "Reload",
    "SyncDecision",
    "SyncEvent",
    "SyncState",
    "WidgetSnapshot",
    "Write",
    "WriteCompleted",
    "decide",
]
