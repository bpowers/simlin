# simlin-cli

CLI tool for simulating and converting SD models. Primarily used for testing and debugging.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [doc/dev/commands.md](/doc/dev/commands.md).

## Key Files

- `src/main.rs` -- CLI entry point: argument parsing, model loading, simulation, format conversion
- `src/gen_stdlib.rs` -- Standard library generation utility (generates `stdlib.gen.rs` for simlin-engine)
