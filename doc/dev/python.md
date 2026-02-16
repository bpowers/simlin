# Python (pysimlin) Development Standards

## Code Style

- Use `ruff` for both linting and formatting (replaces black). Run `ruff check` and `ruff format`.
- Use `mypy` with strict mode (`mypy simlin`).
- Target Python 3.11+. Use modern type syntax (`list[str]`, `dict[str, int]`, `X | None`) and `from __future__ import annotations` in all source files.

## Thread Safety

- **All wrapper classes** (`Project`, `Model`, `Sim`) have a per-instance `threading.Lock` (`self._lock`) that protects `_ptr` and cached state.
- **Module-level `_finalizer_refs`** (a `WeakValueDictionary`) is protected by `_refs_lock` in `_ffi.py`.
- When adding new methods to wrapper classes, always acquire `self._lock` before touching `_ptr` or mutable state.
- **Lock ordering**: `Model` methods must release `self._lock` before calling `Project` methods (which acquire the project's lock) to prevent deadlocks. Use double-checked locking for caches: check cache with lock, compute without lock, write cache with lock.
- This locking is critical for free-threaded Python (PEP 703 / Python 3.13t+ / 3.14t) where the GIL does not serialize access.

## Testing

- Use `pytest` with `hypothesis` for property-based testing.
- Thread-safety tests live in `tests/test_thread_safety.py`.
- Run from `src/pysimlin`: `uv run pytest tests/ -x`
