# simlin-cli

CLI tool for simulating and converting SD models. Primarily used for testing and debugging.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

- `src/main.rs` -- CLI entry point: clap derive-based argument parsing, model loading, simulation, format conversion. All compilation and simulation use the incremental salsa path (`SimlinDb` + `compile_project_incremental`); the monolithic `Project::from` path is not used.
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
- `--format <xmile|vensim|protobuf>` -- auto-detected from file extension when omitted
