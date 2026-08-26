# Compiler unification: one fragment compiler, one builtin table, one dimension matcher, a pure parse

## Summary

The `simlin-engine` compiler is correct and incremental, but the same facts are
stated in several places and the same work is done by several near-copies:
four per-variable fragment compilers that each fabricate stub `Variable`s to
feed a `Context` that only needs shapes; per-builtin facts (arity, which
arguments are arrays, reducer-ness, invariance class) hand-enumerated in ~15
exhaustive `BuiltinFn` matches; array temps materialized at three IR levels;
five or six dimension-matching algorithms; dependency analysis computed three
ways on strings; and a parse that is not a pure function of the equation
because `PREVIOUS`/`INIT`/`SMOOTH` helpers are synthesized as *text* and the
visitor must know the model's module set to do it (which is why adding one
module variable re-keys every parse -- GH #372, `engine-performance.md` C6).

This plan removes that incidental complexity in eight phases, each landing as
reviewed commits on the `compiler-unification` branch. The irreducible parts
-- XMILE array semantics, the two-phase dependency relation, symbolic bytecode
with late resolution, loud-safe refusal -- are kept exactly; every phase must
leave the genuine-output corpus green and record C-LEARN compile cost in the
ledger at the end of this document.

Direction this serves: the engine should be efficient and incremental enough,
and the VM fast enough, that Loops That Matter is simply always on. Every
phase therefore treats the LTM compile path (`db/ltm/`) as a first-class
consumer of the unified machinery, never as a special case left behind.

Out of scope, deliberately: a reference evaluator for `compiler::Expr`
(revisit after this work lands -- the previous one was churn to maintain;
golden `test/` outputs remain the oracle), and loop-form lowering of
apply-to-all equations (GH #1025; sequenced after this
branch).

## Definition of Done

1. **One fragment compiler.** `lower_fragment(&FragmentInput) -> LoweredVarFragment`
   is the only per-variable lowering entry point. Explicit variables, implicit
   helpers, LTM synthetic variables, and LTM implicit helpers are four
   *constructors* of `FragmentInput`. `compiler::Context` consumes `DepShape`s,
   not `&Variable`s. No `bumpalo` arena, no stub `Variable` construction, no
   `MetadataByModel<'a>` lifetime plumbing remains in `db/`.
2. **One builtin table.** `BuiltinFn::signature()` is the single statement of
   per-builtin facts; `BuiltinFn::args()`/`args_mut()` give a uniform argument
   view. Every exhaustive `BuiltinFn` match that only re-derived those facts is
   gone; each surviving match has per-variant semantics and a comment saying so.
3. **One temp allocator, one materialization pass.** Temp ids are allocated by
   one counter per variable lowering; `remap_temp_ids`, `find_max_temp_id`,
   `next_available_temp_id`, and the pointer-keyed `ast_temp_bases` are
   deleted. Array-producing builtins and computed array operands are hoisted
   into temps by one pass after subscript resolution; `Expr3` Pass 1 hoisting
   and the `compiler/mod.rs` A2A hoisting machinery are gone.
4. **One dimension matcher.** `DimMatcher` owns the axis-matching precedence
   (exact name, declared mapping, subdimension, size) and every former matcher
   is a call into it. `lower_from_expr3`'s Subscript arm is decomposed into
   named steps; `lower`/`lower_preserving_dimensions` are one function with an
   explicit mode.
5. **One dependency representation.** Dependencies cross the db boundary as
   structured, interned `DepRef`s (variable, module path, phase, lag). String
   splitting on `·` to classify a dependency is gone; `collect_expr_refs`,
   `DepClassification`'s string sets, and `build_var_info`'s `normalize_dep`/
   `keep_dt_dep` are replaced by reading the structured fields.
6. **One `Variable`.** `Variable { ident, units, eqn, errors, unit_errors, kind }`
   with a `VarKind` enum. Parsing reads a borrowed `VariableSource<'_>` over
   `SourceVariable` fields; `datamodel_variable_from_source` is gone, as is the
   duplicated field extraction in `db/sync.rs`.
7. **Assembly owns no second copy.** The initials phase is renumbered by
   `FragmentMerger` (phase-local literal pool mode), the results-offset map is
   derived from `VariableLayout`, the module-instance input-set extraction has
   one owner, and `assemble_module` collects fragments in one pass.
8. **Errors carry their message.** `EquationError` has `details`; nothing
   between parse and `collect_all_diagnostics` drops error text; parse-stage
   errors are produced as `Diagnostic`s rather than translated three times.
9. **A pure parse.** `parse_source_variable(db, var, project)` is keyed only on
   the variable and project-global inputs. `PREVIOUS(expr)`/`INIT(expr)` with
   a non-slot argument produce *captures* (AST-carried, dependency-scheduled
   evaluation units writing hidden slots), stdlib module functions and macros
   produce AST-carried `ImplicitModule`s; no synthesized helper is ever printed
   to equation text and re-parsed. `ModuleIdentContext` no longer exists.
   Adding a module variable re-keys one parse, not every parse.
10. **Whole-model lowered copies are gone.** Every reader of `model_stage0`/
    `model_stage1`/`Project::from_salsa` is moved to per-variable queries and
    those memos are deleted.
