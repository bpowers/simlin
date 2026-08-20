"""Exhaustive arm coverage for the pure sync state machine (``simlin._sync``).

Rows are derived from the event x state enumeration in ``_sync.decide``:
every ``case`` arm and every branch inside it appears below.  Anything not
listed here is not a decision the machine makes.
"""

from __future__ import annotations

from dataclasses import fields

import pytest

from simlin import _sync
from simlin._sync import (
    AttemptReload,
    DiskObserved,
    DiskParsed,
    ExplicitReload,
    IgnoreEcho,
    KeepLastKnownGood,
    LocalEdit,
    MarkDirty,
    NoChange,
    RejectStale,
    Reload,
    SyncState,
    WidgetSnapshot,
    Write,
    WriteCompleted,
    decide,
)

ON_DISK = "hash-on-disk"
NOVEL = "hash-novel"
BAD = "hash-bad"

CLEAN = SyncState(revision=3, disk_hash=ON_DISK)
DIRTY = SyncState(revision=3, disk_hash=ON_DISK, dirty=True)
DECLINED = SyncState(revision=3, disk_hash=ON_DISK, declined_hashes=frozenset({BAD}))
IN_MEMORY = SyncState()


class TestLocalEdit:
    """LocalEdit x persist in {True, False}."""

    def test_persist_writes_and_bumps(self) -> None:
        decision, state = decide(CLEAN, LocalEdit(persist=True))
        assert decision == Write(revision=4)
        assert state == SyncState(revision=4, disk_hash=ON_DISK, dirty=True)

    def test_no_persist_marks_dirty_and_bumps(self) -> None:
        decision, state = decide(CLEAN, LocalEdit(persist=False))
        assert decision == MarkDirty(revision=4)
        assert state == SyncState(revision=4, disk_hash=ON_DISK, dirty=True)

    def test_in_memory_project_edits_bump_from_zero(self) -> None:
        decision, state = decide(IN_MEMORY, LocalEdit(persist=False))
        assert decision == MarkDirty(revision=1)
        assert state.revision == 1
        assert state.disk_hash is None


class TestWidgetSnapshot:
    """WidgetSnapshot x (base == revision, base != revision) x persist."""

    @pytest.mark.parametrize("persist", [True, False])
    def test_stale_base_is_rejected_without_state_change(self, persist: bool) -> None:
        decision, state = decide(CLEAN, WidgetSnapshot(base_revision=2, persist=persist))
        assert decision == RejectStale(revision=3)
        assert state == CLEAN

    def test_future_base_is_also_stale(self) -> None:
        # A base ahead of the kernel cannot happen in a healthy protocol;
        # treat it exactly like any other mismatch rather than accepting it.
        decision, state = decide(CLEAN, WidgetSnapshot(base_revision=4, persist=True))
        assert decision == RejectStale(revision=3)
        assert state == CLEAN

    def test_current_base_persist_writes(self) -> None:
        decision, state = decide(CLEAN, WidgetSnapshot(base_revision=3, persist=True))
        assert decision == Write(revision=4)
        assert state == SyncState(revision=4, disk_hash=ON_DISK, dirty=True)

    def test_current_base_no_persist_marks_dirty(self) -> None:
        decision, state = decide(CLEAN, WidgetSnapshot(base_revision=3, persist=False))
        assert decision == MarkDirty(revision=4)
        assert state.dirty is True


class TestWriteCompleted:
    """WriteCompleted records the on-disk hash, clears dirty and the
    declined set, and never bumps the revision."""

    def test_records_hash_and_clears_flags(self) -> None:
        start = SyncState(
            revision=4, disk_hash=ON_DISK, dirty=True, declined_hashes=frozenset({BAD})
        )
        decision, state = decide(start, WriteCompleted(content_hash=NOVEL))
        assert decision == NoChange()
        assert state == SyncState(revision=4, disk_hash=NOVEL)

    def test_first_write_of_in_memory_project(self) -> None:
        decision, state = decide(IN_MEMORY, WriteCompleted(content_hash=NOVEL))
        assert decision == NoChange()
        assert state == SyncState(revision=0, disk_hash=NOVEL)


class TestDiskObserved:
    """DiskObserved x {hash == disk_hash, hash declined, dirty, novel}."""

    def test_our_own_bytes_are_an_echo(self) -> None:
        decision, state = decide(CLEAN, DiskObserved(content_hash=ON_DISK))
        assert decision == IgnoreEcho()
        assert state == CLEAN

    def test_echo_wins_even_when_dirty(self) -> None:
        # Unchanged disk bytes are never a conflict, dirty or not.
        decision, state = decide(DIRTY, DiskObserved(content_hash=ON_DISK))
        assert decision == IgnoreEcho()
        assert state == DIRTY

    def test_already_declined_bytes_keep_quietly(self) -> None:
        decision, state = decide(DECLINED, DiskObserved(content_hash=BAD))
        assert decision == KeepLastKnownGood("declined")
        assert state == DECLINED

    def test_novel_bytes_while_dirty_keep_and_remember(self) -> None:
        decision, state = decide(DIRTY, DiskObserved(content_hash=NOVEL))
        assert decision == KeepLastKnownGood("dirty")
        assert state.declined_hashes == frozenset({NOVEL})
        assert state.dirty is True
        assert state.revision == 3

    def test_novel_bytes_while_clean_attempt_reload(self) -> None:
        decision, state = decide(CLEAN, DiskObserved(content_hash=NOVEL))
        assert decision == AttemptReload()
        assert state == CLEAN


