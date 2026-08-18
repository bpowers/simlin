# pysimlin

Python bindings for the simulation engine via CFFI. Exposes full engine functionality in idiomatic Python, targeting AI agents for model analysis, calibration, etc.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).
For Python-specific standards (thread safety, lock ordering), see [docs/dev/python.md](/docs/dev/python.md).

## Key Files

- `simlin/project.py` -- `Project` wrapper class (main API)
- `simlin/model.py` -- `Model` wrapper class
- `simlin/sim.py` -- `Sim` wrapper class (simulation runner)
- `simlin/run.py` -- High-level run utilities
- `simlin/vdf.py` -- `simlin.load_vdf(path)`: import a Vensim VDF binary output file (run, sensitivity, or dataset container, auto-detected by magic) as a `Run.results`-shaped DataFrame (time index, canonical column names, `"var[element]"` arrayed columns). A function returning a plain DataFrame, not a class: a VDF carries only named series, exactly the `Run.results` surface, and no loop/override metadata that would justify a wrapper object. Malformed files raise `SimlinRuntimeError` (the engine's VDF readers are total on arbitrary bytes; see the sweep test in [/src/simlin-engine/src/vdf.rs](/src/simlin-engine/src/vdf.rs))
- `simlin/_ffi.py` -- Low-level CFFI bindings, module-level `_finalizer_refs` with `_refs_lock`. Besides the per-call helpers (`apply_patch_json`, `serialize_json`, `render_svg`/`render_png`, ...) it carries the three primitives a file-backed project is built on: `serialize_mdl(ptr) -> (bytes, list[ErrorDetail])` (Vensim export; lossiness warnings are returned, not raised, mirroring `apply_patch_json`'s collected diagnostics), `replace_contents(dst_ptr, src_ptr)` (in-place reload: `Model` objects already obtained from `dst` stay valid and observe the new contents; open the new bytes into a scratch project with any format and replace from it), and `diagram_sync(ptr, model_name, patch_json=None)` (a non-None applied patch selects the engine's incremental layout, which keeps existing element positions; None is a full relayout, which is what `Project.auto_layout` asks for). Tests: `tests/test_ffi.py`; the reload/serialize churn under ASan lives in `tests/test_memory.py`
- `simlin/_ffi_build.py` -- CFFI build configuration
- `simlin/types.py` -- The unified public variable types (`Stock`/`Flow`/`Aux`/`Module` + `Compat`/`ElementEquation`/`GraphicalFunction`), frozen dataclasses used for BOTH reading (`Model.get_variable`, `edit()`'s `current` dict) and writing (`patch.upsert`); edit by `dataclasses.replace` + `upsert`. There is deliberately no `uid` field (the engine preserves/mints uids on upsert, see `patch.rs::upsert_variable`); everything beyond a variable's core surface -- ACTIVE INITIAL, non-negativity, conveyor/queue markers, data sources -- lives in `Compat`, mirroring the engine's own Compat
- `simlin/json_types.py` -- Wire/patch-only dataclasses (ops, views, sim specs, project structure); variables are the unified types above
- `simlin/json_converter.py` -- Bidirectional unified-type <-> engine wire JSON mapping (`structure_variable`/`unstructure_variable`) plus cattrs config for the wire types; the variable mapping is hand-written on purpose (arrayedEquation folding, legacy-compat merging, `""`-vs-`None` normalization)
- `simlin/analysis.py` -- Model analysis functions
- `simlin/errors.py` -- Error types

## Thread Safety

All wrapper classes have a per-instance lock (`Sim`'s is an `RLock` so `get_run()` can hold it across the whole Run snapshot). Lock ordering: release `Model._lock` before calling `Project` methods. See [docs/dev/python.md](/docs/dev/python.md).

## Non-Standard Commands

```bash
cd src/pysimlin
uv run pytest tests/ -x     # Run tests
uv run ruff check            # Lint
uv run ruff format           # Format
uv run mypy simlin           # Type check (strict)
```