11. The sixteen `#[allow(dead_code)]` `SymbolicOpcode` variants (broadcast
    iteration, incremental view-stack construction) and their `Opcode`, VM, and
    wasm twins are retired (closes GH #612).
12. Docs are evergreen: engine `CLAUDE.md`, `docs/architecture.md`, and
    `docs/design/engine-performance.md` describe the unified pipeline; the
    ledger below records what each phase measured.

## Acceptance Criteria

Slug: `compiler-unification`. Tests named after an AC use the full identifier.

### compiler-unification.AC1: semantics are unchanged
- **AC1.1** Every model in `test/` simulates within its existing tolerance
  after every phase (`simulate.rs`, `simulate_systems`, `mdl_roundtrip`, the
  LTM corpus, the wasm parity corpus).
- **AC1.2** Compiled artifacts are deterministic: the 12-repeat
  `fragment_determinism_tests` / `diagnostic_determinism_tests` stay green.
- **AC1.3** A phase that changes artifact shape (opcode count, temp count, GF
  count, slot count) records the before/after counts in the ledger and says
  why; one that does not change shape records that it is identical.

### compiler-unification.AC2: duplication is removed, not moved
- **AC2.1** After Phase 1, `BuiltinFn::<array-producing variant>` is matched
  outside `builtins.rs` only at sites with per-variant semantics, each with a
  justifying comment. A test constructs every `BuiltinFn` variant and checks
  `args().len()` lies in `[signature().min_args, max_args]`, `map(identity)`
  is the identity, and `args()`/`args_mut()` agree.
- **AC2.2** After Phase 3, `rg bumpalo src/simlin-engine` is empty and no
  `Variable::{Var,Stock,Module} { .. errors: vec![], unit_errors: vec![] }`
  stub construction exists under `src/db/`.
- **AC2.3** After Phase 6, exactly one function decides how two axis lists
  match, and a table-driven test derives its rows from the old matchers'
  precedence orders.
- **AC2.4** After Phase 7, `rg "print_eqn" src/simlin-engine/src/builtins_visitor.rs`
  and the `make_temp_arg`/textual `substitute_dimension_refs` path are gone;
  `rg ModuleIdentContext src/` is empty.

### compiler-unification.AC3: incrementality improves and never regresses
- **AC3.1** A salsa execution-count test shows adding a module variable to a
  model re-executes only that variable's parse (the tight bound
  `implicit_helper_add_is_tight_but_module_helper_add_is_not` asserts today
  flips to tight for the module case).
- **AC3.2** Cold C-LEARN compile retired instructions do not increase in any
  phase (ledger, instruction channel); the structure-changing warm edit cost
  (C6 in `engine-performance.md`) falls after Phase 7.

### compiler-unification.AC4: loud-safe contracts survive
- **AC4.1** Every `Err`/`None` loud-safe path enumerated in the standing
  invariants (stack depth, `resource_base`, `renumber_opcode`, SCC
  segmentation, `fragment_vars_in_layout`) still has its test, and a refactor
  that relocates one relocates its test.
- **AC4.2** The capture design (Phase 7) preserves evaluation order: a capture
  is scheduled by the dependency graph exactly where today's helper variable
  is, never inlined into its parent. A test with `x = PREVIOUS(y)`,
  `y = f(x)` compiles and matches today's output.

## Glossary

- **Fragment**: one variable's one phase (initial / flow / stock) compiled to
  layout-independent symbolic bytecode (`PerVarBytecodes`).
- **Symbolic bytecode**: opcodes whose variable operands are
  `VarRef { name, element_offset }`; `resolve_module` turns them into slots
  at assembly. Addresses are assigned exactly once.
- **Apply-to-all (A2A)**: an arrayed equation written once for every element;
  today unrolled per element at compile time.
- **Capture**: a hidden per-callsite evaluation unit for `PREVIOUS(expr)` /
  `INIT(expr)` whose argument is not a plain variable slot. Replaces today's
  text-synthesized `$⁚{parent}⁚{n}⁚...` helper variable.
- **Implicit module**: a stdlib module-function instance (`SMTH1`, `DELAY3`,
  `TREND`, ...) or macro expansion synthesized from a call in an equation.
