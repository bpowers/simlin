.PHONY: all build test clean install dev lint type-check

# Build the libsimlin library
build-lib:
	cd ../libsimlin && cargo build --release

# Build the Python package
build: build-lib
	uv run python scripts/build_wheels.py

# Install for development
dev: build-lib
	uv sync --extra dev

# Run tests
test:
	uv run pytest tests/ -v

# Run linting
lint:
	uv run ruff check simlin tests
	uv run black --check simlin tests

# Run type checking
type-check:
	uv run mypy simlin

# Format code
format:
	uv run black simlin tests
	uv run ruff check --fix simlin tests

# Clean build artifacts
clean:
	rm -rf build dist *.egg-info .venv
	rm -rf simlin/_ffi.*.so simlin/_ffi.*.pyd
	rm -rf .pytest_cache .mypy_cache
	find . -type d -name __pycache__ -exec rm -rf {} +
	find . -type f -name "*.pyc" -delete

# Install the package
install: build
	uv pip install dist/*.whl

# Run all checks
check: lint type-check test

# Build documentation
docs:
	@echo "Documentation is in README.md"

all: build test
