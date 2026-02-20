# Documentation Index

## Architecture and Design

- [architecture.md](architecture.md) -- Component descriptions, dependency graph, project structure
- [design/ltm--loops-that-matter.md](design/ltm--loops-that-matter.md) -- LTM implementation design: data structures, synthetic variables, module handling
- [design/mdl-parser.md](design/mdl-parser.md) -- Vensim MDL parser design history and implementation notes
- [design/vdf.md](design/vdf.md) -- VDF binary format reverse-engineering and parser design

## Development Standards

- [dev/commands.md](dev/commands.md) -- Build, test, lint commands for all languages
- [dev/benchmarks.md](dev/benchmarks.md) -- Running and profiling benchmarks (criterion, valgrind, perf, gperftools)
- [dev/rust.md](dev/rust.md) -- Rust development standards
- [dev/typescript.md](dev/typescript.md) -- TypeScript/React development standards
- [dev/python.md](dev/python.md) -- Python (pysimlin) development standards
- [dev/workflow.md](dev/workflow.md) -- Problem-solving philosophy and TDD workflow

## Project Management

- [tech-debt.md](tech-debt.md) -- Known technical debt items with measurement commands
- [plans/](plans/README.md) -- Implementation plans (active and completed)

## Domain Knowledge

- [reference/xmile-v1.0.html](reference/xmile-v1.0.html) -- XMILE interchange format specification
- [reference/ltm--loops-that-matter.md](reference/ltm--loops-that-matter.md) -- Loops That Matter technique: link scores, loop scores, algorithm reference
- [array-design.md](array-design.md) -- Array/subscript design notes

## Research Papers

Each paper has a detailed markdown summary -- consult these first for paper-specific details before the PDFs.

- [reference/papers/eberlein2020-finding-the-loops-that-matter--summary.md](reference/papers/eberlein2020-finding-the-loops-that-matter--summary.md) ([PDF](reference/papers/eberlein2020-finding-the-loops-that-matter.pdf))
- [reference/papers/schoenberg2020-loops-that-matter--summary.md](reference/papers/schoenberg2020-loops-that-matter--summary.md) ([PDF](reference/papers/schoenberg2020-loops-that-matter.pdf))
- [reference/papers/schoenberg2020.1-seamlessly-integrating-ltm--summary.md](reference/papers/schoenberg2020.1-seamlessly-integrating-ltm--summary.md) ([PDF](reference/papers/schoenberg2020.1-seamlessly-integrating-ltm.pdf))
- [reference/papers/schoenberg2020.2-thesis--summary.md](reference/papers/schoenberg2020.2-thesis--summary.md) ([PDF](reference/papers/schoenberg2020.2-thesis.pdf)) -- PhD thesis encompassing all LTM articles plus LoopX visualization and FSNN causal inference
- [reference/papers/schoenberg2023-improving-loops-that-matter--summary.md](reference/papers/schoenberg2023-improving-loops-that-matter--summary.md) ([PDF](reference/papers/schoenberg2023-improving-loops-that-matter.pdf))

## Schemas

- [simlin-project.schema.json](simlin-project.schema.json) -- Simlin project JSON schema
- [sdai-model.schema.json](sdai-model.schema.json) -- AI metadata augmentation schema

## Other

- [population-model.png](population-model.png) -- Population model diagram
