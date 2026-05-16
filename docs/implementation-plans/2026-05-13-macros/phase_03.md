# Vensim Macro Support — Phase 3: Compile-time expansion and single-output simulation

**Goal:** Resolve and expand single-output macro invocations so macro-using models simulate with faithful Vensim semantics — per-invocation independent state, expression-valued arguments, macro-local helpers, macro-to-macro nesting, `$`-time access, and builtin-name shadowing.

**Architecture:** Generalize the engine's existing **stdlib-as-modules** mechanism. `SMTH1`/`DELAY3`/`TREND`/`NPV` are small models that `BuiltinVisitor` instantiates as `Variable::Module` instances when it sees function-call syntax; the rest of the compiler and VM then handle them generically. Phase 2 already made each macro definition an ordinary `datamodel::Model` (with a `MacroSpec` and synthesized port variables), so a macro *is* structurally just another module-target model. Phase 3 adds: (1) a **module-function resolver** with a per-project **macro registry** that answers "is this call name a macro or a stdlib function, and what is its `{ model_name, parameter_ports, primary_output }` descriptor?" — with macros taking precedence over identically-named builtins; (2) a generalized `BuiltinVisitor` that consults the resolver and expands a macro call into a `Variable::Module` exactly the way it expands a stdlib call; (3) routing of the `model.rs` pre-classification sites through the resolver. The simulation compiler, the module-instantiation machinery, and the VM are **unchanged** — they are already model-origin-agnostic (ordinary user submodels already work as module targets today).

**Tech Stack:** Rust — the engine compile pipeline (`builtins.rs`, `builtins_visitor.rs`, `model.rs`, `db.rs`, `variable.rs`) and a new `module_functions.rs` module. No external dependencies.

**Scope:** 7 phases from the original design (`docs/design-plans/2026-05-13-macros.md`); this is phase 3 of 7.

**Codebase verified:** 2026-05-14

---

## Acceptance Criteria Coverage

This phase implements and tests:

### macros.AC2: Single-output invocations simulate with correct Vensim semantics
- **macros.AC2.1 Success:** A stockless single-output macro invocation (`macro_expression`) simulates and matches the fixture's `output.tab`.
- **macros.AC2.2 Success:** A stock-bearing macro invocation (`macro_stock`) simulates with correct per-invocation integration, matching the 11-step `output.tab`.
- **macros.AC2.3 Success:** The same macro invoked at multiple call sites produces independent per-invocation state -- two stock-bearing invocations don't share a stock.
- **macros.AC2.4 Success:** A macro invoked with an expression-valued argument (`MYMACRO(a + b, t)`) simulates correctly, with the argument evaluated in the caller's context.
- **macros.AC2.5 Success:** A multi-equation macro body with macro-local helpers (`macro_multi_expression`) simulates correctly; helper names don't leak into the caller's namespace.
- **macros.AC2.6 Success:** A macro that calls another macro (`macro_cross_reference`) expands recursively and simulates correctly.
- **macros.AC2.7 Success:** A macro body referencing global time via the `$` escape simulates with the global time values.
- **macros.AC2.8 Edge:** A macro invocation nested inside a larger expression (`y = c + MYMACRO(x, t)`) expands and simulates correctly.

### macros.AC5: Error handling and edge cases
- **macros.AC5.1 Failure:** A macro invoked with the wrong number of arguments reports an arity-mismatch diagnostic naming the macro.
- **macros.AC5.2 Failure:** A directly or mutually recursive macro is rejected with a cycle-detection error rather than expanding without termination.
- **macros.AC5.3 Failure:** Two macros sharing a name, or a macro name colliding with a model name, report a registry-build diagnostic.
- **macros.AC5.4 Success:** A macro shadowing a builtin (`SSHAPE`, `RAMP FROM TO`) resolves to the macro; the builtin is not invoked.
- **macros.AC5.5 Success:** A macro defined after its first use (`macro_trailing_definition`) still resolves and simulates.
- **macros.AC5.6 Failure:** A call to a name that is neither a macro, a stdlib function, nor a builtin reports an "unknown function or macro" error.

---

## Current state (verified 2026-05-14)

