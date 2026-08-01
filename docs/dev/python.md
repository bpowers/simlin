# Python (pysimlin) Development Standards

## Code Style

- Use `ruff` for both linting and formatting (replaces black). Run `ruff check` and `ruff format`.
- Use `mypy` with strict mode (`mypy simlin`).
- Target Python 3.11+. Use modern type syntax (`list[str]`, `dict[str, int]`, `X | None`) and `from __future__ import annotations` in all source files.

## Thread Safety

- **All wrapper classes** (`Project`, `Model`, `Sim`) have a per-instance lock (`self._lock`) that protects `_ptr` and cached state. `Project` and `Model` use `threading.Lock`; `Sim` uses `threading.RLock` so `Sim.get_run()` can hold the lock across the whole Run snapshot (which re-enters the individually locked accessors on the same thread) -- without that outer hold, a concurrent `run_to`/`reset`/`close` could land between the snapshot's reads and tear it.
- **Module-level `_finalizer_refs`** (a `WeakValueDictionary`) is protected by `_refs_lock` in `_ffi.py`.
- When adding new methods to wrapper classes, always acquire `self._lock` before touching `_ptr` or mutable state.
- **Lock ordering**: `Model` methods must release `self._lock` before calling `Project` methods (which acquire the project's lock) to prevent deadlocks. Use double-checked locking for caches: check cache with lock, compute without lock, write cache with lock.
- This locking is critical for free-threaded Python (PEP 703 / Python 3.13t+ / 3.14t) where the GIL does not serialize access.

## Testing

- Use `pytest` with `hypothesis` for property-based testing.
- Thread-safety tests live in `tests/test_thread_safety.py`.
- Run from `src/pysimlin`: `uv run pytest tests/ -x`
