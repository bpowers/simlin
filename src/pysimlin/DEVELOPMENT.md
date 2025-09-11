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
- `SimlinRuntimeError`: Simulation execution errors
- `SimlinCompilationError`: Model compilation errors

Error codes from C are mapped to Python ErrorCode enum.

## Building

### Prerequisites

1. Rust toolchain (for building libsimlin)
2. Python 3.9+ with pip
3. CFFI development headers
4. Platform-specific C compiler

### Local Development

```bash
# Build libsimlin
cd src/libsimlin
cargo build --release

# Install Python package in dev mode
cd src/pysimlin
pip install -e ".[dev]"

# Run tests
pytest tests/

# Type checking
mypy simlin

# Linting
ruff check simlin tests
black --check simlin tests
```

### Building Wheels

```bash
cd src/pysimlin
python scripts/build_wheels.py
```

This will:
1. Build libsimlin.a for current platform
2. Copy library to platform-specific directory
3. Build wheel with correct platform tag

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
pytest

# Specific test file
pytest tests/test_project.py

# With coverage
pytest --cov=simlin --cov-report=term-missing

# Verbose output
pytest -v

# Memory tests only
pytest tests/test_memory.py -v
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
pytest tests/test_memory.py -v

# Run specific memory test categories
pytest tests/test_memory.py::TestObjectCreationDestruction -v
pytest tests/test_memory.py::TestReferenceCountingEdgeCases -v
pytest tests/test_memory.py::TestFinalizerBehavior -v

# Run with garbage collection debugging
PYTHONMALLOC=debug pytest tests/test_memory.py -v
```

#### Automated Memory Testing (CI)

The GitHub Actions workflow `.github/workflows/memory.yml` provides comprehensive automated memory testing:

**AddressSanitizer (ASan) Testing:**
- Builds libsimlin with AddressSanitizer enabled (`-Z sanitizer=address`)
- Detects memory leaks, use-after-free, and buffer overflows
- Faster execution than Valgrind with better error reporting
- Primary memory testing approach

**Valgrind Testing:**
- Comprehensive memory error detection as fallback
- Uses custom suppression file (`valgrind-python.supp`) for Python internals
- Detects definite memory leaks while filtering false positives
- Broader platform compatibility

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

### Version Bumping

Update version in:
- `pyproject.toml`
- `simlin/__init__.py`

### Building for Release

1. **Local Testing**:
   ```bash
   make clean
   make build
   make test
   ```

2. **Build All Platform Wheels**:
   - Use GitHub Actions workflow
   - Or build on each platform manually

3. **Test Wheels**:
   ```bash
   pip install dist/simlin-*.whl
   python -c "import simlin; print(simlin.__version__)"
   ```

### Publishing to PyPI

1. **Test PyPI** (optional):
   ```bash
   twine upload --repository testpypi dist/*
   ```

2. **Production PyPI**:
   ```bash
   twine upload dist/*
   ```

Or use GitHub Actions workflow triggered by tags.

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
3. **Python Version**: Requires Python 3.9+ for type hint features

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
   - Install CFFI: `pip install cffi`
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