- **DepShape**: what the lowering needs to know about a dependency -- its
  dimensions and whether it is an aux, a stock, or a module instance (with
  the sub-model's layout).
- **Loud-safe**: refusing (`Err`, `NotSimulatable`, `None`) rather than
  producing a plausible wrong number.
- **Retired instructions**: the `perf stat` instruction channel; deterministic
  to ~0.03% across builds, so the channel every ledger row uses for compile
  cost. Cycles are reported only with a same-session null control.

## Architecture

### What stays

The pipeline shape in the engine `CLAUDE.md` is kept: parse -> module-function
expansion -> AST lowering -> per-variable fragment compilation -> assembly ->
execution, with `db::compile_project_incremental` as the one production entry.
Every standing invariant listed there holds throughout: addresses assigned
once at assembly; `resolve_bytecode` the sole producer of concrete bytecode;
stock updates fused to `BinOpAssignNext`; array operands are views; no silent
wrong numbers; the VM is wasmgen's oracle; IEEE-exact folding only; the GF
layout rule; `float::NA`. The VM and wasm backends are not changed except to
delete dead opcodes.

### Target contracts

Contracts are the boundaries later phases build on. Names are binding; field
lists are directional and finalized by the implementing teammate.

**Builtin table (`builtins.rs`, Phase 1).**

```rust
pub enum ArgKind { Scalar, Array { whole: bool }, Table, Ident }  // how a position participates in array lowering
pub enum ResultKind { Scalar, Elementwise, Array { shape_from: u8 } } // Array = a temp sized by argument `shape_from`
pub enum Invariance { Pure, TimeDependent, Lagged, Snapshot }
pub struct BuiltinSig { name, aliases: &'static [&'static str], min_args: u8, max_args: Option<u8>,
                        arg_kinds: &'static [ArgKind], unary_reduces: bool,
                        result: ResultKind, invariance: Invariance }
impl BuiltinSig { pub const ALL: [&'static BuiltinSig; 44]; pub fn by_name(&str) -> Option<&'static BuiltinSig>;
                  pub fn accepts_arity(&self, n: usize) -> bool }
impl<E> BuiltinFn<E> {
    pub fn signature(&self) -> &'static BuiltinSig;
    pub fn name(&self) -> &'static str;                  // signature().name
    pub fn args(&self) -> SmallVec<[&E; 5]>;
    pub fn args_mut(&mut self) -> SmallVec<[&mut E; 5]>;
    pub fn is_unary_reduction(&self) -> bool;            // unary_reduces && one argument
    pub fn arg_kinds(&self) -> SmallVec<[ArgKind; 5]>;   // aligned with args(); applies the unary-reduction rule
    pub fn args_with_kinds(&self) -> impl Iterator<Item = (&E, ArgKind)>;
    pub fn result_kind(&self) -> ResultKind;
    pub fn has_array_operand(&self) -> bool;
    pub fn try_map / map (self, f), try_map_ref / map_ref (&self, f), map_with_kinds, try_map_ref_with_kinds
}
```

`Array { whole }` distinguishes a vector builtin's operand (read whole,
independent of the enclosing apply-to-all element) from a reducer's (the
enclosing element pins the axes it names). `unary_reduces` states that the
one-argument `MAX`/`MIN`/`MEAN` reduce an array (XMILE 3.7.1.3: `MAX(A)`
"extends MAX(x, y)", `MEAN(A)` is `SUM(A)/SIZE(A)`); the n-ary forms are
Simlin's scalar rule -- section 3.5's `MAX(x, y)`/`MIN(x, y)` and Stella's
scalar n-ary `MEAN` (`test/test-models/tests/builtin_mean/builtin_mean.stmx`)
-- and the spec's mixed `MAX(A, 0)` ("2: any mix of arrays and scalars") is
not implemented (GH #1026). `arg_kinds()`/`result_kind()` apply the rule to
the value at hand, which is why they are methods on the call rather than
fields. `max_args: None` is the variadic `MEAN`; `Ident` is `isModuleInput`'s
identifier payload, which counts in the source arity but is not an argument
expression. `try_map`/`try_map_ref` are one macro body expanded by value and by
reference (the rebuild changes the expression type, so it cannot be written
over `args_mut`); `args`/`args_mut` are likewise one body; `for_each_expr_ref`
and `walk_builtin_expr` read `args()`. `BuiltinId::arity()` (`bytecode.rs`) is
the precedent and keeps its role for the VM's 22 `Apply` builtins (`INF` and
`PI` lower to `LoadConstant` and carry no id).

**Fragment input (`db/var_fragment.rs` -> `compiler/`, Phase 3).**

```rust
pub struct DepShape { pub dims: Vec<Dimension>, pub kind: DepKind }
pub enum DepKind { Aux, Stock, Module { model_name: Ident<Canonical>, layout: Arc<VariableLayout> } }
pub struct FragmentInput<'a> {
    ident, kind /* aux | stock{inflows, outflows} | module{..} */,
    ast: Option<&'a Ast<Expr2>>, init_ast: Option<&'a Ast<Expr2>>,
    tables: &'a HashMap<Ident<Canonical>, Vec<Table>>,
    deps: &'a IdentMap<Ident<Canonical>, DepShape>,
    module_inputs: &'a BTreeSet<Ident<Canonical>>,
    dims: &'a DimensionsContext, model_name: &'a Ident<Canonical>,
}
pub fn lower_fragment(input: &FragmentInput<'_>) -> LoweredVarFragment;
```

`compiler::Context` reads `DepShape` where it reads `VariableMetadata.var`
today; what `get_submodel_metadata`/`submodel_offset_within` read off a
sub-model's stubs comes from `DepKind::Module.layout`.

**Variable (`variable.rs`, Phase 4).**

```rust
pub struct Variable<MI = ModuleInput, E = Expr2> {
    pub ident, pub units, pub eqn, pub errors, pub unit_errors,
    pub kind: VarKind<MI, E>,
}
pub enum VarKind<MI, E> {
    Stock { init_ast: Option<Ast<E>>, inflows, outflows, non_negative },
    Aux   { ast: Option<Ast<E>>, init_ast: Option<Ast<E>>, tables, is_flow, is_table_only, non_negative },
    Module { model_name, inputs: Vec<MI> },
}
pub struct VariableSource<'a> { /* borrowed SourceVariable fields: ident, equation, gf, units, kind, inflows, outflows, module_refs, compat */ }
```

**Dimension matcher (`dimensions.rs`, Phase 6).**

```rust
pub enum AxisMatch { Exact, Mapped { via: CanonicalDimensionName }, Subdimension, BySize }
pub fn match_axes(source: &[Dimension], target: &[Dimension], ctx: &DimensionsContext) -> Option<Vec<(usize, AxisMatch)>>;
```

