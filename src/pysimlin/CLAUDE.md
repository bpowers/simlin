# pysimlin

Python bindings for the simulation engine via CFFI. Exposes full engine functionality in idiomatic Python, targeting AI agents for model analysis, calibration, etc.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [doc/dev/commands.md](/doc/dev/commands.md).
For Python-specific standards (thread safety, lock ordering), see [doc/dev/python.md](/doc/dev/python.md).

## Key Files

- `simlin/project.py` -- `Project` wrapper class (main API)
- `simlin/model.py` -- `Model` wrapper class
- `simlin/sim.py` -- `Sim` wrapper class (simulation runner)
- `simlin/run.py` -- High-level run utilities
- `simlin/_ffi.py` -- Low-level CFFI bindings, module-level `_finalizer_refs` with `_refs_lock`
- `simlin/_ffi_build.py` -- CFFI build configuration
- `simlin/types.py` -- Type definitions and protocols
- `simlin/analysis.py` -- Model analysis functions
- `simlin/errors.py` -- Error types

## Thread Safety

All wrapper classes have per-instance `threading.Lock`. Lock ordering: release `Model._lock` before calling `Project` methods. See [doc/dev/python.md](/doc/dev/python.md).

## Non-Standard Commands

```bash
cd src/pysimlin
uv run pytest tests/ -x     # Run tests
uv run ruff check            # Lint
uv run ruff format           # Format
uv run mypy simlin           # Type check (strict)
```