**The stdlib-as-modules mechanism — what Phase 3 generalizes:**
- `is_stdlib_module_function` (`builtins.rs:377-380`) is the predicate for "this name expands to a stdlib module"; `stdlib_args` (`builtins_visitor.rs:16-27`) is the **only** declaration of stdlib input-port names/order (`["input", "delay_time", "initial_value"]` for the smooth/delay/trend family, `["stream", "discount_rate", "initial_value", "factor"]` for `npv`).
- `BuiltinVisitor` (`builtins_visitor.rs:124-146`), its `walk()` (`builtins_visitor.rs:377-566`), and `instantiate_implicit_modules` (`builtins_visitor.rs:574-690`, signature `instantiate_implicit_modules(ident, ast, dimensions_ctx, module_idents) -> EquationResult<(Ast<Expr0>, Vec<datamodel::Variable>)>`, called from `variable.rs:513`). Inside `walk()`'s `Expr0::App` arm the dispatch order is: (1) `rewrite_alias_module_call` (normalizes `delay`/`delayn`/`smthn`), (2) special routing for `modulo`/`previous`/`init`, (3) `if is_builtin_fn(&func)` → a `BuiltinFn`, (4) else `let Some(ports) = stdlib_args(&func) else { return eqn_err!(UnknownBuiltin, ...) }` then the module rewrite (`builtins_visitor.rs:455-540`). **That `else` branch is exactly where an unrecognized macro call dies today.**
- The module rewrite (`builtins_visitor.rs:455-540`): synthetic instance name `format!("$⁚{}⁚{}⁚{}", self.variable_name, self.n, func)` (+ subscript suffix in A2A context); each argument hoisted into a synthetic `Aux` named `$⁚{var}⁚{n}⁚arg{i}`; one `ModuleReference { src: <hoisted arg ident>, dst: format!("{}.{}", module_name, port_name) }` per arg; a `datamodel::Variable::Module { ident: module_name, model_name: format!("stdlib⁚{}", func), references, ... }`; the call expression replaced with `Var(format!("{}·output", module_name))` — the `·output` suffix is **hardcoded**.
- The `model.rs` pre-classification sites: `collect_module_idents` (`model.rs:794-820`) and `equation_is_stdlib_call` (`model.rs:833-848`), both routing through `is_stdlib_module_function`. `collect_module_idents` is called from `ModelStage0::new` (`model.rs:882`, test-only), `ModelStage0::new_cached` (`model.rs:959`, production), and the salsa path `module_ident_context_for_model` (`db.rs:799`).

**The module machinery is already model-origin-agnostic — Phase 3 needs NO compiler/VM changes:**
- `sync_from_datamodel` (`db.rs:2398-2524`) registers **every** `project.models` entry as a `SourceModel`. Since Phase 2 puts macro-marked models in `project.models`, they are **already registered** — no new registration step is needed.
- A `Variable::Module` compiles to `Expr::EvalModule(ident, model_name, input_set, inputs)` (`compiler/mod.rs:686-707`). `enumerate_modules_inner` (`model.rs:741-780`) resolves `model_name` against the project's models (`model_err!(BadModelName, ...)` if absent) and builds the `input_set: BTreeSet<Ident<Canonical>>` from the `ModuleReference.dst` idents. A sub-model variable is overridden as an input port purely by name-membership: `Var::new` (`compiler/mod.rs:2313-2324`) does `ctx.inputs.iter().find(|n| n.as_str() == var.ident())` → `AssignCurr(offset, ModuleInput(off))`. **No flag, no naming convention, no special equation** — the port variable just has to exist with that ident (Phase 2 synthesizes it) and be named in a `ModuleReference.dst`.
- `is_stdlib`/`implicit` is derived **solely** from the `"stdlib⁚"` name prefix (`db.rs:1710`, `project.rs:110-112`); `implicit` drives three stdlib-only behaviors (all-names→`module_idents`, parse-cache bypass, skip unit-inference). A macro model is an ordinary, non-prefixed `SourceModel` and **must not** get the `implicit` treatment — it compiles correctly as an ordinary sub-model because Phase 2 flags its port variables `can_be_module_input: true` (the same flag an ordinary XMILE submodel sets via `access="input"`). Ordinary user models are already instantiated as module targets today — `test/modules_hares_and_foxes/modules_hares_and_foxes.stmx`, `simulates_modules()` at `simulate.rs:378-379`.

**Builtin-name shadowing must be resolved at COMPILE time:** `SSHAPE` and `RAMP FROM TO` are in the MDL `BUILTINS` table (`mdl/builtins.rs:332-333`) → the MDL parser classifies them `CallKind::Builtin` (→ `Token::Function`) **at parse time**, with no knowledge of a same-named macro. `SAMPLE UNTIL` is *not* in the table → `CallKind::Symbol`. So the resolver must consult the macro registry **before** honoring the builtin dispatch, for *both* `CallKind` values — `CallKind` cannot be trusted to detect a shadowing macro.

