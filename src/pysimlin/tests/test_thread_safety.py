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
from simlin._ffi import _finalizer_refs, _refs_lock


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
                _ = model.stocks
                _ = model.flows
                _ = model.auxs
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

        results: dict[str, list[int]] = {"stocks": [], "flows": [], "auxs": []}
        errors: list[Exception] = []
        lock = threading.Lock()

        def read_properties() -> None:
            try:
                stocks = model.stocks
                flows = model.flows
                auxs = model.auxs
                with lock:
                    results["stocks"].append(len(stocks))
                    results["flows"].append(len(flows))
                    results["auxs"].append(len(auxs))
            except Exception as exc:
                with lock:
                    errors.append(exc)

        threads = [threading.Thread(target=read_properties) for _ in range(20)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)

        assert not errors, f"Concurrent property reads produced errors: {errors}"
        # All threads should see the same counts
        assert len(set(results["stocks"])) == 1
        assert len(set(results["flows"])) == 1
        assert len(set(results["auxs"])) == 1

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
                        _ = model.stocks
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
                    _ = model.stocks
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

    def test_refs_lock_protects_finalizer_refs(self) -> None:
        """The _refs_lock must be acquirable and protect _finalizer_refs."""
        # Verify the lock exists and is a real Lock
        assert isinstance(_refs_lock, type(threading.Lock()))

        # Verify we can acquire/release it
        acquired = _refs_lock.acquire(timeout=1)
        assert acquired
        # While holding the lock, access should be safe
        _ = len(_finalizer_refs)
        _refs_lock.release()


class TestPerObjectLockExists:
    """Verify that wrapper objects carry a threading.Lock."""

    def test_project_has_lock(self, xmile_model_path: Path) -> None:
        model = simlin.load(xmile_model_path)
        project = model.project
        assert project is not None
        assert hasattr(project, "_lock")
        assert isinstance(project._lock, type(threading.Lock()))

    def test_model_has_lock(self, xmile_model_path: Path) -> None:
        model = simlin.load(xmile_model_path)
        assert hasattr(model, "_lock")
        assert isinstance(model._lock, type(threading.Lock()))

    def test_sim_has_lock(self, xmile_model_path: Path) -> None:
        model = simlin.load(xmile_model_path)
        sim = model.simulate()
        assert hasattr(sim, "_lock")
        assert isinstance(sim._lock, type(threading.Lock()))
