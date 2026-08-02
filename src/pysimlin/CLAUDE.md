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
- `simlin/_ffi.py` -- Low-level CFFI bindings, module-level `_finalizer_refs` with `_refs_lock`
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