class TestDiskParsed:
    """DiskParsed x ok in {True, False}."""

    def test_ok_reloads_bumps_and_resets(self) -> None:
        start = SyncState(
            revision=3, disk_hash=ON_DISK, dirty=False, declined_hashes=frozenset({BAD})
        )
        decision, state = decide(start, DiskParsed(content_hash=NOVEL, ok=True))
        assert decision == Reload(revision=4)
        assert state == SyncState(revision=4, disk_hash=NOVEL)

    def test_ok_after_explicit_reload_over_dirty_discards_dirty(self) -> None:
        decision, state = decide(DIRTY, DiskParsed(content_hash=ON_DISK, ok=True))
        assert decision == Reload(revision=4)
        assert state == SyncState(revision=4, disk_hash=ON_DISK)

    def test_not_ok_keeps_last_known_good_and_declines_hash(self) -> None:
        decision, state = decide(CLEAN, DiskParsed(content_hash=BAD, ok=False))
        assert decision == KeepLastKnownGood("unparsable")
        assert state == SyncState(revision=3, disk_hash=ON_DISK, declined_hashes=frozenset({BAD}))

    def test_not_ok_then_same_bytes_observed_is_quiet(self) -> None:
        _, state = decide(CLEAN, DiskParsed(content_hash=BAD, ok=False))
        decision, state = decide(state, DiskObserved(content_hash=BAD))
        assert decision == KeepLastKnownGood("declined")

    def test_not_ok_then_good_bytes_reload(self) -> None:
        _, state = decide(CLEAN, DiskParsed(content_hash=BAD, ok=False))
        decision, state = decide(state, DiskObserved(content_hash=NOVEL))
        assert decision == AttemptReload()
        decision, state = decide(state, DiskParsed(content_hash=NOVEL, ok=True))
        assert decision == Reload(revision=4)
        assert state.declined_hashes == frozenset()


class TestExplicitReload:
    """ExplicitReload x {unchanged & clean, unchanged & dirty, novel, declined}."""

    def test_unchanged_and_clean_is_idempotent(self) -> None:
        decision, state = decide(CLEAN, ExplicitReload(content_hash=ON_DISK))
        assert decision == NoChange()
        assert state == CLEAN

    def test_unchanged_but_dirty_reparses_to_discard_local_edits(self) -> None:
        decision, state = decide(DIRTY, ExplicitReload(content_hash=ON_DISK))
        assert decision == AttemptReload()
        assert state == DIRTY

    def test_novel_bytes_attempt_reload(self) -> None:
        decision, state = decide(CLEAN, ExplicitReload(content_hash=NOVEL))
        assert decision == AttemptReload()
        assert state == CLEAN

    def test_declined_bytes_are_retried_on_explicit_request(self) -> None:
        # The watcher stays quiet about declined bytes; an explicit reload()
        # is the user asking for the error, so it retries.
        decision, state = decide(DECLINED, ExplicitReload(content_hash=BAD))
        assert decision == AttemptReload()
        assert state == DECLINED


class TestCoverageOfEnumeration:
    """Guard rails so a new event or decision type cannot be added to
    ``_sync`` without a row above noticing."""

    def test_every_event_type_is_exercised(self) -> None:
        exercised = {
            LocalEdit,
            WidgetSnapshot,
            WriteCompleted,
            DiskObserved,
            DiskParsed,
            ExplicitReload,
        }
        assert set(_sync.SyncEvent.__args__) == exercised  # type: ignore[attr-defined]

    def test_every_decision_type_is_produced(self) -> None:
        produced = {
            Write,
            MarkDirty,
            RejectStale,
            IgnoreEcho,
            AttemptReload,
            Reload,
            KeepLastKnownGood,
            NoChange,
        }
        assert set(_sync.SyncDecision.__args__) == produced  # type: ignore[attr-defined]

    def test_state_fields_are_the_documented_four(self) -> None:
        # Adding a field to SyncState means new arms; make the author revisit
        # this file rather than letting the enumeration silently go stale.
        assert [f.name for f in fields(SyncState)] == [
            "revision",
            "disk_hash",
            "dirty",
            "declined_hashes",
        ]

    def test_unknown_event_raises(self) -> None:
        with pytest.raises(TypeError, match="unknown sync event"):
            decide(CLEAN, object())  # type: ignore[arg-type]
