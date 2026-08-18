"""Tests for ``simlin._disk``: atomic writes and the polling file watcher."""

from __future__ import annotations

import gc
import os
import stat
import threading
import time
import warnings
import weakref
from typing import TYPE_CHECKING

import pytest

from simlin._disk import FileWatcher, atomic_write, content_hash

if TYPE_CHECKING:
    from pathlib import Path


class TestContentHash:
    def test_is_stable_and_content_sensitive(self) -> None:
        assert content_hash(b"abc") == content_hash(b"abc")
        assert content_hash(b"abc") != content_hash(b"abd")
        assert len(content_hash(b"")) == 64


class TestAtomicWrite:
    def test_writes_bytes_and_leaves_no_tempfile(self, tmp_path: Path) -> None:
        target = tmp_path / "m.stmx"
        atomic_write(target, b"<xmile/>")
        assert target.read_bytes() == b"<xmile/>"
        assert [p.name for p in tmp_path.iterdir()] == ["m.stmx"]

    def test_replaces_existing_file(self, tmp_path: Path) -> None:
        target = tmp_path / "m.stmx"
        target.write_bytes(b"old")
        atomic_write(target, b"new")
        assert target.read_bytes() == b"new"

    def test_preserves_existing_mode(self, tmp_path: Path) -> None:
        target = tmp_path / "m.stmx"
        target.write_bytes(b"old")
        target.chmod(0o640)
        atomic_write(target, b"new")
        assert stat.S_IMODE(target.stat().st_mode) == 0o640

    def test_new_file_is_not_private(self, tmp_path: Path) -> None:
        # mkstemp-style 0600 would make a saved model unreadable to
        # collaborators; a fresh file must get the ordinary umask-derived mode.
        target = tmp_path / "m.stmx"
        atomic_write(target, b"new")
        mode = stat.S_IMODE(target.stat().st_mode)
        assert mode & stat.S_IRUSR
        assert mode & stat.S_IWUSR
        # Under any umask that permits group read (the common 022 / 002),
        # the file must be group-readable, which 0600 never is.
        umask = os.umask(0)
        os.umask(umask)
        if not umask & stat.S_IRGRP:
            assert mode & stat.S_IRGRP

    def test_failure_leaves_target_untouched_and_no_tempfile(self, tmp_path: Path) -> None:
        target = tmp_path / "m.stmx"
        target.write_bytes(b"old")

        # A directory as the target: os.replace onto a non-empty directory
        # fails on every platform, exercising the cleanup path.
        bad_target = tmp_path / "adir"
        bad_target.mkdir()
        (bad_target / "occupant").write_bytes(b"")
        with pytest.raises(OSError):  # noqa: PT011 - platform-specific errno
            atomic_write(bad_target, b"x")
        assert bad_target.is_dir()
        assert sorted(p.name for p in tmp_path.iterdir()) == ["adir", "m.stmx"]
        assert target.read_bytes() == b"old"

    def test_missing_parent_directory_raises(self, tmp_path: Path) -> None:
        with pytest.raises(FileNotFoundError):
            atomic_write(tmp_path / "nope" / "m.stmx", b"x")


class _Recorder:
    """Records watcher deliveries; ``keep_going`` lets a test ask the
    watcher to stop through the handler's return value."""

    def __init__(self) -> None:
        self.calls: list[tuple[bytes, str]] = []
        self.keep_going = True

    def __call__(self, data: bytes, digest: str) -> bool:
        self.calls.append((data, digest))
        return self.keep_going


def _touch_change(path: Path, data: bytes) -> None:
    """Rewrite ``path`` with a guaranteed-different stat signature.

    Same-size rewrites within one mtime tick are what the signature could
    miss on coarse filesystems; force a fresh inode via atomic_write so the
    test is deterministic everywhere."""
    atomic_write(path, data)