**Errors:** `ErrorCode` (`common.rs:51-105`) has `UnknownBuiltin` (the current catch-all for an unrecognized call name), `BadBuiltinArgs` (the natural code for an arity mismatch), `CircularDependency` (for a recursion cycle), `DuplicateVariable` (the closest existing code for a duplicate-name collision). `EquationError { start: u16, end: u16, code: ErrorCode }` attaches a code to a byte span within an equation; `Error { kind, code, details: Option<String> }` is the model/project-level error. Construction macros: `eqn_err!(code, start, end)`, `model_err!(code, str)`, `sim_err!(code [, str])`. Salsa path: `Diagnostic { model, variable, error: DiagnosticError, severity }`, `DiagnosticError::{Equation(EquationError), Model(Error), Assembly(String)}`. `compile_project_incremental` (`db.rs:5904-5913`) maps a returned `Err` → `sim_err!(NotSimulatable, msg)`.

**Compile pipeline:** `sync_from_datamodel` → parse (`parse_source_variable_with_module_context`, threading `module_idents` from `module_ident_context_for_model`/`collect_module_idents` at `db.rs:799`) → `instantiate_implicit_modules` → `BuiltinVisitor` → synthetic `Variable::Module` + hoisted `Aux`es land as `implicit_vars` → `model_implicit_var_info` extracts `ImplicitVarMeta { is_module, model_name }` → dep graph → `enumerate_module_instances` → `assemble_module` (`dep_graph.has_cycle()` → `DiagnosticError::Assembly`) → `assemble_simulation` → `compile_project_incremental`.

**Test surfaces:** `convert_mdl(source) -> Result<Project, ConvertError>` (`convert/mod.rs:315-318`, `#[cfg(test)]`, in-crate only). `open_vensim(&str) -> common::Result<Project>` (public). `simulate.rs`: `compile_vm(project)` (lines 96-102), `simulate_mdl_path(path)` (lines 277-296), `simulate_path` (lines 185-251). The three macro `.xmile` fixtures are commented out at `simulate.rs:27-29`; `#[ignore] simulates_clearn()` is at `simulate.rs:908-938`. New MDL fixtures wire in as `#[test] fn simulates_<name>_mdl() { simulate_mdl_path("../../test/.../X.mdl"); }`. `builtins_visitor.rs`'s `mod tests` holds the stdlib expansion tests (`test_arrayed_smooth1`, `test_npv_basic`, `test_arrayed_delayn_unsupported_order` which asserts `UnknownBuiltin`). `TestProject` (`testutils.rs`) with `assert_vm_result` / `assert_compile_error_vm` is the in-crate compile/run assertion surface.

**Documented limitation — the non-time `$` escape.** Vensim's `$` escape has two uses: the *time* form (`Time$` — a macro body's access to global simulation time, AC2.7, fully supported; Phase 2 translates it) and the *non-time* form (`FOO$` — referencing some other non-time model variable from inside a macro body). Per the design, the non-time `$` form is **deprioritized and not supported** in this implementation. A non-time `$` reference is therefore *expected to fail*: it surfaces as an ordinary unknown-variable / unresolved-reference diagnostic at compile time (there is no macro-specific error code for it), and that failure is an **accepted documented limitation**, not a macro-handling bug. Phase 7's C-LEARN expansion assertion accounts for this — such a diagnostic is *not* macro-attributable.

---

<!-- START_SUBCOMPONENT_A (task 1) -->
## Subcomponent A: The module-function resolver and macro registry

<!-- START_TASK_1 -->
### Task 1: `ModuleFunctionDescriptor`, `MacroRegistry`, and registry-build validation

**Verifies:** macros.AC5.2, macros.AC5.3 (the detection logic — end-to-end surfacing is verified in Task 2).

