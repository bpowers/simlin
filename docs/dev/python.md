# Python (pysimlin) Development Standards

## Code Style

- Use `ruff` for both linting and formatting (replaces black). Run `ruff check` and `ruff format`.
- Use `mypy` with strict mode (`mypy simlin`).
- Target Python 3.11+. Use modern type syntax (`list[str]`, `dict[str, int]`, `X | None`) and `from __future__ import annotations` in all source files.

## Thread Safety

- **All wrapper classes** (`Project`, `Model`, `Sim`) have a per-instance lock (`self._lock`) that protects `_ptr` and cached state. `Project` and `Model` use `threading.Lock`; `Sim` uses `threading.RLock` so `Sim.get_run()` can hold the lock across the whole Run snapshot (which re-enters the individually locked accessors on the same thread) -- without that outer hold, a concurrent `run_to`/`reset`/`close` could land between the snapshot's reads and tear it.
- **Module-level `_finalizer_refs`** (a `WeakValueDictionary`) is protected by `_refs_lock` in `_ffi.py`.
- When adding new methods to wrapper classes, always acquire `self._lock` before touching `_ptr` or mutable state.
- **`Project._file_lock`** (an `RLock`) guards a project's file-backing state (path, format, sync state, listeners, registered models) and serialises whole mutate-then-write sequences (`edit()` commit, autosave, `reload()`, the poll thread's in-place reload) so they cannot interleave. It is re-entrant because `_apply_patch_json` commits through `_commit_change_locked` while still holding it.
- **Lock ordering**: `Project._file_lock` -> `Project._lock` -> `Model._lock`. `Model` methods must release `self._lock` before calling `Project` methods (which acquire the project's locks) to prevent deadlocks; nothing may take `_file_lock` while holding `_lock`. `Project._invalidate_model_caches` takes each `Model._lock` briefly under `_file_lock`, which is why a `Model` must never call back into its project while holding its own lock. Change callbacks (`Project.on_change`) always run with no project lock held, and stopping the poll thread (`FileWatcher.stop()`, which joins) is done after releasing `_file_lock` because the thread may be blocked on it. Use double-checked locking for caches: check cache with lock, compute without lock, write cache with lock.
- **`ModelWidget` (simlin/widget.py) owns one lock, `_state_lock`.** ipywidgets/traitlets state is not thread-safe, and the kernel does not promise one thread: ipykernel 7 (JupyterLab 4.4+) runs comm handlers on a subshell thread while a cell executes, so a browser snapshot is applied, written, and pushed there, while `Project.on_change` notifications -- subscribed with `dispatch=shell.kernel.io_loop.add_callback` under ipykernel, direct outside one (scripts, tests) -- are delivered on the IO loop's thread. `_state_lock` (an `RLock`) serialises the widget's two pieces of cross-thread state: the trait push (`_push`, one `hold_sync()` per `(project_json, revision)` pair, so two pushes can never send a torn pair) and the set of own pending revisions. It sits BELOW every Project lock in the order (taken with none held) and is never held around a comm `send`. A widget reads the project's `(contents, revision)` pair through `Project._snapshot()`, which takes `_file_lock` around both reads, and it never holds a project lock while sending on the comm.
- This locking is critical for free-threaded Python (PEP 703 / Python 3.13t+ / 3.14t) where the GIL does not serialize access.

## Testing

- Use `pytest` with `hypothesis` for property-based testing.
- Thread-safety tests live in `tests/test_thread_safety.py`.
- Run from `src/pysimlin`: `uv run pytest tests/ -x`