Precedence is exact name, then declared mapping (either direction, or both
mapping to a common dimension), then subdimension, then size -- the union of
what the existing matchers do, with the table-driven test deriving rows from
each of them.

**Dependency reference (`db/query.rs`, Phase 8).**

```rust
pub struct DepRef { pub var: Ident<Canonical>, pub via_module: Option<Ident<Canonical>>,
                    pub phase: DepPhase /* Dt | Init */, pub lag: DepLag /* Current | Previous | Initial */ }
```

**Pure parse and captures (`builtins_visitor.rs`, `db/query.rs`, Phase 7).**

```rust
pub struct ParsedVariable { pub variable: Variable<ModuleReference, Expr0>,
                            pub captures: Vec<Capture>, pub implicit_modules: Vec<ImplicitModule> }
pub struct Capture { pub id: u32, pub kind: CaptureKind /* Previous { fallback } | Init */, pub expr: Expr0 }
pub struct ImplicitModule { pub id: u32, pub model_name: Ident<Canonical>,
                            pub inputs: Vec<(ImplicitInputSrc /* Var(Ident) | Capture(u32) */, Ident<Canonical>)> }
```

A capture is a *scheduled unit*, not an inlined expression. Today's helper
variable `$⁚p⁚0` is a real variable in the runlist, evaluated where its
dependencies allow, and `PREVIOUS` reads the previous step's snapshot of it;
the parent carries no dt edge to the helper's inputs. Inlining the expression
into the parent would add those edges (and make `x = PREVIOUS(y); y = f(x)` a
cycle). So a capture keeps a hidden layout slot and its own dependency set,
files into the runlists under a synthetic ident, and is compiled through
`lower_fragment` with a `FragmentInput` constructor of its own -- what changes
is that it is carried as an AST subtree with a positional identity, never as
equation text with a name-derived identity (the GH #1002 question dissolves:
identity is `(parent, id)`). The parse becomes keyed on `(variable, project)`
because every decision that needed the module set -- is this dotted name a
module output, is this call a user module -- moves to lowering, where
`FragmentInput.deps` already knows each dependency's kind. The implementing
teammate's first deliverable is the enumerated list of those decisions, each
relocated or shown unnecessary, before any code moves.

### Existing patterns followed

- `BuiltinId::arity()` (`bytecode.rs:622`) is the single-statement pattern
  Phase 1 generalizes to `BuiltinFn`.
- `SymbolicOpcode::gf_run` and `jump_offset` show the house rule for "one
  place decides, exhaustive with no `_`"; retained matches follow it.
- `FragmentMerger` (`compiler/symbolic.rs`) is already the one owner of
  resource renumbering; Phase 5 extends it rather than adding a fourth copy.
- `var_runlist_membership` is the projection pattern for per-variable keys;
  every new salsa query in Phase 7 and 8 follows it.
- `docs/design/engine-performance.md` "Measuring a change" is the measurement
  protocol; the ledger below names the channel on every row.

## Implementation Phases

Each phase is executed as one or more chunks by a teammate, reviewed by a
fresh adversarial reviewer until there are no material findings, then
committed through the pre-commit hook. Phases are sequential except where a
later note says two chunks are file-disjoint.

<!-- START_PHASE_1 -->
### Phase 1: Builtin signature table

**Goal:** `BuiltinFn::signature()`, `args()`, `args_mut()` exist and replace
every exhaustive match that only re-derived per-builtin facts.

**Components:** `builtins.rs` (table, accessors, `map`/`try_map`/
`for_each_expr_ref`/`walk_builtin_expr` re-implemented over `args_mut`);
callers in `ast/expr1.rs` (`constify_dimensions`, arity check), `ast/expr2.rs`
(reducer bracketing in the App arm), `ast/expr3.rs` (`transform_builtin_inner`,
`references_a2a_dimension`), `compiler/context.rs` (`lower_pass0_builtin`),
`compiler/codegen.rs` (operand push by arity; the `BuiltinId` mapping stays),
`compiler/mod.rs` (five sites), `compiler/pretty.rs`, `compiler/invariance.rs`,
`compiler/array_operand.rs`, `db/assemble.rs` (`collect_expr_refs`),
`units_check.rs`, `units_infer.rs`, `patch.rs`, `db/ltm/compile.rs`.

**Dependencies:** none. Records the ledger baseline before changing anything.

**Done when:** AC2.1 test passes; remaining `BuiltinFn` matches outside
`builtins.rs` each carry a comment naming the per-variant semantics they
encode; corpus green; ledger row (expected: artifact identical).
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Temp allocator and dead opcode retirement

**Goal:** temp ids come from one counter per variable lowering; the sixteen
dead opcode variants are gone.

**Components:** (a) `ast/expr3.rs` (`Pass1Context.next_temp_id`),
`compiler/context.rs` (allocator owned by `Context`/`ContextCore`),
`compiler/mod.rs` (delete `remap_temp_ids`, `find_max_temp_id`,
`next_available_temp_id`, `ast_temp_bases`); (b) `compiler/symbolic.rs`
(`SymbolicOpcode`, `resolve_opcode`, `renumber_opcode`, `gf_run`),
`bytecode.rs` (`Opcode`, `stack_effect`, `name`, `jump_offset`), `vm.rs`
arms, `wasmgen/lower.rs` arms and their tests, GH #612.

