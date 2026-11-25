# simlin-tests

The engine crates' black-box tests, benchmarks and instruments.
They are one package because they use every crate at once: a crate's own unit tests cannot, without depending on what is built on it.
Nothing depends on this package, and all of its dependencies are dev-dependencies.
See the root `CLAUDE.md` for the development rules and [Rust standards](/docs/dev/rust.md) for test time budgets and gates.

- `tests/integration/` -- the one integration harness: add a `mod` to `tests/integration/main.rs`, never a new `tests/*.rs` file.
  It holds the model corpus (`tests/integration/simulate.rs` and its neighbours run every corpus model on the VM and on the wasm backend), round trips of the file formats, Loops That Matter on whole models, and the VDF parity harness.
- `tests/vm_alloc.rs` -- that the VM's step loop allocates nothing; a binary of its own because it installs a counting global allocator.
- `benches/` -- Criterion benchmarks ([benchmarks.md](/docs/dev/benchmarks.md)).
- `examples/` -- instruments: programs that print a measurement or write a report for a person.
  Each lists the `instruments` feature, so a plain `cargo test` links none of them and `cargo clippy --all-targets --all-features` still checks them all; run one with `cargo run --release -p simlin-tests --features instruments --example <name>`.

No test here is gated on a feature: a gate that is off compiles its test away without a word (GH #925), so the package's dependencies ask for what the tests need.
The one exception is data a corpus model keeps in a spreadsheet, which needs `ext_data`; `scripts/gates.sh` turns it on.

Fixture paths are relative to this directory, which is where cargo runs the tests.
