# simlin-cli

CLI tool for simulating and converting SD models. Primarily used for testing and debugging.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

- `src/main.rs` -- CLI entry point: clap derive-based argument parsing, model loading, simulation, format conversion. All compilation and simulation use the incremental salsa path (`SimlinDb` + `compile_project_incremental`).
- `src/gen_stdlib.rs` -- Standard library generation utility (generates `stdlib.gen.rs` for simlin-engine)

## CLI Subcommands

Uses [clap](https://docs.rs/clap) derive API. Each subcommand declares exactly the arguments it accepts.

| Subcommand | Description | Key flags |
|---|---|---|
| `simulate` | Simulate a model, print TSV results | `--no-output`, `--ltm` |
| `convert` | Convert between XMILE, Vensim MDL, protobuf | `--to <FORMAT>`, `--model-only`, `--output` |
| `equations` | Print model equations as LaTeX | `--output` |
| `debug` | Compare simulation with a reference run | `--reference FILE`, `--ltm` |
| `gen-stdlib` | Generate Rust stdlib code | `--stdlib-dir`, `--output` |
| `vdf-dump` | Pretty-print VDF file contents | positional `PATH` |

Commands that read model files (`simulate`, `convert`, `equations`, `debug`) share `InputArgs` via `#[command(flatten)]`:
- Positional `PATH` (optional for `simulate`, reads stdin)
- `--format <xmile|vensim|protobuf|systems>` -- auto-detected from file extension when omitted (`.mdl` -> vensim, `.pb`/`.bin` -> protobuf, `.txt` -> systems, everything else -> xmile). Systems format output shows only non-infinite stocks in declaration order.

## Diagnostic reporting

Every diagnostic goes to **STDERR**, warnings included: STDOUT carries the TSV result table, and interleaving a diagnostic would corrupt a redirected or piped run (the same rule the MDL export warnings follow, #856). The severity is not signalled by the stream but by the message itself -- `simlin_engine::errors` words each summary line from the diagnostic's own `DiagnosticSeverity`, so `--ltm` on a conveyor model prints `warning in model 'main': ...conveyor_ltm_degraded...` while a genuine equation error prints `error in model 'main' variable 'x': ...` and a bad `<units>` declaration prints `units error in model ...` (GH #919). The word tracks severity, not the kind of diagnostic, so an advisory is always scannable as one.

Severity also gates behavior, not just wording: `FormattedErrors::push` raises `has_model_errors`/`has_variable_errors` for `Error`-severity diagnostics only, and `simulation_error_is_redundant` suppresses the generic `NotSimulatable` build failure only when a real model error was already printed. Before that gating, a model whose sole diagnostic was an advisory warning had its build failure silently swallowed -- the user saw "failed to create simulation" with no reason.

## External data resolution (Vensim `GET DIRECT *`)

A Vensim model opened from a **named path** resolves its `GET DIRECT *` (DATA, CONSTANTS, LOOKUPS, SUBSCRIPT) references against a `FilesystemDataProvider` rooted at the *model file's parent directory* (a bare filename roots at `.`), matching Vensim's relative-to-model resolution (`open_vensim_model` in `main.rs`). A model read from **stdin** (a pipe or `< file`) gets the null provider and any external-data reference surfaces the engine's "no DataProvider configured" error -- the provider is keyed on the path *argument* (the user's intent), NOT on an `is_file()` check of the `stdin` device sentinel, which under a `< model.mdl` redirection resolves to a regular file and would wrongly root a provider at the device's parent directory.