**Files:**
- Create: `src/simlin-engine/src/module_functions.rs`
- Modify: `src/simlin-engine/src/lib.rs` (add `mod module_functions;`)
- Modify: `src/simlin-engine/src/builtins_visitor.rs` (make `stdlib_args` `pub(crate)`, or move it into `module_functions.rs` — the implementor's choice; it should be the single source of truth for stdlib port names)

**Implementation:**

This task is a pure functional core — it takes datamodel values and returns a registry or an error, with no I/O and no compiler-pipeline plumbing (that is Task 2). Define in `module_functions.rs`:

1. **`ModuleFunctionDescriptor`** — the unified answer for "what does this module-function expand into," serving both stdlib functions and macros:
   ```rust
   pub(crate) struct ModuleFunctionDescriptor {
       /// The `datamodel::Model.name` of the target model — `"stdlib⁚smth1"`
       /// for a stdlib function, or the macro's canonical model name.
       pub model_name: String,
       /// Ordered input-port variable names; call argument `i` wires to port `i`.
       pub parameter_ports: Vec<String>,
       /// The body variable whose value the call expression is replaced with.
       pub primary_output: String,
       /// `:`-list additional output ports (empty for stdlib and for
       /// single-output macros; consumed in Phase 4).
       pub additional_outputs: Vec<String>,
       /// True for project macros (strict arity — argument count must equal
       /// `parameter_ports.len()`); false for stdlib functions, which permit
       /// fewer arguments than ports (trailing ports are optional).
       pub is_macro: bool,
   }
   ```

2. **`stdlib_descriptor(name: &str) -> Option<ModuleFunctionDescriptor>`** — builds a descriptor for a stdlib module-function. It is called *after* `rewrite_alias_module_call` has normalized aliases, so `name` is already a canonical stdlib model name. For a `name` where `stdlib_args(name)` is `Some`: `model_name = format!("stdlib⁚{name}")`, `parameter_ports = stdlib_args(name).to_vec()`, `primary_output = "output"`, `additional_outputs = vec![]`, `is_macro = false`. Otherwise `None`. (This preserves the existing stdlib behavior exactly — it just bundles the previously-scattered facts into one struct. Do **not** fold the `rewrite_alias_module_call` alias normalization into this — that stays a separate pre-step in `walk()`.)

3. **`MacroRegistry`** — built once per project from all of its models:
   ```rust
   pub(crate) struct MacroRegistry {
       /// canonical macro name -> descriptor
       macros: HashMap<String, ModuleFunctionDescriptor>,
   }
   ```
   - **`MacroRegistry::build(models: &[datamodel::Model]) -> Result<MacroRegistry, Error>`** — iterate `models`; a model is a macro iff `model.macro_spec.is_some()`. For each macro model, build a `ModuleFunctionDescriptor` from its `MacroSpec` (`model_name = model.name`, `parameter_ports = macro_spec.parameters`, `primary_output = macro_spec.primary_output`, `additional_outputs = macro_spec.additional_outputs`, `is_macro = true`). Keyed by the canonical macro name.
   - **`MacroRegistry::resolve_macro(&self, call_name: &str) -> Option<&ModuleFunctionDescriptor>`** — canonicalize `call_name` and look it up.
   - **Validation (returns `Err` from `build`):**
     - **macros.AC5.3 — duplicate macro name:** two macro-marked models with the same canonical name → `Err` (use `model_err!(DuplicateVariable, ...)` or, if you add a dedicated `ErrorCode::DuplicateMacroName`, add it at the end of the enum — confirm `ErrorCode` is a runtime type not part of `project_io.proto` first, via grep; it is, so additions are safe). The message names the duplicated macro.
     - **macros.AC5.3 — macro/model name collision:** a macro's canonical name equals a non-macro model's canonical name → `Err`, message naming the collision.
     - **macros.AC5.2 — recursion cycle:** build the macro call graph — for each macro model, parse each body variable's equation text (`Expr0::new(text, LexerType::Equation)`), walk the AST for `App(name, ...)` nodes whose canonicalized `name` is another macro in the set, and add an edge `this_macro -> called_macro`. Run cycle detection over the graph (DFS, or reuse an existing cycle-detection utility if one is exposed in the crate). A cycle → `Err` with code `CircularDependency` and a message naming the cycle path (e.g. `"recursive macro: A -> B -> A"`).

**Testing:** Pure-function unit tests in `module_functions.rs`'s inline `#[cfg(test)] mod tests`. Build small `Vec<datamodel::Model>` fixtures by hand (or with `TestProject` if it is convenient — but hand-built `datamodel::Model`s with `macro_spec: Some(...)` are fine and direct):
- `stdlib_descriptor("smth1")` returns the expected ports/output; `stdlib_descriptor("not_a_thing")` returns `None`.
- `MacroRegistry::build` over a project with one macro → `resolve_macro` returns its descriptor; `resolve_macro` of a non-macro name → `None`.
- A macro whose name equals a stdlib name → `resolve_macro` still returns the macro descriptor (the *precedence* is enforced in Task 3's `walk()` ordering, but confirm the registry itself stores and returns the macro).
- **macros.AC5.3:** two macro models named `FOO` → `build` returns `Err` naming `FOO`. A macro named `main` alongside a `main` model → `build` returns `Err` naming the collision.
- **macros.AC5.2:** a macro `A` whose body calls `A` → `build` returns `Err` with `CircularDependency`. A macro `A` calling `B` calling `A` → `build` returns `Err`. A macro `A` calling `B` (no cycle, the `macro_cross_reference` shape) → `build` succeeds.

**Verification:**
Run: `cargo test -p simlin-engine module_functions`
Expected: all resolver/registry/validation unit tests pass.

**Commit:** `engine: add the module-function resolver and macro registry`
<!-- END_TASK_1 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 2-3) -->
## Subcomponent B: Wire the resolver into compilation

<!-- START_TASK_2 -->
### Task 2: Build the registry during compilation; route the `model.rs` classification sites

**Verifies:** macros.AC5.2, macros.AC5.3 (end-to-end: a recursive or name-colliding macro `.mdl` produces a clear compile error).

**Files:**
- Modify: `src/simlin-engine/src/model.rs` (`collect_module_idents` ~lines 794-820, `equation_is_stdlib_call` ~lines 833-848, `ModelStage0::new` ~line 853, `ModelStage0::new_cached` ~line 932)
- Modify: `src/simlin-engine/src/db.rs` (`module_ident_context_for_model` ~lines 789-811; the compile entry `compile_project_incremental` ~lines 5904-5913)
- Modify: `src/simlin-engine/src/project.rs` (`Project::from_datamodel` / `from_salsa` — the non-incremental compile entry)

**Implementation:**

1. **Build the registry once per compile.** At the compile entry points, build `MacroRegistry::build(&project_models)` and make it available to the parse/classification stages. In the salsa path this is ideally a salsa-tracked query keyed on the macro-marked `SourceModel`s; in the non-incremental path (`Project::from_datamodel`/`from_salsa`) build it once. Thread `&MacroRegistry` to wherever `module_idents` is computed — `module_ident_context_for_model` (`db.rs:799`) and `ModelStage0::new`/`new_cached` (`model.rs`).

2. **Surface registry-build errors as compile failures.** If `MacroRegistry::build` returns `Err` (AC5.2 cycle, AC5.3 duplicate/collision), surface it as a compile failure with that error's message — a project-level error is the natural fit since registry-build precedes per-model processing (propagate the `Err`; `compile_project_incremental` maps it to `sim_err!(NotSimulatable, msg)`). If the salsa diagnostic accumulator is reachable at registry-build time, emit a `Diagnostic` instead. Either way the surfaced message must clearly identify the offending macro(s) / cycle.

3. **Route the `model.rs` pre-classification through the resolver.** `equation_is_stdlib_call` currently returns `is_stdlib_module_function(&canonicalize(&func))`. Change it to also return true when `func` resolves to a macro: take `&MacroRegistry` as a parameter and return `is_stdlib_module_function(name) || registry.resolve_macro(name).is_some()`. (Consider renaming it `equation_is_module_call` since it is no longer stdlib-only — optional cleanup.) Thread `&MacroRegistry` into `collect_module_idents` and on to `equation_is_stdlib_call`. This makes a caller variable whose equation is `y = MYMACRO(...)` get pre-classified into `module_idents`, exactly as a `y = SMTH1(...)` variable is — required so that `PREVIOUS(y)` rewrites correctly.

   **Scope note — `Equation::Arrayed`:** `equation_is_stdlib_call` inspects only `Equation::Scalar` and `Equation::ApplyToAll`; it returns `false` immediately for `Equation::Arrayed` (the per-element-equation form). This is sufficient for Phase 4's apply-to-all `macro_arrayed` fixture, whose arrayed invocation `out[Region] = SCALE(inp[Region], factor)` is stored as `Equation::ApplyToAll`. A macro invocation written as a *per-element* `Equation::Arrayed` would not be pre-classified through this path — but that exactly matches the **pre-existing** behavior for arrayed stdlib calls, so it is not a macro-specific regression and is out of Phase 3 scope; an executing engineer should not chase a per-element-equation macro call's pre-classification miss as a bug.

Do **not** thread the registry into `instantiate_implicit_modules` / `BuiltinVisitor` yet — that is Task 3. (A threaded-but-unused parameter would trip `clippy -D warnings`.) After Task 2: no-macro projects are byte-for-byte unchanged; a valid macro project classifies macro-call variables correctly but its macro calls still hit `UnknownBuiltin` in `BuiltinVisitor` (fixed in Task 3); an *invalid* macro project (cycle / duplicate / collision) fails the compile with a clear error.

**Testing:**
- A focused in-crate unit test of `equation_is_stdlib_call` (its new signature) in `model.rs`'s `#[cfg(test)]` module: with a `MacroRegistry` containing a macro `MYMACRO`, assert it returns `true` for an equation `MYMACRO(a, b)` and `true` for `SMTH1(x, 5)` and `false` for `a + b`.
- **macros.AC5.2** (end-to-end): `convert_mdl` a `.mdl` with a directly recursive macro (its body calls itself) and a `main` invocation, compile it, and assert the compile fails with a cycle-detection error (`CircularDependency`) whose message names the macro. Repeat for a mutually recursive `A`/`B` pair.
- **macros.AC5.3** (end-to-end): `convert_mdl` a `.mdl` with two `:MACRO:` blocks of the same name → assert the compile fails with a duplicate-name error naming the macro. A `.mdl` with a macro named `main` → assert the compile fails with a collision error.
- Existing engine tests still pass unchanged (no-macro projects are unaffected).

**Verification:**
Run: `cargo test -p simlin-engine model::`
Run: `cargo test -p simlin-engine --test simulate`
Expected: all pass, including the new cycle / duplicate / collision tests; existing simulation tests unchanged.

**Commit:** `engine: build the macro registry during compilation`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Generalize `BuiltinVisitor` to expand macro calls

**Verifies:** macros.AC2.1 (end-to-end smoke), macros.AC5.1, macros.AC5.4, macros.AC5.6.

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs` (`BuiltinVisitor` struct ~lines 124-146; `walk()`'s `App` arm ~lines 377-566; the module-rewrite region ~lines 455-540; `instantiate_implicit_modules` ~lines 574-690)
- Modify: `src/simlin-engine/src/variable.rs` (the `parse_var_with_module_context` parse path that calls `instantiate_implicit_modules` ~line 513)
- Modify: `src/simlin-engine/src/model.rs`, `src/simlin-engine/src/db.rs` (extend the Task 2 registry threading the rest of the way down to the `instantiate_implicit_modules` call)
- Test: `src/simlin-engine/src/builtins_visitor.rs` inline `#[cfg(test)] mod tests` (or a new `src/simlin-engine/src/macro_expansion_tests.rs` declared `#[cfg(test)] mod macro_expansion_tests;` if the set grows large)

**Implementation:**

1. **Thread the registry to `BuiltinVisitor`.** Add a `macro_registry: &'a MacroRegistry` field to the `BuiltinVisitor` struct (alongside `module_idents`). Add it as a parameter to `instantiate_implicit_modules` and extend the Task 2 threading from `ModelStage0` through `variable.rs`'s parse path so the registry reaches the `instantiate_implicit_modules` call at `variable.rs:513`.

2. **Generalize the module rewrite into a descriptor-driven helper.** Extract the existing stdlib module-rewrite (`builtins_visitor.rs:455-540`) into a helper that takes a `&ModuleFunctionDescriptor` and the call's args/loc and produces the synthetic `Variable::Module` + hoisted arg `Aux`es + the replacement expression. Changes vs. the current hardcoded code:
   - `model_name` comes from `descriptor.model_name` (was `format!("stdlib⁚{func}")`).
   - The `ModuleReference.dst` port names come from `descriptor.parameter_ports` (was `stdlib_args`).
   - The replacement expression is `Var(format!("{}·{}", module_name, descriptor.primary_output))` (was hardcoded `·output`). For stdlib descriptors `primary_output` is `"output"`, so stdlib expansion is byte-for-byte unchanged. **Separator layering (do not confuse with Phase 4):** the `·` (U+00B7) here is the *compile-time AST* form — `BuiltinVisitor` rewrites an `Expr0::Var` node directly (post-parse), so it uses the already-canonical separator. This is a *distinct layer* from Phase 4's *import-time datamodel* materialization, where a multi-output binding aux's `Equation::Scalar` text is written with an ASCII period (`{module}.{output}`) that `canonicalize()` converts to `·` only later, when that equation string is parsed during compilation. Both are correct — this phase operates on the AST, the Phase 4 MDL converter operates on datamodel equation strings.
   - **Arity check:** if `descriptor.is_macro` and `args.len() != descriptor.parameter_ports.len()`, return `eqn_err!(BadBuiltinArgs, loc.start, loc.end)`. For stdlib descriptors (`is_macro == false`), preserve the current lenient behavior exactly — do not add an arity check to the stdlib path (stdlib functions legitimately accept fewer args than ports; e.g. `SMTH1` with the `initial_value` port unwired).
   The synthetic-name (`$⁚{var}⁚{n}⁚{func}`), arg-hoisting, and A2A subscript-suffix logic are reused unchanged.

3. **Consult the resolver in `walk()`'s `App` arm.** Make two changes to the dispatch:
   - **At the very top of the `App` arm** (before `rewrite_alias_module_call`, before the `modulo`/`previous`/`init` routing, before `is_builtin_fn`): `if let Some(desc) = self.macro_registry.resolve_macro(&func) { return <expand via the descriptor-driven helper> }`. This is the macro-shadows-everything precedence — a project macro named `SSHAPE` is expanded as the macro even though `SSHAPE` parsed as `CallKind::Builtin`.
   - **At the existing stdlib branch** (the `stdlib_args` `else` that currently emits `UnknownBuiltin`): replace `let Some(ports) = stdlib_args(&func) else { eqn_err!(UnknownBuiltin) }` with `let Some(desc) = stdlib_descriptor(&func) else { return eqn_err!(UnknownBuiltin, ...) }` and expand via the same descriptor-driven helper. (`UnknownBuiltin` still fires for a name that is neither a macro, nor an `is_builtin_fn` builtin, nor a stdlib module — satisfying AC5.6. Optionally update the user-facing message text to say "unknown function or macro.")

4. **Make `contains_stdlib_call` macro-aware.** `contains_stdlib_call` (`builtins_visitor.rs:30-56`) is a separate recognition predicate that gates the `Ast::ApplyToAll` / `Ast::Arrayed` per-element expansion paths inside `instantiate_implicit_modules` — it currently recognizes only `is_stdlib_module_function` names (plus `init`/`previous`). Thread `&MacroRegistry` into it (it is called from `instantiate_implicit_modules`, which now has the registry) and have it also return `true` when an `App`'s name resolves to a macro. Without this, a *scalar* macro call would expand (via the `walk()` change above) but an *arrayed* macro invocation would never enter the per-element expansion path. This is the change that lets Phase 4's arrayed-invocation work "ride the existing apply-to-all path for free"; Phase 4 adds the arrayed fixtures and the AC3.4/AC3.5 verification.

After Task 3 a macro call expands into a `Variable::Module` pointing at the macro's model. Because Phase 2 made the macro `Model` a complete sub-model (port variables flagged `can_be_module_input: true`), and `sync_from_datamodel` already registers it, and the module machinery is origin-agnostic, the call now compiles and simulates end-to-end — no further wiring is needed.

**Testing:** Tests build a macro-bearing `datamodel::Project` with `convert_mdl(inline_mdl_str)` (in-crate, test-only) and compile/run it via the in-crate compile path (the same path `TestProject`'s `assert_vm_result` / `assert_compile_error_vm` use — if `TestProject` cannot wrap a `convert_mdl`-produced `Project`, add a minimal `TestProject::from_datamodel`-style constructor, a small general test helper).
- **Structural** (via `instantiate_implicit_modules` directly, or by inspecting the compiled project): a macro call `y = MYMACRO(a, b)` produces a synthetic `Variable::Module` whose `model_name` is the macro's model, with one `ModuleReference` per parameter (`dst` ports matching `MacroSpec.parameters`), and the caller equation replaced by a reference to `<module>·<primary_output>`.
- **`contains_stdlib_call` macro-awareness (structural, non-simulation):** a focused unit test for item 4's change, so the apply-to-all gate's contract is verified *in Phase 3* rather than deferred entirely to Phase 4's arrayed fixtures. With a `MacroRegistry` containing a macro `MYMACRO`, call `contains_stdlib_call` directly on parsed `Expr0` ASTs and assert it returns `true` for an arrayed macro `App` (`MYMACRO(x[Dim], k)`), `true` for a stdlib call (`SMTH1(x, 5)`), and `false` for a plain arithmetic expression (`a + b`). Phase 4's arrayed fixtures then exercise the gate end-to-end, but the predicate's behavior is no longer entirely deferred to Phase 4.
- **macros.AC5.1:** a macro declared with 2 parameters, invoked with 3 args (and separately with 1 arg) → compile fails with `ErrorCode::BadBuiltinArgs`; assert the error span covers the macro call (so the macro is identified in context).
- **macros.AC5.4:** a `.mdl` defining `:MACRO: SSHAPE(x, p)` with body `SSHAPE = x + p`, invoked `y = SSHAPE(3, 4)` → compiles and simulates with `y == 7` (the macro's definition), proving the macro shadowed the `SSHAPE` builtin. Repeat for a `RAMP FROM TO` macro.
- **macros.AC5.6:** a `.mdl` with `y = NOTAFUNCTION(x)` (a name that is neither macro, stdlib, nor builtin) → compile fails with `ErrorCode::UnknownBuiltin`.
- **macros.AC2.1 smoke (end-to-end):** `convert_mdl` a trivial single-output macro (`:MACRO: M(a, b)` / `M = a * b`, invoked `y = M(5, 1.1)`), compile, run the VM, assert `y == 5.5` — confirming the full expansion → registration → compilation → VM path works for macros.

**Verification:**
Run: `cargo test -p simlin-engine builtins_visitor`
Run: `cargo test -p simlin-engine --test simulate` (existing stdlib-module tests must still pass — stdlib expansion is unchanged)
Expected: all pass, including the new macro-expansion tests.

**Commit:** `engine: expand macro invocations through BuiltinVisitor`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (task 4) -->
## Subcomponent C: Single-output simulation fixtures and edge cases

<!-- START_TASK_4 -->
### Task 4: Wire the macro `.mdl` fixtures into `simulate.rs` and add focused simulation tests

**Verifies:** macros.AC2.1, macros.AC2.2, macros.AC2.3, macros.AC2.4, macros.AC2.5, macros.AC2.6, macros.AC2.7, macros.AC2.8, macros.AC5.4, macros.AC5.5.

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (add dedicated `#[test] fn`s; the three macro `.xmile` fixtures commented out at lines 27-29 stay commented — they are wired in Phase 5, because the XMILE reader does not turn their `<macro>` elements into macro-marked models until Phase 5 Task 1)

**Implementation:** Tests only — no production code (if a test surfaces a real wiring gap, fix it, but the expectation after Task 3 is that single-output macros simulate). Two groups:

**Group 1 — the six bundled `.mdl` fixtures**, each wired as a dedicated test calling `simulate_mdl_path` (which does `open_vensim` → `compile_vm` → run → `ensure_results` against the fixture's `output.tab`):
- `simulates_macro_expression_mdl` → `../../test/test-models/tests/macro_expression/test_macro_expression.mdl` — **macros.AC2.1** (stockless single-output).
- `simulates_macro_stock_mdl` → `.../macro_stock/test_macro_stock.mdl` — **macros.AC2.2** (stock-bearing, the 11-step `output.tab`).
- `simulates_macro_multi_expression_mdl` → `.../macro_multi_expression/test_macro_multi_expression.mdl` — **macros.AC2.5** (multi-equation body; the `intermediate` helper must not leak into `main`'s namespace — `ensure_results` already only checks expected columns, but additionally assert the `main` model has no `intermediate` variable).
- `simulates_macro_cross_reference_mdl` → `.../macro_cross_reference/test_macro_cross_reference.mdl` — **macros.AC2.6** (macro calls another macro).
- `simulates_macro_multi_macros_mdl` → `.../macro_multi_macros/test_macro_multi_macros.mdl` — two independent macros.
- `simulates_macro_trailing_definition_mdl` → `.../macro_trailing_definition/test_macro_trailing_definition.mdl` — **macros.AC5.5** (macro defined after its first use).

**Group 2 — focused tests** for the four behaviors with no bundled fixture. Each uses an inline `.mdl` string passed to `open_vensim`, compiled via `compile_vm`, run, with **hand-computed** expected values asserted against the `Results` (use the `Results` API directly, or build a small in-code expected `Results` and call `ensure_results` — the implementor picks the lighter mechanism). Keep each macro trivial so the hand computation is reliable; document the arithmetic in a comment.
- **macros.AC2.3** — independent per-invocation state: a stock-bearing macro `M(rate, init) = INTEG(rate, init)` invoked twice with different arguments (`x = M(1, 0)`, `y = M(2, 10)`); over the run, assert `x` integrates `1`/step from `0` and `y` integrates `2`/step from `10` — they do not share a stock.
- **macros.AC2.4** — expression-valued argument: `M(in, p) = in * p` invoked `y = M(a + b, t)` with constants `a`, `b`, `t`; assert `y == (a + b) * t`, with the argument evaluated in the caller's context.
- **macros.AC2.7** — `$`-time access: a macro body referencing `Time$` (e.g. `M(x) = x + Time$`) invoked `y = M(10)`; assert `y == 10 + time` at each step. (This exercises Phase 2's `$`-time translation plus Phase 3's global-time-resolves-inside-a-module behavior.)
- **macros.AC2.8** — nested invocation: `y = c + M(x, t)` (a macro call inside a larger expression); assert `y` equals `c` plus the macro's value.
- **macros.AC5.4** (simulation-level confirmation): a macro shadowing `SSHAPE` invoked in a model that also uses other builtins; assert the simulated value matches the macro's definition, not the `SSHAPE` builtin's. (Task 3 verifies AC5.4 at the expansion level; this confirms it end-to-end in `simulate.rs`, matching the design's "Done when" listing builtin-name shadowing among the focused fixtures.)

**Verification:**
Run: `cargo test -p simlin-engine --test simulate`
Expected: all six fixture tests and all focused tests pass.

**Commit:** `engine: wire macro fixtures and focused single-output simulation tests`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase 3 completion check

When all four tasks are committed:
- The module-function resolver unifies stdlib and macro lookup behind one `ModuleFunctionDescriptor`; the macro registry is built per-compile with duplicate-name, name-collision, and recursion-cycle validation.
- `BuiltinVisitor` expands a macro call into a `Variable::Module` exactly as it expands a stdlib call; a project macro shadows an identically-named builtin or stdlib function.
- The six single-output `.mdl` macro fixtures simulate and match their `output.tab`, wired into `simulate.rs`; focused tests cover a macro at multiple call sites, expression-valued arguments, `$`-time access, builtin-name shadowing, and nested invocation (the design's "Done when").
- Arity-mismatch, recursion-cycle, registry-collision, and unknown-name errors all report clear diagnostics.
- `macros.AC2.1`–`AC2.8` and `macros.AC5.1`–`AC5.6` are verified.
- The simulation compiler, module-instantiation machinery, and VM are unchanged — no compiler/VM edits were needed, only the front-end resolver and `BuiltinVisitor` generalization.

Multi-output (`:`-list) invocation and arrayed invocation are Phase 4; XMILE round-trip is Phase 5; MDL export is Phase 6.
