# pysimlin Development Guide

## Overview

pysimlin provides Python bindings to the libsimlin C library using CFFI. The package follows a layered architecture:

1. **C Library Layer** (`libsimlin`): Core simulation engine written in Rust, exposed as C API
2. **FFI Layer** (`_ffi.py`): Low-level CFFI bindings to the C API
3. **Python API Layer**: High-level Pythonic classes (Project, Model, Sim)
4. **Integration Layer**: pandas DataFrame support for results

## Architecture

### Memory Management

The package uses Python's reference counting with weakref finalizers for automatic cleanup:

- C objects are wrapped in Python classes
- `_register_finalizer()` sets up automatic cleanup when Python objects are garbage collected
- Manual ref/unref calls to C API maintain proper reference counts

### Type Safety

Full type hints are provided throughout:
- Runtime type checking with isinstance where needed
- mypy strict mode compliance
- TypedDict for structured data
- Protocol classes for interface definitions

### Error Handling

Comprehensive error handling with custom exception hierarchy:
- `SimlinError`: Base exception
- `SimlinImportError`: Model loading errors
- `SimlinRuntimeError`: Engine-rejected operations (bad edits, compile failures, simulation errors); carries per-variable diagnostics on `details`

Error codes from C are mapped to Python ErrorCode enum.

## Building

### Prerequisites

1. Rust toolchain (for building libsimlin)
2. Python 3.11+
3. [uv](https://docs.astral.sh/uv/) - Fast Python package manager
4. CFFI development headers
5. Platform-specific C compiler

Install uv:
```bash
# macOS/Linux
curl -LsSf https://astral.sh/uv/install.sh | sh
# or
brew install uv
```

### Local Development

```bash
# Build libsimlin
cd src/libsimlin
cargo build --release

# Install Python package in dev mode with uv (the dev extra includes the
# `notebook` extra -- anywidget -- because the widget tests drive the real
# ModelWidget; users get it with pip install "pysimlin[notebook]")
cd src/pysimlin
uv sync --extra dev

# Run tests
uv run pytest tests/

# Type checking
uv run mypy simlin

# Linting
uv run ruff check simlin tests
uv run black --check simlin tests
```

### Notebook widget assets

`Model.widget()` (the anywidget-based diagram editor; anywidget itself is the
optional `notebook` extra, see `pyproject.toml`) needs two build outputs of
the TypeScript workspace as package data in `simlin/_widget/`:

- `widget.js` -- `src/notebook-widget`'s single-file ES module (`dist/widget.js`)
- `libsimlin-browser.wasm` -- the engine from `src/engine/core/`, `wasm-opt`'d
  when binaryen is installed (`src/engine/build.sh` records `opt`/`raw` in a
  `.mode` sibling)

They are not committed (`simlin/_widget/.gitignore`). `scripts/stage_widget_assets.py`
is the ONE place that puts them there; everything else calls it:

```bash
cd src/pysimlin
make assets                                        # pnpm build of the widget + deps, then stage
python3 scripts/stage_widget_assets.py --no-build  # restage from the existing build outputs
make check-assets                                  # verify what is staged (exit 1 on a problem)
```

The notebook-widget package's `pnpm build` (and so the root `pnpm build` and the
pre-commit hook) also runs the `--no-build` staging after `rsbuild build`, so a
checkout that has built the frontend already has the assets in place. Beside
the two files the script writes `ASSETS.json` -- source commit, wasm-opt mode,
and each file's size and sha256 -- deterministically (no timestamps): a rerun
on an unchanged tree rewrites nothing, and every wheel built from one staging
carries byte-identical assets.

`setup.py` refuses `bdist_wheel` and `sdist` when either asset is missing or
empty or does not match `ASSETS.json` (a wheel that silently lacked the widget
would raise `SimlinAssetError` on every install), warns when the manifest's
commit is not `HEAD` or the wasm is not `wasm-opt`'d, and can be bypassed
deliberately with `SIMLIN_ALLOW_MISSING_WIDGET_ASSETS=1`. `uv sync` /
`build_ext --inplace` (the test path) are not guarded: the pysimlin tests run
without the widget assets.