**Dependencies:** Phase 1 (so the allocator threads through `args_mut`, not
a hand match). (a) and (b) are file-disjoint and may run as two chunks.

**Done when:** the three helpers and the pointer map do not exist; temp
numbering is deterministic (AC1.2); opcode enums have no `#[allow(dead_code)]`
variant; corpus and wasm parity green; ledger row (temp-count change, if any,
explained).
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: One fragment compiler

**Goal:** `lower_fragment(&FragmentInput)` with four constructors; `Context`
consumes `DepShape`.

**Components:** new `compiler/fragment.rs` (or `db/fragment.rs`) holding
`DepShape`, `FragmentInput`, `lower_fragment`; `compiler/context.rs`
(`VariableMetadata` -> `DepShape`, `get_submodel_metadata`/`submodel_offset_within`
read `DepKind::Module.layout`); `db/var_fragment.rs` and
`db/fragment_compile.rs` reduced to constructors; `db/ltm/compile.rs` both
compile paths reduced to constructors; `db/assemble.rs` (`build_stub_variable`,
`build_submodel_metadata` deleted; `build_module_inputs` and a single
`module_input_set(refs, prefix)` owner used by `enumerate_module_instances_inner`,
`build_var_info`, and LTM); the test-only `compiler::Module`/`build_metadata`
adapted to build `DepShape`s from its `Variable`s; `Cargo.toml` drops `bumpalo`.

**Dependencies:** Phases 1-2.

**Done when:** AC2.2; every fragment (explicit, implicit, LTM synthetic, LTM
implicit) is produced by `lower_fragment`; `rg "bumpalo" src/simlin-engine`
empty; corpus and LTM corpus green; ledger row.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: One `Variable`, borrowed parse input, twins

**Goal:** `Variable { kind }`; `parse_var` reads `VariableSource<'_>`; the
listed twins have one owner.

**Components:** `variable.rs` (`Variable`/`VarKind`, `lower_variable` as a
`kind` map, `parse_var_with_module_context`'s nine parameters become a
`ParseContext`), `db/input.rs` (`datamodel_variable_from_source` deleted),
`db/sync.rs` (one `SourceVariableFields::from_datamodel` behind
`source_variable_from_datamodel` and `update_source_variable`), `model.rs`,
`project.rs`, `ast/mod.rs` (one `paren_if_necessary`, one LaTeX printer
parameterized by expression tier), `variable::var_is_lookup_only` and
`db::source_var_is_table_only` -> one predicate over the shared equation shape,
`compiler/context.rs` (`var_ref`/`submodel_offset_within`/`get_submodel_metadata`
-> one `resolve_qualified`), `canonicalize` calls on already-canonical inputs
in `ClassifyVisitor`, `normalize_subscripts3`, `NamedDimension::get_element_index`
replaced by typed inputs.

**Dependencies:** Phase 3 (the compiler no longer takes `&Variable`, so the
restructure touches parse/model only).

**Done when:** field counts and twins as listed; `too_many_arguments` in
`variable.rs` gone; corpus green; ledger row (expected identical).
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Assembly single-ownership and error channels

**Goal:** `FragmentMerger` renumbers all three phases; offsets derive from
the layout; `assemble_module` collects in one pass; errors keep their text.

**Components:** `compiler/symbolic.rs` (`FragmentMerger` phase-local literal
mode; `renumber_initials_phase` deleted), `db/assemble.rs`
(`calc_flattened_offsets_incremental` replaced by a flatten over
`compute_layout` + module decls; LTM side-loops folded into one
source-parameterized collection; `enumerate_module_instances_inner` uses the
Phase 3 owner), `common.rs` (`EquationError.details`), `db/fragment_compile.rs`
(`accumulate_var_compile_error` keeps details), parse-stage error production
as `Diagnostic` with `Variable.errors`/`ModelStage0.errors` consumers moved.

**Dependencies:** Phase 3 (owners exist), Phase 4 (`Variable` shape settled).

**Done when:** `renumber_initials_phase` and `calc_flattened_offsets_incremental`
do not exist; a test shows a compile error's details reach
`collect_all_diagnostics`; `assemble_module` under ~300 lines; corpus green;
ledger row (expected identical).
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Arrayed lowering core

**Goal:** one `DimMatcher`; a decomposed Subscript arm; one materialization
pass after subscript resolution.

**Components:** (a) `dimensions.rs` (`match_axes`), replacing
`compiler/dimensions.rs` `allocate_implicit_axes_partial` /
`match_dimensions_with_mapping` / `find_dimension_reordering`, `ast/expr2.rs`
`unify_dims_with_names` / `can_all_match` / `find_matching_dimension`,
`compiler/mod.rs` `join_array_views` / `view_contains`, and the inline matching
in `compiler/subscript.rs`; `compiler/context.rs` Subscript arm split into
normalize / resolve-mapped / build-view / emit, `lower` +
`lower_preserving_dimensions` merged under a mode enum. (b) `ast/expr3.rs`
Pass 1 hoisting removed; `compiler/array_operand.rs` becomes the one
materializer of array-producing builtins and computed operands, run after
subscript resolution; `compiler/mod.rs` A2A hoisting machinery
(`expand_a2a_with_hoisting`, `expand_arrayed_hoisted`,
`hoist_nested_array_builtins_in_scalar`, `replace_nested_builtins_for_element`,
`expression_depends_on_active_dimension`) deleted or reduced to per-element
expansion only.

