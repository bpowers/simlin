"""Thread-safety tests for pysimlin.

These tests verify that the Python wrapper objects can be safely used
from multiple threads concurrently.  They exercise:

- Different objects from different threads (always safe)
- The same object from different threads (safe thanks to per-object locks)
- Module-level shared state (_finalizer_refs / _refs_lock)
- Context manager cleanup racing with method calls

The underlying Rust FFI layer uses per-object Mutexes, so correctness
at the C level is guaranteed.  These tests focus on the Python-level
locking that is critical for free-threaded Python (PEP 703 / 3.13t+).
"""

from __future__ import annotations

import contextlib
import gc
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

import pytest

import simlin
from simlin import SimlinRuntimeError


@pytest.fixture
def xmile_model_path() -> Path:
    """Path to the XMILE test model."""
    tests_dir = Path(__file__).parent
    return tests_dir / "fixtures" / "teacup.stmx"


@pytest.fixture
def mdl_model_path() -> Path:
    """Path to the MDL test model."""
    tests_dir = Path(__file__).parent
    return tests_dir / "fixtures" / "teacup.mdl"


class TestConcurrentDistinctObjects:
    """Test that distinct objects used from different threads work correctly."""

    def test_concurrent_project_loads(self, xmile_model_path: Path, mdl_model_path: Path) -> None:
        """Loading different projects concurrently must not corrupt state."""
        errors: list[Exception] = []

        def load_model(path: Path) -> None:
            try:
                model = simlin.load(path)
                _ = model.variables
            except Exception as exc:
                errors.append(exc)

        threads = []
        for _i in range(10):
            for path in (xmile_model_path, mdl_model_path):
                t = threading.Thread(target=load_model, args=(path,))
                threads.append(t)

        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)

        assert not errors, f"Concurrent loads produced errors: {errors}"

    def test_concurrent_simulations_different_objects(self, xmile_model_path: Path) -> None:
        """Running distinct simulations concurrently must produce correct results."""
        model = simlin.load(xmile_model_path)

        results: list[float] = []
        errors: list[Exception] = []
        lock = threading.Lock()

        def run_sim() -> None:
            try:
                sim = model.simulate()
                sim.run_to_end()
                val = sim.get_value("time")
                with lock:
                    results.append(val)
            except Exception as exc:
                with lock:
                    errors.append(exc)

        with ThreadPoolExecutor(max_workers=8) as pool:
            futures = [pool.submit(run_sim) for _ in range(20)]
            for fut in as_completed(futures):
                fut.result()  # propagate exceptions

        assert not errors, f"Concurrent sims produced errors: {errors}"
        assert len(results) == 20
        # All simulations should reach the same final time
        assert len(set(results)) == 1, f"Inconsistent final times: {set(results)}"


class TestSharedObjectAccess:
    """Test that the same object accessed from multiple threads is safe."""

    def test_concurrent_model_property_access(self, xmile_model_path: Path) -> None:
        """Reading cached properties from multiple threads must be safe.

        This specifically exercises the double-checked locking pattern
        used in Model's cached properties.
        """
        model = simlin.load(xmile_model_path)
        # Ensure caches are cold
        model._invalidate_caches()

        results: list[int] = []
        errors: list[Exception] = []
        lock = threading.Lock()

        def read_properties() -> None:
            try:
                variables = model.variables
                with lock:
                    results.append(len(variables))
            except Exception as exc:
                with lock:
                    errors.append(exc)

        threads = [threading.Thread(target=read_properties) for _ in range(20)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)

        assert not errors, f"Concurrent property reads produced errors: {errors}"
        # All threads should see the same count
        assert len(set(results)) == 1

    def test_concurrent_project_serialize(self, xmile_model_path: Path) -> None:
        """Serializing the same project from multiple threads must be safe."""
        model = simlin.load(xmile_model_path)
        project = model.project
        results: list[int] = []
        errors: list[Exception] = []
        lock = threading.Lock()

        def serialize() -> None:
            try:
                data = project.serialize_json()
                with lock:
                    results.append(len(data))
            except Exception as exc:
                with lock:
                    errors.append(exc)

        threads = [threading.Thread(target=serialize) for _ in range(20)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)

        assert not errors, f"Concurrent serialization produced errors: {errors}"
        assert len(results) == 20
        # All serializations should produce the same length
        assert len(set(results)) == 1

    def test_concurrent_sim_get_value(self, xmile_model_path: Path) -> None:
        """Getting values from the same Sim concurrently must be safe."""
        model = simlin.load(xmile_model_path)
        sim = model.simulate()
        sim.run_to_end()

        results: list[float] = []
        errors: list[Exception] = []
        lock = threading.Lock()

        def get_val() -> None:
            try:
                val = sim.get_value("time")
                with lock:
                    results.append(val)
            except Exception as exc:
                with lock:
                    errors.append(exc)

        threads = [threading.Thread(target=get_val) for _ in range(20)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)

        assert not errors, f"Concurrent get_value produced errors: {errors}"
        assert len(results) == 20
        assert len(set(results)) == 1


