.PHONY: all build test clean install dev lint type-check assets check-assets

# Build the libsimlin library. mimalloc: the engine compile path is
# allocation-heavy and mimalloc roughly halves allocator time on native builds
# (see docs/design/engine-performance.md).
build-lib:
	cd ../libsimlin && cargo build --release --features mimalloc

# Stage the notebook widget's assets (widget.js + libsimlin-browser.wasm +
# ASSETS.json) into simlin/_widget/. Builds the TypeScript workspace first;
# `python scripts/stage_widget_assets.py --no-build` restages without it.
assets:
	python3 scripts/stage_widget_assets.py

# Verify the staged assets are complete and match their manifest.
check-assets:
	python3 scripts/stage_widget_assets.py --check

# Build the Python package (build_wheels.py stages the widget assets itself)
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
	uv run ruff format --check simlin tests

# Run type checking
type-check:
	uv run mypy simlin

# Format code
format:
	uv run ruff format simlin tests
	uv run ruff check --fix simlin tests

# Clean build artifacts
clean:
	rm -rf build dist *.egg-info .venv
	rm -rf simlin/_clib.*.so simlin/_clib.*.pyd
	rm -f simlin/_widget/widget.js simlin/_widget/libsimlin-browser.wasm simlin/_widget/ASSETS.json
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