**Dependencies:** Phases 1-2 (signature `arg_kinds` tell the materializer
which positions are arrays; the allocator makes a single pass possible);
Phase 3 (one lowering entry). (a) then (b), as two chunks.

**Done when:** AC2.3; `rg "Pass1Context" src/` empty; corpus, array tests,
and wasm parity green; ledger row (opcode/temp counts may change; explained).
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Pure parse; captures and AST-carried implicit modules

**Goal:** AC3.1 and AC2.4: parse keyed on `(variable, project)`; helpers are
`Capture`/`ImplicitModule` values, not text; `ModuleIdentContext` deleted.

**Components:** first deliverable is a written enumeration (in this document's
Additional Considerations) of every decision `builtins_visitor.rs` makes using
`module_idents`/`model_var_names` and where each moves. Then:
`builtins_visitor.rs` (emit `Capture`/`ImplicitModule` instead of
`datamodel::Variable` text; `make_temp_arg`, textual
`substitute_dimension_refs`, `rewrite_alias_module_call` retired),
`db/query.rs` (`parse_source_variable` without module context;
`ImplicitVarMeta`/`model_implicit_var_info` reshaped around positional ids),
`db/implicit_deps.rs`, `db/fragment_compile.rs` (capture and implicit-module
`FragmentInput` constructors), `db/layout.rs` (hidden capture slots),
`db/dep_graph.rs` (captures as units), `db/assemble.rs`
(`enumerate_module_instances` over `ImplicitModule`), `db/ltm/` (LTM's
PREVIOUS helpers become captures), `model.rs` (`collect_module_idents`,
`equation_is_module_call` deleted), `db/input.rs` (`ModuleIdentContext`
deleted).

**Dependencies:** Phases 3-5 (one lowering entry, one `Variable`, one
diagnostic path). Four chunks: investigation note; captures for
PREVIOUS/INIT; implicit modules and macros including LTM; deletion of the
module-ident context and the salsa re-keying test.

**Done when:** AC2.4, AC3.1, AC4.2; C6 structural-edit cost measured and
recorded; corpus and LTM corpus green.
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Structured dependencies, Stage0/Stage1 retirement, docs

**Goal:** `DepRef` everywhere; no whole-model lowered memos; documentation
evergreen.

**Components:** `db/query.rs` (`VariableDeps` over `DepRef`), `variable.rs`
(`DepClassification` produces `DepRef`s or is replaced by a lowered-fragment
walk), `db/assemble.rs` (`collect_expr_refs` deleted; invariance support from
`DepRef`), `db/dep_graph.rs` (`build_var_info` reads fields), `db/stages.rs`
and every reader of `model_stage0`/`model_stage1` (`db/units.rs`,
`project.rs::from_salsa`, `model.rs::ModelStage1::set_dependencies`,
`enumerate_modules`) moved to per-variable queries and the memos deleted;
decision recorded on the test-only `compiler::Module` oracle (keep adapted or
delete -- see Additional Considerations); engine `CLAUDE.md`,
`docs/architecture.md`, `docs/design/engine-performance.md` rewritten as
current state.

**Dependencies:** Phase 7 (captures change what an implicit dependency is).

**Done when:** `rg "BTreeSet<String>" src/simlin-engine/src/db` finds no
dependency set; `model_stage0`/`model_stage1` do not exist; docs describe the
pipeline as it is; final ledger row; this document's ledger complete.
<!-- END_PHASE_8 -->

## Additional Considerations

**Team process.** One checkout, branch `compiler-unification`, at most three
agents running at once, every teammate on the same model as the lead. A chunk
is: implementer works with the tree uncommitted and runs the relevant
`cargo test -p simlin-engine` subsets, `cargo clippy`, and `cargo fmt` before
declaring done; the lead verifies the tree (`git status`, `git diff --stat`,
a test run); a fresh reviewer reads the diff adversarially and reports
findings split into defects / semantic changes not pinned by a test /
violated invariants / duplication the chunk was meant to remove / doc drift,
plus design observations; the same implementer fixes; a fresh re-reviewer
checks the prior findings and the fixes; when there are no material findings
the implementer commits through the pre-commit hook (never `--no-verify`) and
the lead confirms the commit in `git log`. Read-only audits may run in
parallel with an implementer; two implementers never edit one checkout. A
phase that claims identical semantics also builds the pre-change commit from
`git archive` into the scratchpad and runs both CLIs on hand-written probe
models covering the shapes the corpus lacks, diffing the simulation output.

**Teammate standing rules** (inline in every prompt): TDD; derive test rows
from the enumeration and name uncovered arms; fixtures must be what
production produces; evergreen docs, no changelog sentences; no emoji; no
`Co-Authored-By`; no destructive git (`stash`/`checkout`/`reset`) to toggle a
change for measurement -- use a worktree; run `cargo test` and heavy examples
under `systemd-run --user --scope -p MemoryMax=20G -p MemorySwapMax=0 --collect`;
never two cargo builds concurrently; never pipe `cargo test` through
`head`/`tail`; per-test budget a few seconds, suite under the 3-minute cap;
cite sources for any claim about Vensim/Stella/XMILE; when a discovered
issue is out of the chunk's scope, say so in the report rather than fixing it
silently or dropping it.

