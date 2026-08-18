"""Project class for loading, editing, and saving system dynamics models.

pattern: Imperative Shell

A ``Project`` wraps the engine handle and, when opened from or saved to a
file, owns that file: it serializes in the file's own format, writes
atomically, tracks a monotonic ``revision``, and (optionally) polls the file
so external writers -- Claude Code, the ``simlin`` MCP server, ``git
checkout`` -- are picked up.  What each disk/edit/widget event *means* is
decided by the pure state machine in ``simlin._sync``; this class executes
those decisions.

Thread-safety: each ``Project`` owns two locks.  ``_lock`` (a plain
``Lock``) protects ``_ptr`` and the FFI calls.
``_file_lock`` (an ``RLock``) protects the file-backing state -- path,
format, sync state, listeners, models -- and serialises whole
mutate-then-write sequences so a poll-thread reload cannot interleave with
an ``edit()`` commit.  Lock order is ``_file_lock`` -> ``_lock`` ->
``Model._lock``; a ``Model`` method must release its own lock before calling
any ``Project`` method, and nothing may take ``_file_lock`` while holding
``_lock``.  Change callbacks always fire with no lock held.
"""

from __future__ import annotations

import dataclasses
import json
import threading
import warnings
import weakref
from pathlib import Path
from typing import TYPE_CHECKING, Any, Self, Union

from . import _sync
from ._disk import FileWatcher, atomic_write, content_hash
from ._dt import validate_dt
from ._ffi import (
    _register_finalizer,
    c_to_string,
    check_out_error,
    extract_error_details,
    ffi,
    free_c_string,
    lib,
    string_to_c,
)
from ._ffi import (
    apply_patch_json as _ffi_apply_patch_json,
)
from ._ffi import (
    diagram_sync as _ffi_diagram_sync,
)
from ._ffi import (
    open_json as _ffi_open_json,
)
from ._ffi import (
    render_png as _ffi_render_png,
)
from ._ffi import (
    render_svg as _ffi_render_svg,
)
from ._ffi import (
    replace_contents as _ffi_replace_contents,
)
from ._ffi import (
    serialize_json as _ffi_serialize_json,
)
from ._ffi import (
    serialize_mdl as _ffi_serialize_mdl,
)
from ._formats import FileFormat, resolve_read_format, resolve_write_format
from ._sync import ChangeEvent, SyncState
from .errors import (
    ErrorDetail,
    SimlinError,
    SimlinImportError,
    SimlinRuntimeError,
    SimlinWriteError,
)
from .json_converter import converter
from .json_types import (
    JsonProjectPatch,
    SetSimSpecs,
)
from .json_types import (
    Model as JsonModel,
)
from .json_types import (
    Project as JsonProject,
)
from .json_types import (
    SimSpecs as JsonSimSpecs,
)

if TYPE_CHECKING:
    from collections.abc import Callable
    from types import TracebackType

    from .model import Model

    ChangeCallback = Callable[[ChangeEvent], None]
    Dispatch = Callable[[Callable[[], None]], None]

_PathLike = Union[str, Path]

# JSON format constants
JSON_FORMAT_SIMLIN = "simlin"
JSON_FORMAT_SDAI = "sd-ai"


def _collect_error_details(err_ptr: Any) -> list[ErrorDetail]:
    """Convert a C SimlinError pointer into Python ErrorDetail objects.

    Note: This function does NOT free the C memory. The caller is responsible
    for calling simlin_error_free() on the original pointer.
    """
    return extract_error_details(err_ptr)


# Patch operations that add, remove, or rename variables.  Only these make a
# diagram stale (a new variable needs a view element; a deleted one must lose
# its element), so only they trigger the incremental layout after an edit --
# the same rule simlin-mcp-core's edit_model applies (``has_variable_ops``).
# View ops are the caller placing elements explicitly; sim-spec and loop-name
# ops do not touch structure.
_VARIABLE_OP_TYPES = frozenset(
    {
        "upsertStock",
        "upsertFlow",
        "upsertAux",
        "upsertModule",
        "deleteVariable",
        "renameVariable",
        "updateStockFlows",
    }
)


def _models_with_variable_ops(patch_json: bytes) -> list[str]:
    """Names of the models in an engine patch that gain/lose/rename variables.

    Returns ``[]`` for a patch that is not JSON or has no model ops -- the
    engine reports malformed patches itself, and this only decides whether a
    layout pass is needed afterwards.
    """
    try:
        patch = json.loads(patch_json)
    except (ValueError, UnicodeDecodeError):
        return []
    if not isinstance(patch, dict):
        return []
    names: dict[str, None] = {}
    for model_patch in patch.get("models") or []:
        if not isinstance(model_patch, dict):
            continue
        ops = model_patch.get("ops") or []
        if any(isinstance(op, dict) and op.get("type") in _VARIABLE_OP_TYPES for op in ops):
            # The engine treats "" and "main" as the same default model, but
            # the FFI requires a non-empty name.  A patch may legally carry
            # several entries for one model; lay it out once.
            names[model_patch.get("name") or "main"] = None
    return list(names)


def _read_model_file(path: Path) -> bytes:
    """Read a model file for :func:`simlin.load` / :meth:`Project.open`,
    turning the two ways a path can be wrong into ``SimlinImportError``."""
    if not path.exists():
        raise SimlinImportError(f"File not found: {path}")
    if not path.is_file():
        raise SimlinImportError(f"{path} is not a file")
    return path.read_bytes()


def _pretty_json(compact: bytes) -> bytes:
    """Re-indent engine JSON for on-disk files: line-oriented output diffs
    well in git, matching what simlin-serve and simlin-mcp-core write."""
    return json.dumps(json.loads(compact), indent=2, ensure_ascii=False).encode("utf-8")


