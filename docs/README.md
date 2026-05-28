# Documentation Index

## Architecture and Design

- [architecture.md](architecture.md) -- Component descriptions, dependency graph, project structure
- [design/2026-02-21-incremental-compilation.md](design/2026-02-21-incremental-compilation.md) -- Incremental compilation via salsa: symbolic bytecode, per-variable tracking, LTM integration
- [design/engine-performance.md](design/engine-performance.md) -- Engine compile/simulate profile (C-LEARN), implemented optimizations, and remaining proposals
- [design/ltm--loops-that-matter.md](design/ltm--loops-that-matter.md) -- LTM implementation design: data structures, synthetic variables, module handling
- [design/mdl-parser.md](design/mdl-parser.md) -- Vensim MDL parser design history and implementation notes
- [design/vdf.md](design/vdf.md) -- VDF binary format specification and parser design

## Development Standards

- [dev/commands.md](dev/commands.md) -- Build, test, lint commands for all languages
- [dev/deploy.md](dev/deploy.md) -- Deploying the web app to Google App Engine: runbook, env vars, smoke test, rollback
- [dev/benchmarks.md](dev/benchmarks.md) -- Running and profiling benchmarks (criterion, valgrind, perf, gperftools)
- [dev/rust.md](dev/rust.md) -- Rust development standards
- [dev/typescript.md](dev/typescript.md) -- TypeScript/React development standards
- [dev/python.md](dev/python.md) -- Python (pysimlin) development standards
- [dev/workflow.md](dev/workflow.md) -- Problem-solving philosophy and TDD workflow

## Project Management

- [tech-debt.md](tech-debt.md) -- Known technical debt items with measurement commands
- [design-plans/](design-plans/) -- Design plans (architecture and phasing for major efforts)
  - [design-plans/2026-04-05-server-rewrite.md](design-plans/2026-04-05-server-rewrite.md) -- Local-first `simlin-serve` binary: filesystem-backed editor + in-process MCP
  - [design-plans/2026-04-25-ltm-per-ref-elem-graph.md](design-plans/2026-04-25-ltm-per-ref-elem-graph.md) -- Per-reference element causal graph: classify each AST reference by access shape, emit truthful per-reference element edges (includes a post-Phases-1-5 measurement postscript)
  - [design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md](design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md) -- Tiered LTM loop enumeration: variable-level Johnson first, expand only the cross-element subgraph
  - [design-plans/2026-05-09-ltm-503-cross-element-agg.md](design-plans/2026-05-09-ltm-503-cross-element-agg.md) -- Cross-element LTM scoring: per-element arrayed-target partials, element-level cross-element loops, array reducers as aggregate nodes
  - [design-plans/2026-05-11-ltm-arrays-hardening.md](design-plans/2026-05-11-ltm-arrays-hardening.md) -- Arrayed/cross-element LTM hardening: unify the reference-site walkers behind one classification IR (#520), then layer eight fixes (#487, #511, #510, #514, #515, #483, #502, #492)
  - [design-plans/2026-05-13-macros.md](design-plans/2026-05-13-macros.md) -- Vensim macro support: macros as a data-driven generalization of the stdlib module mechanism, persisted via a `MacroSpec` marker on `Model`; 7 implementation phases
  - [design-plans/2026-05-19-clearn-residual.md](design-plans/2026-05-19-clearn-residual.md) -- Close C-LEARN's residual (#590/#591) as general Vensim import/simulation primitives: arrayed inline graphical functions, import-time macro shadowing, user-macro INITIAL recurrence, residual attribution; 5 phases
  - [design-plans/2026-05-20-wasm-backend.md](design-plans/2026-05-20-wasm-backend.md) -- WebAssembly code-generation backend: compile a model to one self-contained wasm module as an alternative to the bytecode VM (for fast interactive re-simulation), validated to full VM parity; 8 phases
  - [design-plans/2026-05-22-engine-wasm-sim.md](design-plans/2026-05-22-engine-wasm-sim.md) -- Integrate the wasm backend into `@simlin/engine` as a selectable engine (`Model.simulate({engine:'wasm'})`): vm-vs-wasm demux below the `Sim` facade in `DirectBackend`, a resumable blob run ABI for `runTo`, and a node VM-vs-wasm benchmark; 4 phases
  - [design-plans/2026-05-22-layout-quality-eval.md](design-plans/2026-05-22-layout-quality-eval.md) -- Layout quality evaluation + hill-climbing harness: a pure geometry-accurate `LayoutMetrics` (overlap/sprawl/accurate-arc crossings) and benchstat-style seed-distribution stats, an on-demand corpus sweep that renders and scores layouts against human references, and Rung 0 (rank seeds by `weighted_cost`); 5 phases
  - [design-plans/2026-05-26-wasm-ltm.md](design-plans/2026-05-26-wasm-ltm.md) -- LTM on the wasm backend: thread `enableLtm` into `simlin_model_compile_to_wasm`, add Results-from-slab + from-wasm analyze FFI, and wire `Sim.getLinks` / `Run.links` to run the shared analytic core over a wasm sim so its links match the VM (no protocol change); 6 phases
- [plans/](plans/README.md) -- Implementation plans (active and completed)
- [test-plans/](test-plans/) -- Human verification plans for completed features
  - [test-plans/2026-05-22-engine-wasm-sim.md](test-plans/2026-05-22-engine-wasm-sim.md) -- Manual verification for the `@simlin/engine` selectable wasm engine (`Model.simulate({engine:'wasm'})`): re-running the automated gates, driving the gated/`#[ignore]`d heavy tests, and the human-judged extras (interactive scrubbing feel, VM-vs-wasm benchmark numbers); all 25 ACs already have automated coverage
  - [test-plans/2026-05-22-layout-quality-eval.md](test-plans/2026-05-22-layout-quality-eval.md) -- Manual verification for the layout-quality eval: running the on-demand corpus sweep and inspecting its `target/layout-eval/` artifacts (metrics.json, the worst-first contact-sheet), plus the human-judgment calibration gate (best/median/worst ordering, reference-vs-auto scoring, weight magnitudes)
  - [test-plans/2026-05-26-wasm-ltm.md](test-plans/2026-05-26-wasm-ltm.md) -- Manual verification for LTM on the wasm backend: re-running the automated gates (`simulate_ltm_wasm`, the libsimlin from-wasm analyze FFIs, `wasm-ltm.test.ts`, `worker-wasm.test.ts`), driving the ignored heavy discovery twins (C-LEARN, World3), and the human-judged end-to-end Node-REPL scenarios (`Model.simulate({engine:'wasm', enableLtm:true})`, `sim.getLinks()`/`Run.links` parity, the WorkerBackend round-trip, and the no-silent-fallback contract on an Unsupported LTM model); all 16 ACs already have automated coverage
- `implementation-plans/` -- Detailed phase-by-phase implementation plans, created during plan execution

## Security

- [threat-model.md](threat-model.md) -- `simlin-serve` V1 threat model: trust boundary, defenses (loopback bind, bearer token, DNS-rebinding mitigation, cross-origin defense, supply chain), and out-of-scope items

## Domain Knowledge

- [reference/xmile-v1.0.html](reference/xmile-v1.0.html) -- XMILE interchange format specification
- [reference/vensim-macros.md](reference/vensim-macros.md) -- Vensim macros (`:MACRO:`): definition/call syntax, semantics (per-invocation stock state, locality, recursion), XMILE `<macro>` representation, xmutil's mapping, and implementation implications
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