### Building Wheels

```bash
cd src/pysimlin
uv run python scripts/build_wheels.py                    # everything from source
uv run python scripts/build_wheels.py --no-asset-build   # reuse the last frontend build
uv run python scripts/build_wheels.py --require-opt      # refuse a wasm that was not wasm-opt'd
```

`--require-opt` is off by default so a development machine without binaryen
still produces a wheel (with a `raw` wasm and a warning). The release
workflow does not run `build_wheels.py`; it passes `--require-opt` to
`scripts/stage_widget_assets.py` (its `widget-assets` job) and to
`scripts/check_wheel_assets.py`, so a raw wasm is refused before and after
cibuildwheel.

This will:
1. Build libsimlin.a for current platform (mimalloc feature)
2. Build and stage the notebook widget assets (above)
3. Build the wheel (`python -m build`) with the correct platform tag

The sdist (`python -m build --sdist`) includes the staged assets and
`scripts/stage_widget_assets.py`, so building from it needs no node; it is
not otherwise self-contained (the CFFI build reads `simlin.h` from the
libsimlin crate in the repo tree), and releases publish wheels only.

## Testing

### Test Structure

- `tests/conftest.py`: pytest fixtures and configuration
- `tests/test_project.py`: Project class tests
- `tests/test_model.py`: Model class tests
- `tests/test_sim.py`: Simulation and DataFrame tests
- `tests/test_errors.py`: Error handling tests
- `tests/test_analysis.py`: Analysis types tests
- `tests/test_memory.py`: Memory leak and stress tests

### Running Tests

```bash
# All tests
uv run pytest

# Specific test file
uv run pytest tests/test_project.py

# With coverage
uv run pytest --cov=simlin --cov-report=term-missing

# Verbose output
uv run pytest -v

# Memory tests only
uv run pytest tests/test_memory.py -v
```

### Memory Leak Testing

pysimlin includes comprehensive memory leak testing to ensure proper resource management in the C extension. The memory testing framework uses multiple approaches:

#### Test Coverage

The memory tests (`tests/test_memory.py`) cover:

1. **Object Creation/Destruction Stress Tests**
   - Rapid creation and destruction of Projects, Models, and Simulations
   - Nested object creation patterns
   - Large-scale object churn testing

2. **Reference Counting Edge Cases**
   - Circular reference prevention
   - Multiple references to same C objects
   - Parent-child object cleanup ordering
   - Exception handling during construction

3. **Finalizer Behavior Testing**
   - Proper finalizer registration and execution
   - Cleanup order verification
   - Garbage collection interaction

4. **Context Manager Cleanup**
   - Explicit cleanup in `__exit__` methods
   - Exception safety in context managers
   - Nested context manager scenarios

5. **Error Path Memory Safety**
   - Import errors with invalid data
   - Runtime errors during simulation
   - File not found scenarios
   - Corrupted data handling

#### Local Memory Testing

```bash
# Run all memory tests
uv run pytest tests/test_memory.py -v

# Run specific memory test categories
uv run pytest tests/test_memory.py::TestObjectCreationDestruction -v
uv run pytest tests/test_memory.py::TestReferenceCountingEdgeCases -v
uv run pytest tests/test_memory.py::TestFinalizerBehavior -v

# Run with garbage collection debugging
PYTHONMALLOC=debug uv run pytest tests/test_memory.py -v
```

#### Automated Memory Testing (CI)

The GitHub Actions workflow `.github/workflows/memory.yml` provides comprehensive automated memory testing:

