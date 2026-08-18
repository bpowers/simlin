# pysimlin

Python bindings for the simulation engine via CFFI. Exposes full engine functionality in idiomatic Python, targeting AI agents for model analysis, calibration, etc.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).
For Python-specific standards (thread safety, lock ordering), see [docs/dev/python.md](/docs/dev/python.md).

## Key Files

- `simlin/project.py` -- `Project` wrapper class (main API). Also the imperative shell for file-backed projects: `Project.open()` / `simlin.open()`, `path`/`format`/`revision`/`dirty`/`autosave`, `save()`/`save_as()`/`reload()`/`watch()`/`on_change()`. Every accepted mutation funnels through `_apply_patch_json` (or `auto_layout`) into `_commit_change_locked` (bump revision, invalidate model caches, autosave) followed by `_notify` AFTER `_file_lock` is released -- new mutation paths must go through it, not around it, and must never notify under the lock. `_apply_patch_json(expected_revision=)` rejects an `edit()` whose base revision moved; `_write_to` refuses to overwrite a file another tool changed (`save(force=True)` overrides); the poll thread's delivery is re-verified against the current watcher/path/bytes in `_ingest_disk_bytes`. A file-backed project also runs the engine's incremental layout after variable ops (mirroring simlin-mcp-core's `edit_model`); in-memory projects deliberately do not persist a layout until `auto_layout()`
- `simlin/_formats.py` -- `FileFormat` and the single suffix/content -> format table shared by `load()`, `open()`, `save()`, `save_as()` (mirrors simlin-serve `format_for_path` + simlin-mcp-core JSON key sniffing). Add formats here only
- `simlin/_sync.py` -- pure sync state machine (`decide(state, event)`) for local edits, widget snapshots, disk observations, and explicit reloads; the tests enumerate every arm. `ChangeEvent`/`ChangeSource` live here
- `simlin/_disk.py` -- `content_hash`, mode-preserving `atomic_write` (tempfile + rename), and the stdlib polling `FileWatcher` (deliberately not inotify/watchfiles: one file, all filesystems incl. Colab FUSE, no native wheel)
- `simlin/diagram.py` -- `Diagram`, the `_repr_svg_` value returned by `Model.diagram()`
- `simlin/model.py` -- `Model` wrapper class; proxies `path`/`revision`/`dirty`/`save()`/`reload()` to its project, holds `selection` (set by the widget layer), and `diagram()` / `_svg_mimebundle()` (the static seam the widget's `_repr_mimebundle_` falls back to)
- `simlin/sim.py` -- `Sim` wrapper class (simulation runner)
- `simlin/run.py` -- High-level run utilities
- `simlin/vdf.py` -- `simlin.load_vdf(path)`: import a Vensim VDF binary output file (run, sensitivity, or dataset container, auto-detected by magic) as a `Run.results`-shaped DataFrame (time index, canonical column names, `"var[element]"` arrayed columns). A function returning a plain DataFrame, not a class: a VDF carries only named series, exactly the `Run.results` surface, and no loop/override metadata that would justify a wrapper object. Malformed files raise `SimlinRuntimeError` (the engine's VDF readers are total on arbitrary bytes; see the sweep test in [/src/simlin-engine/src/vdf.rs](/src/simlin-engine/src/vdf.rs))
- `simlin/_ffi.py` -- Low-level CFFI bindings, module-level `_finalizer_refs` with `_refs_lock`. Besides the per-call helpers (`apply_patch_json`, `serialize_json`, `render_svg`/`render_png`, ...) it carries the three primitives that reloading a project from disk is built on: `serialize_mdl(ptr) -> (bytes, list[ErrorDetail])` (Vensim export; lossiness warnings are returned, not raised, mirroring `apply_patch_json`'s collected diagnostics), `replace_contents(dst_ptr, src_ptr)` (in-place reload: `Model` objects already obtained from `dst` stay valid and observe the new contents, but the caller must run `Model._invalidate_caches()` on them; open the new bytes into a scratch project with any format and replace from it), and `diagram_sync(ptr, model_name, patch_json=None)` (a non-None applied patch selects the engine's incremental layout, which keeps existing element positions; None is a full relayout, which is what `Project.auto_layout` asks for). Tests: `tests/test_ffi.py`; the reload/serialize churn under ASan lives in `tests/test_memory.py`
- `simlin/_ffi_build.py` -- CFFI build configuration
- `simlin/types.py` -- The unified public variable types (`Stock`/`Flow`/`Aux`/`Module` + `Compat`/`ElementEquation`/`GraphicalFunction`), frozen dataclasses used for BOTH reading (`Model.get_variable`, `edit()`'s `current` dict) and writing (`patch.upsert`); edit by `dataclasses.replace` + `upsert`. There is deliberately no `uid` field (the engine preserves/mints uids on upsert, see `patch.rs::upsert_variable`); everything beyond a variable's core surface -- ACTIVE INITIAL, non-negativity, conveyor/queue markers, data sources -- lives in `Compat`, mirroring the engine's own Compat
- `simlin/json_types.py` -- Wire/patch-only dataclasses (ops, views, sim specs, project structure); variables are the unified types above
- `simlin/json_converter.py` -- Bidirectional unified-type <-> engine wire JSON mapping (`structure_variable`/`unstructure_variable`) plus cattrs config for the wire types; the variable mapping is hand-written on purpose (arrayedEquation folding, legacy-compat merging, `""`-vs-`None` normalization)
- `simlin/analysis.py` -- Model analysis functions
- `simlin/errors.py` -- Error types

## Thread Safety

All wrapper classes have a per-instance lock (`Sim`'s is an `RLock` so `get_run()` can hold it across the whole Run snapshot). `Project` has a second `RLock`, `_file_lock`, for file-backing state and whole mutate-then-write sequences. Lock ordering: `Project._file_lock` -> `Project._lock` -> `Model._lock`; release `Model._lock` before calling `Project` methods; change callbacks fire with no lock held. See [docs/dev/python.md](/docs/dev/python.md).

## Non-Standard Commands

```bash
cd src/pysimlin
uv run pytest tests/ -x     # Run tests
uv run ruff check            # Lint
uv run ruff format           # Format
uv run mypy simlin           # Type check (strict)
```