class TestFileWatcherPolling:
    def test_rejects_non_positive_interval(self, tmp_path: Path) -> None:
        with pytest.raises(ValueError, match="interval"):
            FileWatcher(tmp_path / "m", _Recorder(), interval=0)

    def test_unchanged_file_delivers_nothing(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")
        rec = _Recorder()
        watcher = FileWatcher(path, rec, interval=0.01)
        assert watcher.poll_once() is True
        assert watcher.poll_once() is True
        assert rec.calls == []

    def test_change_delivers_bytes_and_hash_once(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")
        rec = _Recorder()
        watcher = FileWatcher(path, rec, interval=0.01)
        _touch_change(path, b"two")
        assert watcher.poll_once() is True
        assert rec.calls == [(b"two", content_hash(b"two"))]
        # Same content again: signature unchanged, nothing new delivered.
        assert watcher.poll_once() is True
        assert rec.calls == [(b"two", content_hash(b"two"))]

    def test_file_created_after_watcher_starts_is_delivered(self, tmp_path: Path) -> None:
        path = tmp_path / "later.stmx"
        rec = _Recorder()
        watcher = FileWatcher(path, rec, interval=0.01)
        assert watcher.poll_once() is True
        assert rec.calls == []
        path.write_bytes(b"now")
        assert watcher.poll_once() is True
        assert rec.calls == [(b"now", content_hash(b"now"))]

    def test_missing_file_warns_once_then_recovers(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")
        rec = _Recorder()
        watcher = FileWatcher(path, rec, interval=0.01)
        path.unlink()
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            assert watcher.poll_once() is True
            assert watcher.poll_once() is True
        messages = [str(w.message) for w in caught if w.category is RuntimeWarning]
        assert len(messages) == 1
        assert "no longer exists" in messages[0]
        assert str(path) in messages[0]
        # Recreating the file is a change and is delivered.
        path.write_bytes(b"back")
        assert watcher.poll_once() is True
        assert rec.calls == [(b"back", content_hash(b"back"))]

    def test_handler_exception_is_warned_not_raised(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")

        def boom(data: bytes, digest: str) -> bool:
            raise RuntimeError("handler exploded")

        watcher = FileWatcher(path, boom, interval=0.01)
        _touch_change(path, b"two")
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            assert watcher.poll_once() is True
        assert any("handler exploded" in str(w.message) for w in caught)

    def test_handler_can_stop_the_watcher(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")
        rec = _Recorder()
        rec.keep_going = False
        watcher = FileWatcher(path, rec, interval=0.01)
        _touch_change(path, b"two")
        assert watcher.poll_once() is False


class TestFileWatcherThread:
    def test_thread_delivers_change_and_stops(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")
        delivered = threading.Event()
        seen: list[bytes] = []

        def handler(data: bytes, digest: str) -> bool:
            seen.append(data)
            delivered.set()
            return True

        watcher = FileWatcher(path, handler, interval=0.02)
        assert watcher.running is False
        watcher.start()
        assert watcher.running is True
        try:
            _touch_change(path, b"two")
            assert delivered.wait(5.0), "watcher thread never delivered the change"
            assert seen == [b"two"]
        finally:
            watcher.stop()
        assert watcher.running is False

    def test_start_is_idempotent_and_stop_is_safe_twice(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")
        watcher = FileWatcher(path, _Recorder(), interval=0.02)
        watcher.start()
        first = watcher._thread
        watcher.start()
        assert watcher._thread is first
        watcher.stop()
        watcher.stop()
        assert watcher.running is False

    def test_thread_exits_when_handler_returns_false(self, tmp_path: Path) -> None:
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")
        rec = _Recorder()
        rec.keep_going = False
        watcher = FileWatcher(path, rec, interval=0.02)
        watcher.start()
        thread = watcher._thread
        assert thread is not None
        _touch_change(path, b"two")
        thread.join(timeout=5.0)
        assert not thread.is_alive()
        assert watcher.running is False

    def test_thread_does_not_keep_handler_owner_alive(self, tmp_path: Path) -> None:
        # The Project passes a handler that only holds a weak reference to
        # itself and returns False once it is gone; the watcher must not
        # defeat that by strongly referencing anything else of the owner's.
        path = tmp_path / "m.stmx"
        path.write_bytes(b"one")

        class Owner:
            pass

        owner = Owner()
        owner_ref = weakref.ref(owner)

        def make_handler(ref: weakref.ref[Owner]):  # type: ignore[no-untyped-def]
            def handler(data: bytes, digest: str) -> bool:
                return ref() is not None

            return handler

        watcher = FileWatcher(path, make_handler(owner_ref), interval=0.02)
        watcher.start()
        try:
            del owner
            gc.collect()
            assert owner_ref() is None
            _touch_change(path, b"two")
            deadline = time.monotonic() + 5.0
            while watcher.running and time.monotonic() < deadline:
                time.sleep(0.02)
            assert watcher.running is False
        finally:
            watcher.stop()