**AddressSanitizer (ASan) Testing:**
- Builds libsimlin with AddressSanitizer enabled (`-Z sanitizer=address`)
- Detects memory leaks, use-after-free, and buffer overflows
- Faster execution than Valgrind with better error reporting
- Primary memory testing approach

**Valgrind Testing (local, not in CI):**
- Comprehensive memory error detection as fallback
- Uses custom suppression file (`valgrind-python.supp`) for Python internals
- Detects definite memory leaks while filtering false positives
- Broader platform compatibility
- Run valgrind on the venv's `python` directly (not on `uv run`, which only
  spawns it -- valgrind would trace `uv` and finish in a second having seen
  nothing), with `PYTHONMALLOC=malloc` so allocations are visible to it:

```bash
cd src/pysimlin
PYTHONMALLOC=malloc valgrind --leak-check=full --show-leak-kinds=definite \
  --errors-for-leak-kinds=definite --suppressions=valgrind-python.supp \
  --log-file=valgrind.log \
  .venv/bin/python -m pytest -q --no-cov -p no:cacheprovider tests/test_ffi.py
grep -E "definitely lost|ERROR SUMMARY" valgrind.log
```

**macOS Testing:**
- Uses native macOS `leaks` tool when available
- Tests on Apple Silicon platform
- Validates memory behavior on different architectures

#### Memory Testing

**Using AddressSanitizer:**
```bash
# Build libsimlin with AddressSanitizer
cd src/libsimlin
RUSTFLAGS="-Z sanitizer=address" cargo +nightly build --release

# Run memory tests
cd src/pysimlin
ASAN_OPTIONS="detect_leaks=1:abort_on_error=1" PYTHONMALLOC=malloc \
  python -m pytest tests/test_memory.py
```

#### Memory Testing Best Practices

1. **Test Design:**
   - Create and destroy many objects to amplify leaks
   - Test error paths and exception scenarios
   - Verify cleanup in different object destruction orders
   - Use weak references to verify garbage collection

2. **CI Integration:**
   - Run memory tests on every pull request
   - Use AddressSanitizer for comprehensive detection
   - Fail CI on any memory safety issues

3. **Debugging Memory Issues:**
   - Use ASan to detect leaks, use-after-free, and buffer overflows
   - Check finalizer registration in `_finalizer_refs`
   - Verify C pointer cleanup in context managers

#### Common Memory Issues

1. **Missing Finalizers:**
   - Symptom: Objects not cleaned up after garbage collection
   - Solution: Ensure `_register_finalizer()` is called in `__init__`

2. **Double Free:**
   - Symptom: Crashes or ASan errors on cleanup
   - Solution: Check `_ptr != ffi.NULL` before cleanup calls

3. **Reference Cycles:**
   - Symptom: Objects not garbage collected
   - Solution: Use weak references or explicit cleanup

4. **Exception Path Leaks:**
   - Symptom: Memory leaks when errors occur
   - Solution: Test error scenarios and ensure cleanup

The memory testing framework ensures that pysimlin properly manages C resources and prevents memory leaks in production use.

## Release Process

### Versioning

There is no version to bump by hand: `pyproject.toml` declares
`dynamic = ["version"]` and setuptools-scm derives it from the
`pysimlin-v<version>` git tag (`tag_regex` in `[tool.setuptools_scm]`).
`simlin.__version__` reads that back out of the installed distribution's
metadata.

### Cutting a release

1. **Dry-run the release workflow.** `.github/workflows/release.yml` also
   accepts `workflow_dispatch`, and its `publish` job is gated on
   `refs/tags/`, so a manual run exercises the whole pipeline -- widget
   assets, cibuildwheel, and the wheel test matrix -- without uploading
   anything:

   ```bash
   gh workflow run release.yml --ref main
   ```

   This is worth the ~45 minutes. The `test-wheels` matrix runs the suite
   against an *installed wheel* in an environment that is not the dev venv
   (no `uv sync`, whatever `actions/setup-python` happens to bundle), so it
   can fail on a tree where `make test` is green.