**Measurement per phase.** Before the first edit of each phase, and after its
last commit, record on C-LEARN: cold compile retired instructions
(`perf stat -e instructions -- target/release/examples/clearn_profile` with
`CLEARN_PROFILE=compile`, `CLEARN_COMPILE_ITERS` fixed across the pair, build
flags fixed), and the artifact counts `CompiledSimulation::bytecode_profile()`
reports (slots, opcodes per runlist, literals, GFs, temps, views, names,
modules). Name the channel on every row. A cycles number without a
same-session null control is not recorded.

**Test-only monolith (`compiler::Module`, `Project::from_salsa`,
`ModelStage1::set_dependencies`).** Phase 3 adapts it (it must build
`DepShape`s from its `Variable`s) and Phase 8 removes the Stage0/Stage1 memos
it consumes. Whether the monolith itself stays as a differential oracle or is
deleted is a product decision: it is a second implementation of layout and
metadata that cannot lower resolved SCCs, and every compiler change is made
twice because of it. The lead surfaces this to the owner before Phase 8; the
default if unanswered is to keep it compiling and narrow its surface.

**Phase 7 investigation output.** The enumerated list of module-context
decisions and their new homes is appended here by the Phase 7 teammate before
code moves. (Placeholder until then.)

**Phase 1 semantic divergences.** Making the signature table the one statement
of per-builtin facts changed how four edge shapes compile. None occurs in the
corpus (C-LEARN artifacts are identical); each is pinned:

1. A bare arrayed LOOKUP table reference (`LOOKUP(g, t)` with a per-element
   `g`) resolves under pass 0 exactly as a bare arrayed variable does: the
   enclosing apply-to-all element pins the axes it iterates and every other
   axis is a wildcard. So `out[COP] = LOOKUP(g, t)` over `g[COP]` and
   `cell[COP,ROW] = LOOKUP(g, t)` over `g[COP,ROW]` apply each element's own
   table (both were process aborts), and `out[COP] = SUM(LOOKUP(g, t))` over
   `g[COP,ROW]` sums the element's row (was the sum of the whole table). An
   array-valued apply assigned to one slot (`s = LOOKUP(g, t)`) is refused
   with a diagnostic (was a process abort). Pinned by
   `per_element_gf_tests::bare_one_dim_table_in_a_matching_a2a_equation_applies_each_elements_table`,
   `bare_two_dim_table_in_a_matching_a2a_equation_applies_each_cells_table`,
   `bare_two_dim_table_under_a_reducer_sums_the_elements_own_row`, and
   `array_valued_table_apply_assigned_to_one_slot_is_refused_not_aborted`.
2. Every position of an n-ary `MEAN` is `ArgKind::Scalar`, so an array-shaped
   operand is refused (was a process abort in the scalar-mean emitter) and
   `MEAN(a[@1], b[@2])` lowers exactly as `MAX(a[@1], b[@2])` does (was
   uncompilable). Pinned by
   `builtin_signature_tests::n_ary_mean_over_array_operands_is_refused_not_aborted`
   and `n_ary_mean_lowers_its_operands_as_scalars_like_n_ary_max`.
