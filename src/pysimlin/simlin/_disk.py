"""Disk I/O for file-backed projects: content hashing, atomic writes, and
the polling file watcher.

pattern: Imperative Shell (filesystem and threads; no engine, no policy)

Policy -- what an observed change *means* -- lives in ``simlin._sync``; this
module only moves bytes and reports what it saw.
"""

from __future__ import annotations

import contextlib
import hashlib
import os
import secrets
import shutil
import threading
import warnings
from pathlib import Path
from typing import TYPE_CHECKING, Any, Union

if TYPE_CHECKING:
    import weakref
    from collections.abc import Callable

_PathLike = Union[str, Path]


class _Unknown:
    """Type of the ``_UNKNOWN`` sentinel (distinct from ``None`` = "the file
    did not exist at the last tick")."""


_UNKNOWN = _Unknown()


def content_hash(data: bytes) -> str:
    """Hex SHA-256 of ``data``: the identity of a file's contents used for
    echo suppression and change detection."""
    return hashlib.sha256(data).hexdigest()


def atomic_write(path: _PathLike, data: bytes) -> None:
    """Write ``data`` to ``path`` atomically: a sibling tempfile is written,
    fsynced, and renamed over the target, so a reader (our own watcher, an
    editor, ``simlin-serve``) sees either the old bytes or the new bytes and
    never a partial file.  This is the same convention as
    ``simlin_engine::io::atomic_write``.

    The tempfile is created with ``open(..., "xb")`` so it gets the ordinary
    umask-derived mode rather than ``mkstemp``'s private ``0600``; when the
    target already exists its mode is copied so a save does not silently
    change a model file's permissions.  A failure at any step removes the
    tempfile and re-raises; the target is untouched.
    """
    target = Path(path)
    parent = target.parent
    for _ in range(16):
        tmp = parent / f".{target.name}.{os.getpid()}.{secrets.token_hex(4)}.tmp"
        try:
            f = tmp.open("xb")
        except FileExistsError:
            continue
        break
    else:  # pragma: no cover - 16 random collisions
        raise OSError(f"could not create a temporary file next to {target}")

    try:
        with f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        if target.exists():
            shutil.copymode(target, tmp)
        os.replace(tmp, target)
    except BaseException:
        with contextlib.suppress(OSError):
            tmp.unlink()
        raise

    # Best-effort directory fsync so the rename itself is durable; not
    # supported everywhere (Windows cannot open a directory), hence tolerant.
    try:
        dir_fd = os.open(parent, os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(dir_fd)
    except OSError:
        pass
    finally:
        os.close(dir_fd)


class FileWatcher:
    """Poll one file for changes on a daemon thread.

    Why polling from the standard library rather than a ``watchfiles`` /
    inotify dependency: a project watches exactly one file, so a
    ``stat`` every half second is negligible; polling works on every
    filesystem including network mounts and Colab's FUSE-mounted Drive
    where inotify delivers nothing; and it adds no native wheel to
    ``pip install pysimlin``.

    Each tick stats the file and, when its ``(mtime, size, inode)``
    signature changed, reads the bytes and calls
    ``handler(watcher, data, hash)``.  The very first tick always reads and
    delivers, so a change that landed between the caller's last read of the
    file and ``start()`` is never missed (the receiver recognises its own
    bytes by hash).  The read is re-checked against a second stat so a
    non-atomic writer caught mid-write is skipped this tick rather than
    delivered as a truncated file.  ``handler`` returns ``False`` to stop the
    thread (the ``Project`` uses this to end watching once it has been
    garbage collected, so the thread never keeps a project alive).

    A watcher is single-use: once stopped it cannot be started again --
    create a new one.  This keeps "is this the project's current watcher"
    a simple identity check for the receiver.

    The thread never dies silently: any exception from the stat/read/handler
    path is reported through ``warnings.warn(RuntimeWarning)`` -- once per
    distinct message until a tick succeeds -- and polling continues.
    """

    def __init__(
        self,
        path: _PathLike,
        handler: Callable[[FileWatcher, bytes, str], bool],
        *,
        interval: float = 0.5,
    ) -> None:
        if interval <= 0:
            raise ValueError(f"interval must be positive, got {interval}")
        self._path = Path(path)
        self._handler = handler
        self._interval = float(interval)
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        # No signature yet: the first tick reads whatever is there.
        self._last_signature: tuple[int, int, int] | None | _Unknown = _UNKNOWN
        self._warned: set[str] = set()
        self._stopped = False
        # Owner-managed: the Project stores the weakref.finalize that stops
        # this watcher when the project is collected, so retiring the
        # watcher early can detach it.
        self.finalizer: weakref.finalize[Any, Any] | None = None

    @property
    def path(self) -> Path:
        return self._path

    @property
    def interval(self) -> float:
        return self._interval

    @property
    def running(self) -> bool:
        thread = self._thread
        return thread is not None and thread.is_alive()

    def start(self) -> None:
        if self._stopped:
            raise RuntimeError("a stopped FileWatcher cannot be restarted; create a new one")
        if self.running:
            return
        self._thread = threading.Thread(
            target=self._run, name=f"simlin-watch:{self._path.name}", daemon=True
        )
        self._thread.start()

    def request_stop(self) -> None:
        """Ask the poll thread to exit at its next wake-up without waiting
        for it.  Safe from any context, including a ``weakref.finalize``
        callback during garbage collection where joining would be wrong."""
        self._stopped = True
        self._stop.set()

    def stop(self) -> None:
        """Stop polling and (unless called from the poll thread itself, e.g.
        inside a change callback) wait for the thread to exit."""
        self.request_stop()
        thread = self._thread
        if thread is not None and thread is not threading.current_thread():
            thread.join(timeout=max(1.0, self._interval * 4))
        self._thread = None

    def poll_once(self) -> bool:
        """Run one tick synchronously; returns ``False`` when the handler
        asked to stop.  Tests drive the watcher deterministically through
        this; the thread runs the same code.  Errors are reported as
        warnings (once per distinct message until a tick succeeds), never
        raised, so the poll thread cannot die of them."""
        try:
            keep_going = self._tick()
        except Exception as exc:  # the thread must survive anything
            message = f"simlin: watching {self._path}: {exc}"
            if message not in self._warned:
                self._warned.add(message)
                warnings.warn(message, RuntimeWarning, stacklevel=2)
            return True
        self._warned.clear()
        return keep_going

    def _run(self) -> None:
        while not self._stop.wait(self._interval):
            if not self.poll_once():
                return

    def _signature(self) -> tuple[int, int, int] | None:
        try:
            st = os.stat(self._path)
        except FileNotFoundError:
            return None
        return (st.st_mtime_ns, st.st_size, st.st_ino)

    def _tick(self) -> bool:
        before = self._signature()
        if before == self._last_signature:
            return True
        if before is None:
            # No file.  On the first tick that means we were pointed at a
            # path that does not exist (usually a mistake); later it means it
            # was deleted or is mid-rename by a non-atomic writer.  Remember
            # that so its (re)appearance is a change, and tell the user once
            # -- a tick that finds the file clears the warning memory.
            never_seen = self._last_signature is _UNKNOWN
            self._last_signature = None
            if never_seen:
                raise FileNotFoundError(f"{self._path} does not exist")
            raise FileNotFoundError(f"{self._path} no longer exists")
        data = self._path.read_bytes()
        after = self._signature()
        if after != before:
            # Caught a writer mid-flight; leave the signature alone so the
            # next tick re-reads the settled file.
            return True
        self._last_signature = before
        return self._handler(self, data, content_hash(data))


__all__ = ["FileWatcher", "atomic_write", "content_hash"]
