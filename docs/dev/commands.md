# Build, Test, and Lint Commands

## Global Setup

Run at the start of every session:

```bash
./scripts/dev-init.sh
```

## Build

| Command | Description |
|---------|-------------|
| `pnpm build` | Build web app + WASM engine (full stack) |
| `cargo build` | Build Rust components only |
| `pnpm clean` | Clean all build artifacts |
| `pnpm format` | Format both TypeScript/JavaScript and Rust |

## Lint

| Command | Description |
|---------|-------------|
| `pnpm lint` | Lint Rust (clippy) + TypeScript/JavaScript (eslint) |
| `cargo clippy --all-targets --all-features -- -D warnings` | Rust linting only |
| `cargo fmt --check` | Rust format check |

## Test

| Command | Description |
|---------|-------------|
| `cargo test` | Run all Rust tests but the gates |
| `scripts/gates.sh` | Run the gates: the `#[ignore]`d tests, and the tests of the `ext_data` feature, optimized (`scripts/gates.sh <filter>` for some of them) |
| `pnpm test` | Run all TypeScript tests |
| `pnpm tsc` | TypeScript type checking |

`cargo test --workspace` runs under a 3-minute wall-clock cap in both the pre-commit hook (`timeout(1)`) and CI (GitHub Actions `timeout-minutes: 3`).
A test too heavy for that suite is a gate: `#[ignore = "<what it sweeps>; run under the gates profile"]`, run by `scripts/gates.sh` and by CI's `gates` job on every push to main and every pull request to main.
See [rust.md](rust.md#test-time-budgets) for the budget and for what makes a test a gate.

## Code Coverage

| Command | Description |
|---------|-------------|
| `cargo llvm-cov` | Rust code coverage (LLVM source-based) |
| `cargo llvm-cov --html` | HTML coverage report in `target/llvm-cov/html/` |

Install: `cargo install cargo-llvm-cov`

## Benchmarks

| Command | Description |
|---------|-------------|
| `cargo bench -p simlin-engine` | Run all Rust benchmarks |
| `cargo bench -p simlin-engine --bench compiler` | Compiler pipeline benchmarks (real models) |
| `cargo bench -p simlin-engine --bench simulation` | Simulation/VM benchmarks |
| `cargo bench -p simlin-engine --bench array_ops` | Array operation benchmarks |

Results are saved in `target/criterion/` with HTML reports.
See [benchmarks.md](benchmarks.md) for profiling instructions.

## Generated Files

| Command | Description |
|---------|-------------|
| `pnpm build:gen-protobufs` | Regenerate protobuf bindings (TypeScript + Rust) |
| `cbindgen --config src/libsimlin/cbindgen.toml --crate simlin --output src/libsimlin/simlin.h` | Regenerate C header from FFI exports |

## Component-Specific Commands

### simlin-engine (Rust)

```bash
cargo test -p simlin-engine              # Engine tests only
cargo test -p simlin-engine mdl::        # MDL parser tests
```

### pysimlin (Python)

```bash
cd src/pysimlin
uv run pytest tests/ -x           # Run tests
uv run ruff check                  # Lint
uv run ruff format                 # Format
uv run mypy simlin                 # Type check (strict)
make assets                        # Build + stage the notebook widget assets (widget.js + wasm)
make check-assets                  # Verify the staged widget assets
make e2e                           # JupyterLab notebook-editor journey (Playwright)
make export-check                  # nbconvert the example notebook (widget view with state, SVG without)
uv run python scripts/build_wheels.py   # Build the wheel (libsimlin + widget assets + CFFI)
```

`make e2e` launches a real `jupyter lab` from the pysimlin venv (synced with the
`e2e` extra) and drives the notebook editor widget end to end.
It is its own CI
job (`pysimlin-e2e`), not part of the pre-commit hook.
It needs the widget
assets staged into `simlin/_widget/` (`make assets`) and Playwright's chromium
(`npx playwright install --with-deps chromium`).
