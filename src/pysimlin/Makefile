.PHONY: all build test clean install dev lint type-check

# Build the libsimlin library
build-lib:
	cd ../libsimlin && cargo build --release

# Build the Python package
build: build-lib
        python scripts/build_wheels.py

# Regenerate protobuf bindings
protos:
	mkdir -p simlin/_generated
	protoc -I../simlin-engine/src --python_out=simlin/_generated ../simlin-engine/src/project_io.proto

# Install for development
dev: build-lib
        pip install -e ".[dev]"

# Run tests
test:
	pytest tests/ -v

# Run linting
lint:
	ruff check simlin tests
	black --check simlin tests

# Run type checking
type-check:
	mypy simlin

# Format code
format:
	black simlin tests
	ruff check --fix simlin tests

# Clean build artifacts
clean:
	rm -rf build dist *.egg-info
	rm -rf simlin/_ffi.*.so simlin/_ffi.*.pyd
	rm -rf .pytest_cache .mypy_cache
	find . -type d -name __pycache__ -exec rm -rf {} +
	find . -type f -name "*.pyc" -delete

# Install the package
install: build
	pip install dist/*.whl

# Run all checks
check: lint type-check test

# Build documentation
docs:
	@echo "Documentation is in README.md"

all: build test