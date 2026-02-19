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
| `cargo test` | Run all Rust tests |
| `pnpm test` | Run all TypeScript tests |
| `pnpm tsc` | TypeScript type checking |

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

Results are saved in `target/criterion/` with HTML reports. See [benchmarks.md](benchmarks.md) for profiling instructions.

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

### simlin-engine MDL equivalence tests

```bash
# Run MDL equivalence tests (requires xmutil feature)
cargo test -p simlin-engine --features xmutil test_mdl_equivalence -- --nocapture

# Run C-LEARN equivalence test (large model, ignored by default)
cargo test -p simlin-engine --features xmutil test_clearn_equivalence -- --ignored --nocapture
```

### pysimlin (Python)

```bash
cd src/pysimlin
uv run pytest tests/ -x           # Run tests
uv run ruff check                  # Lint
uv run ruff format                 # Format
uv run mypy simlin                 # Type check (strict)
```