2. **Tag.** `scripts/release-pysimlin.sh <version>` builds and stages the
   widget assets as a preflight (`SKIP_ASSET_PREFLIGHT=1` opts out), creates
   the `pysimlin-v<version>` tag on the current commit, then commits the
   `src/simlin-mcp/pysimlin.version` bump on top. The tag has to precede that
   commit: simlin-mcp's `pysimlin_version_matches_latest_tag` test requires
   the matching tag to exist, and it is the tagged commit that setuptools-scm
   turns into the wheel's version.

3. **Push.** `git push origin main pysimlin-v<version>`. The tag triggers
   `release.yml`; this time `publish` runs and uploads `wheelhouse/*.whl` to
   PyPI with `pypa/gh-action-pypi-publish`.

### What the workflow does

- `widget-assets` builds the notebook widget ONCE on ubuntu
  (`stage_widget_assets.py --require-opt`) and uploads `widget.js`,
  `libsimlin-browser.wasm`, and `ASSETS.json`. Every `build-wheels` runner
  downloads that one artifact into `simlin/_widget/` and re-verifies it
  before cibuildwheel, so all platform wheels carry byte-identical assets.
- `test-wheels` runs `scripts/check_wheel_assets.py` over the wheels
  (present, non-empty, manifest-consistent, identical across wheels) and
  `--installed` against the installed wheel
  (`Project.new().main_model.widget()` constructs), then the pytest suite
  from the checkout against that wheel.

### Checking a wheel by hand

```bash
python3 scripts/check_wheel_assets.py dist/pysimlin-*.whl
uv pip install dist/pysimlin-*.whl
python -c "import simlin; print(simlin.__version__)"
```

## Platform Support

### macOS ARM64
- Built on macOS 14+
- Platform tag: `macosx_11_0_arm64`
- Requires Apple Silicon Mac

### Linux x86_64
- Built on Ubuntu 22.04+
- Platform tag: `manylinux_2_28_x86_64`
- Compatible with most modern Linux distributions

### Linux ARM64
- Built on Ubuntu 22.04+ with QEMU
- Platform tag: `manylinux_2_28_aarch64`
- For ARM servers and embedded systems

## API Design Principles

1. **Pythonic Interface**: Follow Python conventions (snake_case, properties, context managers)
2. **Type Safety**: Full type hints for IDE support and static analysis
3. **DataFrame Integration**: Return simulation results as pandas DataFrames
4. **Error Clarity**: Clear exception messages with context
5. **Memory Safety**: Automatic cleanup with no manual memory management required

## Known Limitations

1. **Variable Discovery**: Currently requires passing variable list to `get_results()` - C API enhancement needed for automatic discovery
2. **Platform Support**: Limited to macOS ARM64 and Linux (x86_64, ARM64)
3. **Python Version**: Requires Python 3.11+

## Future Enhancements

1. **Windows Support**: Add Windows wheel building
2. **Variable Discovery**: Enhance C API to get variables from Sim
3. **Async Support**: Add async simulation execution
4. **Streaming Results**: Support for streaming large simulations
5. **Model Editing**: Add support for modifying model structure

## Debugging

### Common Issues

1. **Library Not Found**:
   - Ensure libsimlin is built: `cargo build --release`
   - Check library path in `_ffi_build.py`

2. **CFFI Build Errors**:
   - Install CFFI: `uv pip install cffi`
   - Check compiler is available

3. **Import Errors**:
   - Rebuild CFFI module: `python simlin/_ffi_build.py`
   - Check Python path includes package directory

### Debug Build

For debugging with symbols:
```bash
cd src/libsimlin
cargo build  # Debug build without --release
```

Then update `_ffi_build.py` to use debug library path.

## Contributing

1. Follow existing code style (black, ruff)
2. Add tests for new features
3. Update type hints
4. Run full test suite before submitting
5. Update documentation as needed