class TestContextManagerThreadSafety:
    """Test that context manager cleanup is safe under concurrent access."""

    def test_exit_while_method_in_progress(self, xmile_model_path: Path) -> None:
        """Calling __exit__ while another thread is using the object must not crash.

        After __exit__, the pointer is NULL and subsequent calls raise
        SimlinRuntimeError.  The key property is *no crash* (no SEGV,
        no use-after-free).
        """
        model = simlin.load(xmile_model_path)

        errors: list[Exception] = []
        barrier = threading.Barrier(2, timeout=10)

        def reader() -> None:
            try:
                barrier.wait()
                for _ in range(50):
                    # SimlinRuntimeError after close is expected; the
                    # important thing is that we never crash.
                    with contextlib.suppress(Exception):
                        _ = model.variables
            except Exception as exc:
                errors.append(exc)

        def closer() -> None:
            try:
                barrier.wait()
                model.__exit__(None, None, None)
            except Exception as exc:
                errors.append(exc)

        t1 = threading.Thread(target=reader)
        t2 = threading.Thread(target=closer)
        t1.start()
        t2.start()
        t1.join(timeout=10)
        t2.join(timeout=10)

        # The important assertion: no crashes or deadlocks
        assert not errors, f"Context manager race produced errors: {errors}"


class TestFinalizerRegistryThreadSafety:
    """Test that the module-level _finalizer_refs registry is thread-safe."""

    def test_concurrent_object_creation_and_gc(self, xmile_model_path: Path) -> None:
        """Creating and garbage-collecting objects from many threads must be safe.

        This exercises _refs_lock protecting _finalizer_refs.
        """
        errors: list[Exception] = []

        def create_and_discard() -> None:
            try:
                for _ in range(5):
                    model = simlin.load(xmile_model_path)
                    _ = model.variables
                    del model
                gc.collect()
            except Exception as exc:
                errors.append(exc)

        threads = [threading.Thread(target=create_and_discard) for _ in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)

        assert not errors, f"Concurrent creation/GC produced errors: {errors}"


class TestGetRunSnapshotAtomicity:
    """get_run() must take its snapshot atomically.

    The Run surfaces (results DataFrame, loops, ltm_mode) are read from the
    Sim by several separate accessor calls. If another thread advances or
    resets the Sim between those calls, the snapshot is torn -- e.g. the time
    index is captured at one length and a variable series at another, and
    DataFrame construction raises. get_run() therefore holds the Sim lock
    across the whole materialization.
    """

    def test_get_run_consistent_while_other_thread_resimulates(
        self, xmile_model_path: Path
    ) -> None:
        model = simlin.load(xmile_model_path)
        sim = model.simulate()
        sim.run_to_end()
        expected_len = sim.get_step_count()
        expected_cols = set(sim.get_run().results.columns)

        errors: list[Exception] = []
        stop = threading.Event()
        barrier = threading.Barrier(2, timeout=10)

        def resimulate() -> None:
            try:
                barrier.wait()
                while not stop.is_set():
                    sim.reset()
                    sim.run_to_end()
            except Exception as exc:
                errors.append(exc)

        def snapshot() -> None:
            try:
                barrier.wait()
                for _ in range(100):
                    try:
                        run = sim.get_run()
                    except SimlinRuntimeError as exc:
                        # An atomic snapshot taken between reset() and
                        # run_to_end() legitimately has no results yet.
                        # Anything else is a real failure.
                        if "no results" not in str(exc).lower():
                            raise
                        continue
                    df = run.results
                    # A torn snapshot manifests as an exception inside
                    # get_run() (index/column length mismatch) or as a frame
                    # with missing columns (per-variable reads that raced the
                    # reset are silently suppressed).
                    assert len(df) == expected_len, (
                        f"snapshot has {len(df)} rows, expected {expected_len}"
                    )
                    assert set(df.columns) == expected_cols, (
                        f"snapshot is missing columns: {expected_cols - set(df.columns)}"
                    )
            except Exception as exc:
                errors.append(exc)
            finally:
                stop.set()

        t1 = threading.Thread(target=resimulate)
        t2 = threading.Thread(target=snapshot)
        t1.start()
        t2.start()
        t1.join(timeout=30)
        t2.join(timeout=30)

        assert not errors, f"Torn get_run() snapshot: {errors}"

    def test_reset_cannot_interleave_with_snapshot_reads(self, xmile_model_path: Path) -> None:
        """Deterministically force a reset() between two snapshot reads.

        The hammer test above depends on thread timing; this one pins the
        exact interleaving the Sim lock must exclude. The first get_series()
        call inside get_run()'s materialization unblocks a thread that calls
        sim.reset(), then waits for it briefly. Without get_run() holding the
        Sim lock across the whole snapshot, the reset lands mid-snapshot and
        the remaining per-variable reads see an empty sim (those errors are
        suppressed, yielding a frame with missing columns). With the lock
        held, the reset blocks until the snapshot is complete.
        """
        model = simlin.load(xmile_model_path)
        sim = model.simulate()
        sim.run_to_end()
        expected_len = sim.get_step_count()
        expected_cols = set(sim.get_run().results.columns)

        real_get_series = sim.get_series
        reset_requested = threading.Event()
        reset_finished = threading.Event()
        injected = False

        def resetter() -> None:
            reset_requested.wait(timeout=10)
            sim.reset()
            reset_finished.set()

        def injecting_get_series(name: str):
            nonlocal injected
            series = real_get_series(name)
            if not injected:
                injected = True
                reset_requested.set()
                # With the snapshot correctly locked the reset cannot start,
                # so this wait times out; without the lock the reset completes
                # here and tears the snapshot.
                reset_finished.wait(timeout=0.5)
            return series

        t = threading.Thread(target=resetter)
        t.start()
        sim.get_series = injecting_get_series  # type: ignore[method-assign]
        try:
            run = sim.get_run()
        finally:
            sim.get_series = real_get_series  # type: ignore[method-assign]
        t.join(timeout=10)

        assert injected, "fault injection never fired; test is vacuous"
        df = run.results
        assert len(df) == expected_len
        assert set(df.columns) == expected_cols, (
            f"snapshot is missing columns: {expected_cols - set(df.columns)}"
        )