3. `RANK`'s array argument is `Array { whole: true }` like
   `VECTOR SORT ORDER`'s, so it opens the `Expr2` dimension-union gate:
   `out[X,Y] = RANK(a[*] + b[*], 1)` compiles (was `MismatchedDimensions`),
   and the incomparable apply-to-all spelling is refused exactly as the
   `VECTOR SORT ORDER` twin is. Pinned by
   `builtin_signature_tests::rank_accepts_the_cross_dimension_operand_vector_sort_order_accepts`
   and `rank_and_vector_sort_order_refuse_the_same_incomparable_a2a_operand`.
   The same fact closes a pre-existing divergence between the LTM pass-1 gate
   (`db/ltm/compile.rs`) and Pass 1 itself: the gate classified `RANK` as
   non-decomposing while Pass 1 decomposed its argument (GH #995), so an LTM
   fragment embedding `RANK(<computed array>, d)` took the unscoped lower.
   Pinned by `pass1_gate_covers_each_decomposition_builtin`.
4. `ISMODULEINPUT` spelled with zero or two arguments is `BadBuiltinArgs`
   (was `ExpectedIdent`, and silently accepted with the second argument
   dropped). Pinned by
   `ast::expr1::tests::constructor_admits_exactly_the_signatures_arity_range`.

**Phase 2a semantic divergences.** Making temp ids final when issued changed
how one arrayed shape compiles. It does not occur in the corpus (C-LEARN
artifacts are identical); it is pinned:

1. In an arrayed (EXCEPT / per-element) equation, an arm that is *not* the
   hoisting arm -- the EXCEPT default beside a hoisting override, or an
   explicit override beside a hoisting first element -- and whose expression
   holds a Pass 1 temp beside its own hoisted builtin (`SUM(vals[*] * 2) +
   SUM(RANK(bump[*], 1))` beside an arm `SUM(RANK(vals[*], 1))`) read the
   *other* arm's hoist through that temp: the arm's Pass 1 pre-expression was
   emitted once, renumbered past the ids already taken, while the main
   expression came from a second lowering that still numbered its temp 0, the
   id of the hoisting arm's hoist. `out[d]` with that default and override
   gave `[6, 12, 12]` (`6 + 6`) where the same expression as a scalar gives
   `126` (`120 + 6`). With one allocator per fragment the pre-expression and
   the main expression come from one lowering and name one id, so the arm
   reads its own operand: `[6, 126, 126]`. Pinned, with the expected values
   derived from the builtins' rules, by
   `db::temp_allocation_tests::default_arm_beside_a_hoisting_override_reads_its_own_pass1_temp`,
   `explicit_arm_beside_a_hoisting_arm_reads_its_own_pass1_temp`,
   `default_arm_with_two_pass1_temps_reads_both_of_its_own`, and the 2-D
   `two_d_override_beside_a_hoisting_default_reads_its_own_pass1_temp`.

**Risks.** Phase 6(b) and Phase 7 are the two places where artifact shape
changes are expected; both rely on the corpus as oracle and must report opcode
and temp deltas in the ledger. Phase 7 changes salsa keying; the determinism
suites and the execution-count tests are the guard.

## Measured

Ledger rows are appended by each phase's teammate. Every compile-cost number
is retired instructions unless the row says otherwise. A phase row names its
commit by subject line: the row is written before the commit exists, so the
hash is not available to it.

| phase | commit | cold compile Ir | slots | opcodes (flow / stock / init) | literals / GFs / temps / views | notes |
|---|---|---:|---:|---|---|---|
| baseline | `867f2e63` | 10.788 G (median of 9; range 10.778-10.791) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | retired instructions, `perf stat -e instructions` (user space, whole process: parse, one measured compile, five `CLEARN_COMPILE_ITERS` compiles, one VM run), `CLEARN_PROFILE=compile`; release `opt-level=3` + LTO, mimalloc; 371 names, 7 modules; 1174 initials |
| 1 | `engine: one signature table of per-builtin facts` | 10.753 G (median of 4; range 10.748-10.755), -0.30% (interleaved pairs -0.34 / -0.28 / -0.34%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN: every count above, the 371 names and 7 modules, and the full opcode histogram; same channel and flags as the baseline row; the saving is `try_map_ref` lowering `Expr2` builtins without cloning them; four edge-shape divergences, pinned (Additional Considerations, "Phase 1 semantic divergences") |
| 2a | `engine: one temp allocator per variable lowering` | 10.349 G (median of 5; range 10.345-10.353), -3.78% against the Phase 1 commit re-measured in the same session (10.756 G, median of 5, range 10.747-10.759; interleaved pairs -3.75 / -3.78 / -3.74%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN: every count above, the 371 names and 7 modules, the full opcode histogram and the post-fusion stream counts; temp numbering identical on every checked-in fragment golden (no regeneration); same channel and flags as the baseline row. The saving is deleted reconciliation work: the operand materializer walked every fragment's whole expression list into a `HashMap` to find the next free id, the arrayed path lowered every element twice (once to classify, once to keep), the shared-hoist paths re-lowered every element a second time to replay its temp ids, and every re-lowered element tree was walked twice more to shift and re-scan its ids. One corrected shape, not in the corpus: an arrayed arm that is not the hoisting arm and holds a Pass 1 temp beside its own hoist read another arm's hoist through that temp at the Phase 1 commit and reads its own operand on this one (Additional Considerations, "Phase 2a semantic divergences"). Every other probe -- the engine suite, the determinism suites, and hand-written models covering nested hoists, EXCEPT arms in XMILE and MDL, 2-D apply-to-all with reducers, the Phase 1 per-element GF shapes, and the pre-existing refusal of an array-producing builtin inside operand arithmetic -- simulates byte-identically on the pre-change and post-change CLIs |
| 2b | `engine: retire the unemitted opcode families` | 10.365 G (median of 5; range 10.355-10.369), +0.19% against the Phase 2a commit re-measured in the same session (10.345 G, median of 5, range 10.341-10.347; interleaved pairs +0.17 / +0.08 / +0.16%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN: every count above, the 371 names and 7 modules, the full opcode histogram and the post-fusion stream counts (the `bytecode_profile` block is byte-identical); same channel and flags as the baseline row. The compile measurement embeds one VM run, and the delta is that run's: on the run channel (`CLEARN_PROFILE=run CLEARN_RUN_ITERS=20`, retired instructions, same binaries) the identical artifact executes 31.936 G against 31.654 G (medians of 5; interleaved pairs +0.87 / +0.92 / +0.90%), +0.9% or about 13.4 M instructions per run, which is 13.4 M of the compile channel's 19.8 M and leaves a +0.06% residual inside that channel's floor. The run delta is dispatch-loop codegen, not emitted work: the opcode stream is identical, `eval_bytecode` lost sixteen arms and shrank from 63,380 to 52,520 bytes, and a sampled profile of the run loop places the delta in `eval_bytecode` and the `RuntimeView` helpers LLVM inlines into or splits out of it (`offset_for_iter_index` folded in, `dense_linear_start` split out) -- the codegen-perturbation class `engine-performance.md` "Measuring a change" records for this function. Accepted by the owner as a simplicity trade and not investigated further; a perf pass follows this branch. Sixteen `SymbolicOpcode` variants, their `Opcode` twins, VM and wasm arms, `ByteCodeContext.arrays` / `.subdim_relations`, and the `BeginIter` flat-offset precompute whose only reader was `LoadIterElement` are gone; the `dead_code` lint, denied under clippy, is the standing pin (GH #612). Semantics: the engine suite (lib 5657, integration 767), the wasm parity corpus and the 12-repeat determinism suites are green, and 43 hand-written probes (the Phase 1 and 2a probe sets plus the array corpus models) simulate byte-identically, exit codes included, on the pre-change and post-change CLIs |