def _disk_handler(project_ref: weakref.ref[Project]) -> Callable[[FileWatcher, bytes, str], bool]:
    """Build the poll-thread callback for a project.

    The watcher thread must never keep the project alive, so the handler
    holds only a weak reference and asks the watcher to stop (returns
    ``False``) once the project has been collected.  (The project also
    stops its watcher from a ``weakref.finalize``, so an idle file does not
    leave the thread polling until the next change either.)
    """

    def handler(watcher: FileWatcher, data: bytes, digest: str) -> bool:
        project = project_ref()
        if project is None:
            return False
        project._ingest_disk_bytes(watcher, data, digest)
        return True

    return handler


class Project:
    """Represents a simulation project containing one or more models.

    A project is the top-level container for system dynamics models.
    It can be loaded from various formats (XMILE, Vensim MDL, JSON,
    protobuf) and provides access to models and analysis functions.

    A project opened with :meth:`open` (or given a path by :meth:`save_as`)
    is *file-backed*: it remembers its :attr:`path` and :attr:`format`,
    every accepted change bumps :attr:`revision`, and with
    :attr:`autosave` on (the default), each change is written straight back
    to the file, so :attr:`dirty` only becomes true when such a write fails
    (and clears once :meth:`save` succeeds).  A save never overwrites a
    change another tool made to the file since we last read or wrote it;
    see :meth:`save`.
    :meth:`watch` polls the file so changes made by other tools are loaded
    in place -- existing :class:`Model` handles keep working and see the
    new contents.  Subscribe with :meth:`on_change` to be told about every
    accepted change and where it came from.

    Thread-safety: individual instances are safe to use from multiple
    threads.  All public methods acquire the internal locks described in
    the module docstring before touching ``_ptr`` or file state.
    """

    def __init__(
        self,
        ptr: Any,
        *,
        path: _PathLike | None = None,
        format: FileFormat | None = None,
        autosave: bool = True,
        disk_hash: str | None = None,
    ) -> None:
        """Initialize a Project from a C pointer.

        ``path``/``format``/``disk_hash`` describe the file the pointer was
        loaded from (all ``None`` for an in-memory project); use
        :meth:`open` rather than passing them by hand.
        """
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Project from NULL pointer")
        if (path is None) != (format is None):
            raise ValueError("path and format must be given together")
        self._lock = threading.Lock()
        self._ptr = ptr
        _register_finalizer(self, lib.simlin_project_unref, ptr)

        # File-backing state, all guarded by _file_lock (see module docstring).
        self._file_lock = threading.RLock()
        self._path: Path | None = Path(path) if path is not None else None
        self._format: FileFormat | None = format
        self._autosave = bool(autosave)
        self._sync = SyncState(disk_hash=disk_hash)
        self._listeners: dict[int, tuple[ChangeCallback, Dispatch | None]] = {}
        self._next_listener_id = 0
        self._watcher: FileWatcher | None = None
        self._closed = False
        # MDL lossiness messages already reported for this project (to_mdl).
        self._mdl_warned: set[str] = set()
        # Model handles created by get_model(); their caches are invalidated
        # on every accepted change.  Weak so a project never pins its models.
        self._models: weakref.WeakSet[Model] = weakref.WeakSet()

    def _check_alive(self) -> None:
        """Raise if the underlying C object has been freed.

        Must be called while ``_lock`` is held.
        """
        if self._ptr == ffi.NULL:
            raise SimlinRuntimeError("Project has been closed")

    # ── construction from bytes / files ─────────────────────────────────

    @staticmethod
    def _open_bytes(data: bytes, format: FileFormat) -> Any:
        """Parse ``data`` in ``format`` into a fresh engine project pointer.

        The single reader dispatch: :func:`simlin.load`, :meth:`open`,
        :meth:`reload`, and the poll thread all come through here.
        """
        c_data = ffi.new("uint8_t[]", data)
        err_ptr = ffi.new("SimlinError **")
        match format:
            case FileFormat.XMILE:
                ptr = lib.simlin_project_open_xmile(c_data, len(data), err_ptr)
            case FileFormat.MDL:
                ptr = lib.simlin_project_open_vensim(c_data, len(data), err_ptr)
            case FileFormat.NATIVE_JSON:
                ptr = lib.simlin_project_open_json(
                    c_data, len(data), lib.SIMLIN_JSON_FORMAT_NATIVE, err_ptr
                )
            case FileFormat.SDAI_JSON:
                ptr = lib.simlin_project_open_json(
                    c_data, len(data), lib.SIMLIN_JSON_FORMAT_SDAI, err_ptr
                )
            case FileFormat.PROTOBUF:
                ptr = lib.simlin_project_open_protobuf(c_data, len(data), err_ptr)
        check_out_error(err_ptr, f"Open {format.name} project")
        if ptr == ffi.NULL:
            raise SimlinImportError(f"Open {format.name} project returned no project")
        return ptr

    @classmethod
    def _from_bytes(cls, data: bytes, format: FileFormat) -> Project:
        """An in-memory project parsed from ``data`` in ``format``."""
        return cls(cls._open_bytes(data, format))

    @classmethod
    def open(
        cls,
        path: _PathLike,
        *,
        autosave: bool = True,
        watch: bool = True,
    ) -> Project:
        """Open a model file as a file-backed project.

        The format is taken from the suffix (``.stmx``/``.xmile``/``.xml``
        XMILE, ``.mdl`` Vensim, ``.sd.json``/``.json`` JSON sniffed for
        native vs SD-AI, ``.pb`` protobuf) or, for an unknown suffix, from
        the contents.  Saves regenerate the file in that same format.

        Args:
            path: The model file.
            autosave: Write every accepted change back to ``path``
                immediately.  With ``False`` changes stay in memory
                (``dirty`` becomes true) until :meth:`save`.
            watch: Poll ``path`` (every 0.5 s) and load external changes in
                place; see :meth:`watch`.

        Raises:
            SimlinImportError: if the file does not exist or its format
                cannot be determined.
            SimlinRuntimeError: if the engine cannot parse the file.
        """
        p = Path(path)
        data = _read_model_file(p)
        fmt = resolve_read_format(p, data)
        project = cls(
            cls._open_bytes(data, fmt),
            path=p,
            format=fmt,
            autosave=autosave,
            disk_hash=content_hash(data),
        )
        if watch:
            project.watch(True)
        return project

    # ── file-backing surface ────────────────────────────────────────────

    @property
    def path(self) -> Path | None:
        """The file this project is backed by, or ``None`` for an
        in-memory project (:meth:`new`, :func:`simlin.load`)."""
        with self._file_lock:
            return self._path

    @property
    def format(self) -> FileFormat | None:
        """The on-disk format saves are written in (``None`` in memory)."""
        with self._file_lock:
            return self._format

    @property
    def revision(self) -> int:
        """Monotonic per-process counter, ``0`` at open; ``+1`` for every
        accepted change from any source (``edit()``, a widget, an external
        write picked up from disk, an explicit :meth:`reload`)."""
        with self._file_lock:
            return self._sync.revision

    @property
    def dirty(self) -> bool:
        """``True`` while the in-memory project differs from what is on
        disk (an unsaved change, or an autosave that failed)."""
        with self._file_lock:
            return self._sync.dirty

    @property
    def autosave(self) -> bool:
        """Whether accepted changes are written to :attr:`path` immediately."""
        with self._file_lock:
            return self._autosave

    @autosave.setter
    def autosave(self, enabled: bool) -> None:
        with self._file_lock:
            self._autosave = bool(enabled)

    @property
    def watching(self) -> bool:
        """Whether the poll thread is running for :attr:`path`."""
        with self._file_lock:
            return self._watcher is not None and self._watcher.running

    def save(self, *, force: bool = False) -> None:
        """Write the project to :attr:`path` in :attr:`format`, atomically.

        A save never silently overwrites someone else's work: if the file
        on disk no longer holds the bytes this project last loaded or wrote
        (another tool changed it and, because of unsaved local changes, the
        change was held back), ``save()`` raises and leaves both the file
        and the in-memory project as they are.  Resolve it with
        :meth:`reload` (take the on-disk version, discarding local changes)
        or ``save(force=True)`` (overwrite the file with the local version).

        Raises:
            SimlinRuntimeError: if the project has no path (use
                :meth:`save_as`), or the file changed on disk and ``force``
                is not set.
            OSError: if the write fails; the in-memory project is kept and
                :attr:`dirty` is unchanged.
        """
        with self._file_lock:
            if self._path is None or self._format is None:
                raise SimlinRuntimeError(
                    "project has no file path; use save_as(path) to choose one"
                )
            self._write_to(self._path, self._format, force=force)

    def save_as(self, path: _PathLike, format: FileFormat | None = None) -> None:
        """Write the project to ``path`` and adopt it as :attr:`path`.

        The format comes from ``format`` or, if omitted, the suffix of
        ``path``.  If the project was watching its previous file, it now
        watches the new one.

        Saving to a *different* path never conflicts with changes to the
        current file; ``save_as`` to the current path behaves like
        :meth:`save`.

        Raises:
            ValueError: if the suffix is unknown and no ``format`` is given.
            SimlinRuntimeError: if the suffix is read-only (``.vpm``,
                ``.proto``) and no ``format`` is given.
            OSError: if the write fails; nothing is adopted.
        """
        target = Path(path)
        fmt = resolve_write_format(target, format)
        old_watcher: FileWatcher | None = None
        with self._file_lock:
            self._write_to(target, fmt)
            self._path = target
            self._format = fmt
            if self._watcher is not None:
                old_watcher = self._watcher
                self._watcher = self._start_watcher(target, old_watcher.interval)
        self._retire_watcher(old_watcher)

    def reload(self) -> bool:
        """Re-read :attr:`path` and replace the project contents in place.

        Existing :class:`Model` handles stay valid and see the new
        contents; their cached ``base_case`` is dropped.  Idempotent: when
        the file still holds the bytes we last wrote or loaded (and there
        are no unsaved local changes), nothing happens and ``False`` is
        returned.  With unsaved local changes, ``reload()`` discards them in
        favour of the file.

        Returns:
            ``True`` if the contents changed (``revision`` advanced).

        Raises:
            SimlinRuntimeError: if the project has no path, or the file no
                longer parses (the previous contents are kept).
            OSError: if the file cannot be read (e.g. it was deleted).
        """
        with self._file_lock:
            path = self._path
            if path is None:
                raise SimlinRuntimeError("project has no file path to reload from")
            data = path.read_bytes()
            digest = content_hash(data)
            decision, self._sync = _sync.decide(self._sync, _sync.ExplicitReload(digest))
            if isinstance(decision, _sync.NoChange):
                return False
            revision = self._load_disk_bytes(path, data, digest)
        # revision is None only when parsing failed, and that raised above.
        assert revision is not None
        self._notify(ChangeEvent("reload", revision))
        return True

    def watch(self, enabled: bool = True, interval: float = 0.5) -> None:
        """Start (or stop) polling :attr:`path` for external changes.

        A change made by another writer is loaded in place within about
        ``interval`` seconds: :attr:`revision` advances, model caches are
        invalidated, and :meth:`on_change` subscribers are told with
        ``source == "disk"``.  Our own writes are recognised by content hash
        and never round-trip as external changes.  A file rewritten with
        unparsable contents is NOT loaded: the last-known-good project stays,
        a ``RuntimeWarning`` names the error (once per distinct content), and
        the next valid write is picked up.  While unsaved local changes exist
        (:attr:`dirty`), external changes are likewise held back with a
        warning; resolve with :meth:`reload` (take the file's version) or
        ``save(force=True)`` (overwrite it) -- a plain :meth:`save` refuses.

        Polling is a stdlib daemon thread; see ``simlin._disk.FileWatcher``
        for why polling rather than inotify/``watchfiles``.

        Raises:
            SimlinRuntimeError: if enabling on a project with no path.
        """
        old_watcher: FileWatcher | None = None
        with self._file_lock:
            if enabled:
                if self._closed:
                    raise SimlinRuntimeError("cannot watch a closed project")
                if self._path is None:
                    raise SimlinRuntimeError(
                        "cannot watch an in-memory project; save_as(path) first"
                    )
                current = self._watcher
                if (
                    current is not None
                    and current.running
                    and current.path == self._path
                    and current.interval == interval
                ):
                    return
                old_watcher = current
                self._watcher = self._start_watcher(self._path, interval)
            else:
                old_watcher = self._watcher
                self._watcher = None
        self._retire_watcher(old_watcher)

    def _start_watcher(self, path: Path, interval: float) -> FileWatcher:
        watcher = FileWatcher(path, _disk_handler(weakref.ref(self)), interval=interval)
        # The handler only runs when the file changes, so it alone would let
        # the thread outlive a collected project on an idle file; the
        # finalizer asks the thread to exit as soon as the project is gone
        # (request_stop, not stop: never join from inside garbage collection).
        watcher.finalizer = weakref.finalize(self, watcher.request_stop)
        watcher.start()
        return watcher

    @staticmethod
    def _retire_watcher(watcher: FileWatcher | None) -> None:
        """Stop a watcher this project no longer uses and drop the GC
        finalizer that would otherwise reference it until the project dies.
        Must be called with ``_file_lock`` released: ``stop()`` joins the
        poll thread, which may be blocked on that lock."""
        if watcher is None:
            return
        finalizer = watcher.finalizer
        if finalizer is not None:
            finalizer.detach()
        watcher.stop()

    def on_change(
        self,
        callback: ChangeCallback,
        *,
        dispatch: Dispatch | None = None,
    ) -> Callable[[], None]:
        """Subscribe to accepted changes.

        ``callback(event)`` receives a :class:`ChangeEvent` after every
        revision bump, from whichever thread caused it (the caller's thread
        for ``edit()``/``reload()``, the poll thread for disk changes).  Pass
        ``dispatch`` to marshal the call elsewhere -- e.g. a Jupyter
        kernel's IO loop, ``dispatch=loop.call_soon_threadsafe`` -- in which
        case ``dispatch(fn)`` is invoked with a zero-argument callable.
        Callbacks never run under a project lock, and an exception in one is
        reported as a ``RuntimeWarning`` rather than propagating into the
        edit or poll that triggered it.

        Because delivery happens after the lock is released, two changes
        racing on different threads may deliver their events out of
        revision order.  ``event.revision`` says which change an event is
        about; a listener that wants the project's state should read the
        current :attr:`revision` / contents at delivery time rather than
        assume the event describes the latest state.

        Returns:
            A zero-argument function that unsubscribes.
        """
        with self._file_lock:
            token = self._next_listener_id
            self._next_listener_id += 1
            self._listeners[token] = (callback, dispatch)

        def unsubscribe() -> None:
            with self._file_lock:
                self._listeners.pop(token, None)

        return unsubscribe

    def _notify(self, event: ChangeEvent) -> None:
        """Deliver ``event`` to every subscriber, outside all locks."""
        with self._file_lock:
            listeners = list(self._listeners.values())
        for callback, dispatch in listeners:

            def deliver(callback: ChangeCallback = callback) -> None:
                try:
                    callback(event)
                except Exception as exc:  # a listener must not break the edit
                    warnings.warn(
                        f"simlin: on_change callback {callback!r} raised: {exc!r}",
                        RuntimeWarning,
                        stacklevel=2,
                    )

            if dispatch is None:
                deliver()
            else:
                dispatch(deliver)

    # ── internal: writing ───────────────────────────────────────────────

    def _serialize(self, format: FileFormat) -> bytes:
        """The bytes for this project in ``format`` -- the single writer
        dispatch shared by :meth:`save`, :meth:`save_as`, and autosave."""
        match format:
            case FileFormat.XMILE:
                return self.to_xmile()
            case FileFormat.MDL:
                return self.to_mdl()
            case FileFormat.NATIVE_JSON:
                return _pretty_json(self.serialize_json())
            case FileFormat.SDAI_JSON:
                return _pretty_json(self.serialize_json(JSON_FORMAT_SDAI))
            case FileFormat.PROTOBUF:
                return self.serialize_protobuf()

    def _disk_conflict(self, path: Path) -> bool:
        """Whether ``path`` (our own file) no longer holds the bytes we last
        loaded or wrote.  A missing file is not a conflict: re-creating it
        loses nothing.  Caller holds ``_file_lock``."""
        known = self._sync.disk_hash
        if known is None:
            return False
        try:
            current = path.read_bytes()
        except FileNotFoundError:
            return False
        return content_hash(current) != known

    def _write_to(self, path: Path, format: FileFormat, *, force: bool = False) -> None:
        """Serialize and atomically write; caller holds ``_file_lock``.

        Writing to our own :attr:`path` first checks that nobody else has
        changed the file since we last read or wrote it (see :meth:`save`);
        ``force`` skips that check.  On success the sync state records the
        written bytes' hash so the poll thread treats them as our echo.  On
        failure the exception propagates and the sync state is untouched:
        ``dirty`` already says whether memory differs from the file.
        """
        if not force and path == self._path and self._disk_conflict(path):
            raise SimlinRuntimeError(
                f"{path} changed on disk since it was loaded or last saved; call "
                f"reload() to take the on-disk version (discarding local changes) or "
                f"save(force=True) to overwrite it"
            )
        data = self._serialize(format)
        atomic_write(path, data)
        _, self._sync = _sync.decide(self._sync, _sync.WriteCompleted(content_hash(data)))

    def _snapshot(self) -> tuple[bytes, int]:
        """The current native-JSON contents and the revision they belong to,
        read together under ``_file_lock`` so a change on another thread
        cannot land between the two reads and split the pair.  This is what
        a widget seeds and re-seeds the browser from."""
        with self._file_lock:
            return self.serialize_json(), self._sync.revision

    # ── internal: accepting changes ─────────────────────────────────────

    def _register_model(self, model: Model) -> None:
        with self._file_lock:
            self._models.add(model)

    def _invalidate_model_caches(self) -> None:
        """Drop every live model's cached run results after the project
        changed.  Takes each ``Model._lock`` briefly; callers hold
        ``_file_lock`` (never ``_lock``), which respects the lock order."""
        with self._file_lock:
            models = list(self._models)
        for model in models:
            model._invalidate_caches()

    def _sync_diagram_for_patch(self, patch_json: bytes) -> None:
        """Give newly created variables diagram elements after an edit.

        Runs the engine's incremental layout for every model whose patch
        adds, deletes, or renames variables: existing element positions are
        preserved, new elements are placed, removed ones dropped.  A model
        with no diagram yet gets a full layout.  Layout failure is not an
        edit failure -- the model data is already correct -- so it is
        reported as a ``RuntimeWarning`` and the edit stands.
        """
        for model_name in _models_with_variable_ops(patch_json):
            try:
                with self._lock:
                    self._check_alive()
                    _ffi_diagram_sync(self._ptr, model_name, patch_json)
            except SimlinRuntimeError as exc:
                warnings.warn(
                    f"simlin: diagram layout for model {model_name!r} failed after "
                    f"the edit was applied; the diagram may be missing new "
                    f"variables: {exc}",
                    RuntimeWarning,
                    stacklevel=3,
                )

    def _commit_change_locked(self) -> tuple[int, BaseException | None]:
        """Bookkeeping after an in-memory mutation was accepted, with
        ``_file_lock`` held by the caller: bump the revision, invalidate model
        caches, and autosave if configured.  Returns the new revision and the
        autosave failure (if any) so the caller can release the lock, notify
        subscribers of the in-memory change, and only then raise -- a failed
        autosave still bumps and notifies because the in-memory change is
        real; :attr:`dirty` stays ``True`` until a save succeeds.
        """
        persist = self._path is not None and self._autosave
        decision, self._sync = _sync.decide(self._sync, _sync.LocalEdit(persist))
        assert isinstance(decision, _sync.Write | _sync.MarkDirty)
        self._invalidate_model_caches()
        write_error = self._try_autosave() if isinstance(decision, _sync.Write) else None
        return decision.revision, write_error

    def _try_autosave(self) -> BaseException | None:
        """Autosave to the current path; caller holds ``_file_lock`` and has
        just accepted a change (so ``_path``/``_format`` are set).  Returns
        the failure instead of raising so the caller can notify subscribers
        of the in-memory change before propagating it."""
        assert self._path is not None
        assert self._format is not None
        try:
            self._write_to(self._path, self._format)
        except Exception as exc:
            return exc
        return None

    def _apply_snapshot(self, data: bytes, base_revision: int) -> bool:
        """Accept a whole-project native-JSON snapshot from a widget edited
        at ``base_revision``.

        Returns ``False`` (and changes nothing) when ``base_revision`` is
        not the current revision: the widget must be re-seeded from the
        kernel's state and the stale snapshot is never written.  Otherwise
        the contents are replaced in place, the revision bumps, autosave
        writes, and subscribers see ``source == "widget"``.

        Raises:
            SimlinRuntimeError: if the snapshot does not parse; the project
                and revision are unchanged.
            SimlinWriteError: if the snapshot was applied (revision bumped,
                subscribers notified, ``dirty`` set) but the autosave write
                failed; ``__cause__`` is the underlying error.  Distinct from
                the parse failure so the caller can tell the browser its
                edit stands and only the file lags.
        """
        write_error: BaseException | None = None
        with self._file_lock:
            persist = self._path is not None and self._autosave
            decision, new_state = _sync.decide(
                self._sync, _sync.WidgetSnapshot(base_revision, persist)
            )
            if isinstance(decision, _sync.RejectStale):
                return False
            assert isinstance(decision, _sync.Write | _sync.MarkDirty)
            self._replace_from_bytes(data, FileFormat.NATIVE_JSON)
            self._sync = new_state
            revision = decision.revision
            self._invalidate_model_caches()
            if isinstance(decision, _sync.Write):
                write_error = self._try_autosave()
        self._notify(ChangeEvent("widget", revision))
        if write_error is not None:
            raise SimlinWriteError(
                f"snapshot applied (revision {revision}) but not written: {write_error}",
                revision,
            ) from write_error
        return True

    # ── internal: loading from disk ─────────────────────────────────────

    def _replace_from_bytes(self, data: bytes, format: FileFormat) -> None:
        """Replace this project's contents in place with ``data``.

        The bytes are parsed into a temporary project first, so a parse
        failure raises and leaves this project untouched.  Existing
        ``Model`` handles (which address the project by pointer + model
        name) stay valid across the swap.
        """
        replacement = Project._from_bytes(data, format)
        with self._lock:
            self._check_alive()
            with replacement._lock:
                _ffi_replace_contents(self._ptr, replacement._ptr)

    def _load_disk_bytes(self, path: Path, data: bytes, digest: str) -> int | None:
        """Shared tail of :meth:`reload` and the poll thread after the sync
        machine said ``AttemptReload``: parse, swap in place, record the
        outcome.  Caller holds ``_file_lock``.

        Returns the new revision on success.  On a parse failure the state
        records the declined hash and the exception propagates.
        """
        try:
            fmt = resolve_read_format(path, data)
            self._replace_from_bytes(data, fmt)
        except SimlinError:
            _, self._sync = _sync.decide(self._sync, _sync.DiskParsed(digest, ok=False))
            raise
        decision, self._sync = _sync.decide(self._sync, _sync.DiskParsed(digest, ok=True))
        assert isinstance(decision, _sync.Reload)
        # The write format follows the file: a .json rewritten as SD-AI
        # keeps being saved as SD-AI.
        self._format = fmt
        self._invalidate_model_caches()
        return decision.revision

    def _ingest_disk_bytes(self, watcher: FileWatcher, data: bytes, digest: str) -> None:
        """Poll-thread entry: ``watcher`` read ``data`` from the file.
        Executes the sync machine's decision; never raises (the watcher
        would only warn anyway, and these messages are more specific).

        The delivery is re-verified under ``_file_lock`` before it counts:
        it must come from the project's current watcher for its current
        path (``save_as`` retires watchers), and the file must still hold
        ``data``.  Between the watcher's read and this point our own
        ``edit()`` may have written the file; loading the watcher's older
        bytes over that write would silently revert it, so a delivery that
        no longer matches the file is dropped -- the watcher sees the newer
        write on its next tick.
        """
        revision: int | None = None
        with self._file_lock:
            path = self._path
            if path is None or watcher is not self._watcher or watcher.path != path:
                return
            try:
                if content_hash(path.read_bytes()) != digest:
                    return
            except OSError:
                return
            decision, self._sync = _sync.decide(self._sync, _sync.DiskObserved(digest))
            match decision:
                case _sync.IgnoreEcho():
                    return
                case _sync.KeepLastKnownGood(reason="dirty"):
                    warnings.warn(
                        f"simlin: {path} changed on disk but this project has unsaved "
                        f"local changes; keeping the in-memory model. Call reload() to "
                        f"take the on-disk version (discarding the local changes) or "
                        f"save(force=True) to overwrite the file with them.",
                        RuntimeWarning,
                        stacklevel=2,
                    )
                    return
                case _sync.KeepLastKnownGood():
                    return
                case _sync.AttemptReload():
                    try:
                        revision = self._load_disk_bytes(path, data, digest)
                    except SimlinError as exc:
                        warnings.warn(
                            f"simlin: {path} changed on disk but could not be loaded; "
                            f"keeping the previous model: {exc}",
                            RuntimeWarning,
                            stacklevel=2,
                        )
                        return
                case _:
                    raise AssertionError(f"unexpected sync decision {decision!r}")
        assert revision is not None
        self._notify(ChangeEvent("disk", revision))

    @classmethod
    def new(
        cls,
        *,
        name: str = "simlin project",
        sim_start: float = 0.0,
        sim_stop: float = 10.0,
        dt: float = 1.0,
        time_units: str = "",
    ) -> Project:
        """Create a new, empty project using default simulation settings.

        Args:
            name: Project name recorded in the metadata.
            sim_start: Simulation start time.
            sim_stop: Simulation stop time.
            dt: Simulation time step (Euler method by default).
            time_units: Optional time unit label.

        Returns:
            A new Project instance ready for editing.
        """
        sim_specs = JsonSimSpecs(
            start_time=float(sim_start),
            end_time=float(sim_stop),
            dt=str(dt),
            method="euler",
            time_units=time_units if time_units else "",
        )
        project = JsonProject(
            name=name,
            sim_specs=sim_specs,
            models=[JsonModel(name="main")],
        )
        json_data = json.dumps(converter.unstructure(project)).encode("utf-8")
        project_ptr = _ffi_open_json(json_data)
        return cls(project_ptr)

    def __get_model_count(self) -> int:
        """Internal method to get the number of models in the project.

        Caller must hold ``_lock``.
        """
        count_ptr = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_project_get_model_count(self._ptr, count_ptr, err_ptr)
        check_out_error(err_ptr, "Get model count")
        return int(count_ptr[0])

    def get_model_names(self) -> list[str]:
        """Get the names of all models in the project.

        Returns:
            List of model names
        """
        with self._lock:
            self._check_alive()
            count = self.__get_model_count()
            if count == 0:
                return []

            # Allocate array for C string pointers
            c_names = ffi.new("char *[]", count)
            out_written_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_project_get_model_names(self._ptr, c_names, count, out_written_ptr, err_ptr)
            check_out_error(err_ptr, "Get model names")

            written = int(out_written_ptr[0])
            if written != count:
                for i in range(count):
                    if c_names[i] != ffi.NULL:
                        free_c_string(c_names[i])
                raise SimlinImportError(
                    f"Failed to get model names: got {written}, expected {count}"
                )

            # Convert to Python strings and free C memory
            names: list[str] = []
            for i in range(count):
                if c_names[i] != ffi.NULL:
                    name = c_to_string(c_names[i])
                    free_c_string(c_names[i])
                    if name is not None:
                        names.append(name)

            return names

    def get_model(self, name: str = "") -> Model:
        """Get a model from the project by name.

        Args:
            name: The model name, or empty string for the default/main model

        Returns:
            The requested Model instance

        Raises:
            SimlinImportError: If the model doesn't exist
        """
        from .model import Model

        names = self.get_model_names()
        if name:
            if name not in names:
                raise SimlinImportError(f"Model not found: {name}")
            resolved_name = name
        else:
            if not names:
                raise SimlinImportError("Project contains no models")
            resolved_name = names[0]

        with self._lock:
            self._check_alive()
            c_name = string_to_c(resolved_name) if name else ffi.NULL
            err_ptr = ffi.new("SimlinError **")
            model_ptr = lib.simlin_project_get_model(self._ptr, c_name, err_ptr)
            check_out_error(err_ptr, f"Get model '{name or 'default'}'")

        return Model(model_ptr, project=self, name=resolved_name)

    @property
    def models(self) -> tuple[Model, ...]:
        """All models in this project (immutable tuple).

        Returns:
            Tuple of all Model objects in the project

        Example:
            >>> for model in project.models:
            ...     print(model._name)
        """
        model_names = self.get_model_names()
        return tuple(self.get_model(name) for name in model_names)

    @property
    def main_model(self) -> Model:
        """The main/default model.

        Returns:
            The first/main model in the project

        Raises:
            SimlinImportError: If the project has no models

        Example:
            >>> model = project.main_model
        """
        return self.get_model()

    def get_errors(self) -> list[ErrorDetail]:
        """Get all errors in the project (compilation and validation).

        Returns:
            List of ErrorDetail objects, or empty list if no errors
        """
        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            error_ptr = lib.simlin_project_get_errors(self._ptr, err_ptr)
            check_out_error(err_ptr, "Get errors")

        if error_ptr == ffi.NULL:
            return []

        try:
            return _collect_error_details(error_ptr)
        finally:
            lib.simlin_error_free(error_ptr)

    def to_xmile(self) -> bytes:
        """Export the project to XMILE format.

        Returns:
            The XMILE XML data as bytes

        Raises:
            SimlinImportError: If export fails
        """
        with self._lock:
            self._check_alive()
            output_ptr = ffi.new("uint8_t **")
            output_len_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_project_serialize_xmile(self._ptr, output_ptr, output_len_ptr, err_ptr)
            check_out_error(err_ptr, "Export to XMILE")

            if output_ptr[0] == ffi.NULL:
                raise SimlinImportError("Export returned null output")

            try:
                return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
            finally:
                lib.simlin_free(output_ptr[0])

    def _apply_patch_json(
        self,
        patch_json: bytes,
        *,
        dry_run: bool = False,
        allow_errors: bool = False,
        expected_revision: int | None = None,
    ) -> list[ErrorDetail]:
        """Apply a JSON patch, surfacing validation details as Python exceptions.

        Rejection vs diagnostics: the engine REJECTS a patch only for genuine
        validation failures (error-severity diagnostics, or a NEW unit warning
        in a previously-clean model); a rejection raises here with the
        underlying details attached. Separately, the engine reports every
        diagnostic on the patched project -- including warnings that existed
        BEFORE the patch -- through the collected-errors channel. Those are
        informational: an accepted-and-committed patch must not look like a
        failure just because the project already had warnings, so they are
        returned, not raised.

        Args:
            patch_json: JSON-encoded patch data (UTF-8 bytes)
            dry_run: If True, validate without applying changes
            allow_errors: If True, apply even when validation reports errors
            expected_revision: The :attr:`revision` the patch was built
                against; if the project has changed since (a reload from
                disk, another edit), the patch is rejected unapplied

        Returns:
            List of ErrorDetail objects for collected diagnostics (including
            pre-existing warnings on the project)

        Raises:
            SimlinRuntimeError: If the patch is rejected (or fails to
                parse/apply), or the project's revision no longer matches
                ``expected_revision``; the exception carries the underlying
                diagnostics on its ``details`` attribute.
        """
        with self._file_lock:
            if expected_revision is not None and expected_revision != self._sync.revision:
                raise SimlinRuntimeError(
                    f"project changed during edit (revision {expected_revision} -> "
                    f"{self._sync.revision}); the edit was not applied, re-run it "
                    f"against the current contents"
                )
            with self._lock:
                self._check_alive()
                diagnostics = _ffi_apply_patch_json(self._ptr, patch_json, dry_run, allow_errors)
            if dry_run:
                return diagnostics
            # Every accepted mutation funnels through here (edit(),
            # set_sim_specs()); a file-backed project keeps its diagram in
            # step with the variables before the change is committed/saved.
            if self._path is not None:
                self._sync_diagram_for_patch(patch_json)
            revision, write_error = self._commit_change_locked()
        self._notify(ChangeEvent("edit", revision))
        if write_error is not None:
            raise write_error
        return diagnostics

    def serialize_json(self, format: str = JSON_FORMAT_SIMLIN) -> bytes:
        """Serialize the project to JSON.

        Args:
            format: ``JSON_FORMAT_SIMLIN`` (native Simlin JSON, the default)
                or ``JSON_FORMAT_SDAI`` (the SD-AI interchange format).

        Returns:
            Compact JSON-encoded project data (UTF-8 bytes)

        Raises:
            ValueError: If ``format`` is not one of the two constants
            SimlinRuntimeError: If serialization fails
        """
        if format == JSON_FORMAT_SIMLIN:
            with self._lock:
                self._check_alive()
                return _ffi_serialize_json(self._ptr)
        if format != JSON_FORMAT_SDAI:
            raise ValueError(
                f"unknown JSON format {format!r}; expected "
                f"{JSON_FORMAT_SIMLIN!r} or {JSON_FORMAT_SDAI!r}"
            )
        with self._lock:
            self._check_alive()
            output_ptr = ffi.new("uint8_t **")
            output_len_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_project_serialize_json(
                self._ptr,
                lib.SIMLIN_JSON_FORMAT_SDAI,
                False,  # include_stdlib
                output_ptr,
                output_len_ptr,
                err_ptr,
            )
            check_out_error(err_ptr, "Project SD-AI JSON serialization")
            if output_ptr[0] == ffi.NULL:
                raise SimlinImportError("Serialize returned null output")
            try:
                return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
            finally:
                lib.simlin_free(output_ptr[0])

    def to_mdl(self) -> bytes:
        """Export the project to Vensim MDL format (including the sketch).

        Constructs MDL cannot express (a non-negative flag, a discrete
        lookup) are dropped and reported as ``RuntimeWarning``s, once per
        distinct message for the lifetime of this project.

        Returns:
            The ``.mdl`` text as UTF-8 bytes

        Raises:
            SimlinRuntimeError: If export fails
        """
        with self._lock:
            self._check_alive()
            data, issues = _ffi_serialize_mdl(self._ptr)
        # An autosaving .mdl project exports on every edit, and the same
        # lossy constructs (a non-negative flag, a discrete lookup) are
        # dropped every time; warn once per distinct message per project --
        # the same rule the watcher applies to bad on-disk content.  A
        # message is remembered for the project's lifetime, so a construct
        # that disappears and later reappears does not warn again.
        fresh: list[str] = []
        with self._file_lock:
            for issue in issues:
                message = f"simlin: Vensim export: {issue.message}" + (
                    f" ({issue.variable_name})" if issue.variable_name else ""
                )
                if message not in self._mdl_warned:
                    self._mdl_warned.add(message)
                    fresh.append(message)
        for message in fresh:
            warnings.warn(message, RuntimeWarning, stacklevel=2)
        return data

    def set_sim_specs(self, **kwargs: Any) -> None:
        """Update the project's simulation specifications.

        Args:
            start: Simulation start time (float)
            stop: Simulation stop time (float)
            dt: Time step (float or string)
            save_step: Save step interval (float)
            sim_method: Simulation method (0 for "euler", 1 for "rk4", or string)
            time_units: Time units string
        """
        if not kwargs:
            raise ValueError("set_sim_specs requires at least one field")

        # Read current specs via JSON
        project_json = json.loads(self.serialize_json().decode("utf-8"))
        current = converter.structure(project_json["simSpecs"], JsonSimSpecs)

        # Map from legacy protobuf-style field names to JSON field names
        field_mapping = {"start": "start_time", "stop": "end_time", "sim_method": "method"}

        # Build updates dict
        updates: dict[str, Any] = {}
        for key, value in kwargs.items():
            json_key = field_mapping.get(key, key)
            if json_key == "dt":
                updates["dt"] = validate_dt(value)
            elif json_key == "save_step":
                updates["save_step"] = float(value) if value is not None else 0.0
            elif json_key == "method":
                method_map = {0: "euler", 1: "rk4"}
                if isinstance(value, int):
                    updates["method"] = method_map.get(value, "euler")
                else:
                    updates["method"] = str(value).lower()
            elif json_key in {"start_time", "end_time"}:
                updates[json_key] = float(value)
            elif json_key == "time_units":
                updates["time_units"] = str(value) if value else ""
            else:
                raise ValueError(f"Unknown SimSpecs field: {key}")

        new_specs = dataclasses.replace(current, **updates)

        # Apply patch using JSON
        patch = JsonProjectPatch(project_ops=[SetSimSpecs(sim_specs=new_specs)])
        patch_json = json.dumps(converter.unstructure(patch)).encode("utf-8")
        self._apply_patch_json(patch_json)

    def serialize_protobuf(self) -> bytes:
        """Serialize the project to binary protobuf format.

        Returns:
            The protobuf binary data

        Raises:
            SimlinImportError: If serialization fails
        """
        with self._lock:
            self._check_alive()
            output_ptr = ffi.new("uint8_t **")
            output_len_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_project_serialize_protobuf(self._ptr, output_ptr, output_len_ptr, err_ptr)
            check_out_error(err_ptr, "Project serialization")

            if output_ptr[0] == ffi.NULL:
                raise SimlinImportError("Serialize returned null output")

            try:
                return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
            finally:
                lib.simlin_free(output_ptr[0])

    def auto_layout(self, model_name: str = "main") -> None:
        """Generate and persist an automatic diagram layout for a model.

        Computes positions for every variable and replaces the model's
        diagram views with the result, so the layout survives serialization
        (:meth:`to_xmile`, :meth:`serialize_json`). Rendering does not
        require this: :meth:`render_svg` and :meth:`render_png` generate a
        transient layout automatically when the model has no view. On a
        model that already has a diagram, the existing layout is discarded
        and regenerated (only the zoom level is preserved).

        Args:
            model_name: Name of the model to lay out (default: ``"main"``)

        Raises:
            SimlinRuntimeError: If the model doesn't exist or layout fails
        """
        with self._file_lock:
            with self._lock:
                self._check_alive()
                _ffi_diagram_sync(self._ptr, model_name)
            revision, write_error = self._commit_change_locked()
        self._notify(ChangeEvent("edit", revision))
        if write_error is not None:
            raise write_error

    def render_svg(self, model_name: str = "main") -> bytes:
        """Render a model's stock-and-flow diagram as SVG.

        A model without a diagram view (e.g. one built from scratch through
        ``Model.edit()``) is rendered with an automatically generated
        layout; use :meth:`auto_layout` to persist such a layout instead.

        Args:
            model_name: Name of the model to render (default: ``"main"``)

        Returns:
            SVG data as UTF-8 encoded bytes

        Raises:
            SimlinRuntimeError: If the model doesn't exist or rendering fails
        """
        with self._lock:
            self._check_alive()
            return _ffi_render_svg(self._ptr, model_name)

    def render_svg_string(self, model_name: str = "main") -> str:
        """Render a model's stock-and-flow diagram as an SVG string.

        Convenience wrapper around :meth:`render_svg` that decodes the
        result to a Python string.

        Args:
            model_name: Name of the model to render (default: ``"main"``)

        Returns:
            SVG string
        """
        return self.render_svg(model_name).decode("utf-8")

    def render_png(
        self,
        model_name: str = "main",
        *,
        width: int = 0,
        height: int = 0,
    ) -> bytes:
        """Render a model's stock-and-flow diagram as a PNG image.

        Pass ``width=0`` and ``height=0`` (or omit them) to use the SVG's
        intrinsic dimensions. When only one dimension is non-zero the other
        is derived from the aspect ratio. When both are non-zero, ``width``
        takes precedence.

        A model without a diagram view (e.g. one built from scratch through
        ``Model.edit()``) is rendered with an automatically generated
        layout; use :meth:`auto_layout` to persist such a layout instead.

        Args:
            model_name: Name of the model to render (default: ``"main"``)
            width: Target width in pixels (0 for intrinsic)
            height: Target height in pixels (0 for intrinsic)

        Returns:
            PNG image data as bytes

        Raises:
            SimlinRuntimeError: If the model doesn't exist or rendering fails
        """
        with self._lock:
            self._check_alive()
            return _ffi_render_png(self._ptr, model_name, width, height)

    def __enter__(self) -> Self:
        """Context manager entry point."""
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> None:
        """Context manager exit point with explicit cleanup."""
        self.close()

    def close(self) -> None:
        """Release the engine handle, stop watching, and drop subscribers.

        Idempotent.  Unsaved changes are NOT written; call :meth:`save`
        first if you want them.
        """
        self.watch(False)
        with self._file_lock:
            self._listeners.clear()
            self._closed = True
            self._path = None
            self._format = None
        with self._lock:
            finalizer = getattr(self, "_finalizer", None)
            if finalizer and getattr(finalizer, "alive", False):
                finalizer()
            self._ptr = ffi.NULL

    def __repr__(self) -> str:
        """Return a string representation of the Project."""
        try:
            with self._lock:
                self._check_alive()
                model_count = self.__get_model_count()
            return f"<Project with {model_count} model(s)>"
        except Exception:
            return "<Project (invalid)>"
