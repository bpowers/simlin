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

1. **One fragment compiler.** `lower_fragment(&FragmentInput, is_initial) -> Result<Var>`
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
   deleted. `compiler/array_operand.rs` is the one pass that materializes an
   array value into a temp -- the array-producing builtins, the per-element
   arrayed-GF applies, and the computed array operands -- and it is the only
   caller of `TempAllocator::alloc`. It runs after subscript resolution, reads
   the positions off `BuiltinFn::signature()`, and decides once-per-equation
   against once-per-element by structural identity of the lowered body.
   `Expr3` is a structural rewrite with no temp variant, and the
   `compiler/mod.rs` A2A hoisting machinery is gone.
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
   `SourceVariable` fields; `datamodel_variable_from_source` is gone from every
   parse path, as is the duplicated field extraction in `db/sync.rs`. One
   re-assembly is left, and is confined to the macro-registry build
   (`db::macro_registry::macro_body_variable`), whose consumer
   `MacroRegistry::build` walks whole `datamodel::Model`s.
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
    `model_stage1` (unit checking, the database-free `ModelStage0::new_in_project`
    oracle tests) is moved to per-variable queries and those memos are deleted.
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

**Fragment input (`compiler/fragment.rs`, Phase 3).**

```rust
pub struct DepShape { pub dims: Vec<Dimension>, pub kind: DepKind }   // dims empty = scalar
pub enum DepKind { Var, Module { shape: Arc<ModelShape> } }           // Var: aux, flow, stock, table, helper
pub struct ModelShape { pub vars: IdentMap<Ident<Canonical>, ShapeEntry>, pub n_slots: usize }
pub struct ShapeEntry { pub offset: usize, pub shape: DepShape }      // nested module: its own ModelShape
pub struct FragmentInput<'a> {
    pub target: Variable,                                  // the variable in its Expr2 form
    pub deps: IdentMap<Ident<Canonical>, DepShape>,        // every referenceable name, the target included
    pub tables: HashMap<Ident<Canonical>, Vec<Table>>,
    pub module_inputs: BTreeSet<Ident<Canonical>>,
    pub model_name: Ident<Canonical>,
    pub dimensions: &'a [Dimension], pub dimensions_ctx: &'a DimensionsContext,
    var_sizes: VarSizes,                                   // derived by reference_extents(&deps) in new()
}
impl FragmentInput<'_> { pub fn new(target, deps, tables, module_inputs, model_name, dimensions, dimensions_ctx) -> Self;
                         pub fn emit_ctx(&self) -> ModuleCtx<'_> }
pub fn lower_fragment(input: &FragmentInput<'_>, is_initial: bool) -> Result<Var>;
pub fn reference_extents(deps: &IdentMap<Ident<Canonical>, DepShape>) -> VarSizes;
// db/: the four constructors
fn explicit_fragment_input(db, var, model, project, module_input_names) -> ExplicitFragment   // Fatal { unit_diags, fatal_diags } | Ready { unit_diags, input }
fn implicit_fragment_input(db, meta, model, project, module_input_names) -> Result<FragmentInput, ImplicitInputError>
fn ltm_fragment_input(db, var_name, equation, model, project) -> Result<FragmentInput, String>
fn ltm_implicit_fragment_input(db, meta, model, project, module_input_names) -> Option<FragmentInput>
#[salsa::tracked(returns(ref))] fn model_shape(db, model, project) -> Arc<ModelShape>
fn module_input_set(prefix, refs) -> BTreeSet<Ident<Canonical>>   // the one owner of "which ports does this wiring bind"
```

`lower_fragment` is per phase (`is_initial`), so an emitter lowers only the
phases its runlist membership admits and an LTM synthetic variable lowers its
one flow phase; one `FragmentInput` serves every phase and the emission. A
module dependency's `DepKind::Module` carries the sub-model's `ModelShape` --
layout slot plus shape per variable, recursively for nested instances -- and
`Context::resolve` is the one reader of it: a plain name is a dependency
shape, a `·`-qualified name walks the shapes accumulating the slot offset.
`DepKind` has no `Stock` variant and no `model_name` because lowering reads
neither (a stock and an aux are both one slot per element; a module's model is
identified by its shape), and `ModelShape` rather than `VariableLayout` because
the resolver needs each sub-model variable's dimensions and nested-module
identity, which a layout does not carry.

**Variable (`variable.rs`, Phase 4).** As implemented:

```rust
pub struct Variable<MI = ModuleInput, E = Expr2> {
    pub ident: Ident<Canonical>, pub units: Option<datamodel::UnitMap>,
    pub eqn: Option<datamodel::Equation>,          // None for a module instance
    pub errors: Vec<EquationError>, pub unit_errors: Vec<UnitError>,
    pub kind: VarKind<MI, E>,
}
pub enum VarKind<MI = ModuleInput, E = Expr2> {
    Stock { init_ast: Option<Ast<E>>, inflows: Vec<Ident<Canonical>>,
            outflows: Vec<Ident<Canonical>>, non_negative: bool },
    Aux   { ast: Option<Ast<E>>, init_ast: Option<Ast<E>>, tables: Vec<Table>,
            non_negative: bool, is_flow: bool, is_table_only: bool },
    Module { model_name: Ident<Canonical>, inputs: Vec<MI> },
}
pub struct VariableSource<'a> {                    // borrowed; two producers
    pub ident: &'a str, pub equation: Cow<'a, datamodel::Equation>,
    pub kind: SourceVariableKind, pub units: Option<&'a str>,
    pub gf: Option<&'a datamodel::GraphicalFunction>,
    pub inflows: &'a [String], pub outflows: &'a [String],
    pub module_refs: &'a [datamodel::ModuleReference], pub model_name: &'a str,
    pub non_negative: bool, pub can_be_module_input: bool,
    pub active_initial: Option<&'a str>,
}
pub struct ParseContext<'a> {                      // was nine parameters
    pub dimensions: &'a DimensionsContext, pub units_ctx: &'a units::Context,
    pub model_var_names: Option<&'a HashSet<Ident<Canonical>>>,   // LTM parse only
    pub macro_registry: Option<&'a MacroRegistry>,
    pub enclosing_model: Option<&'a str>,
}
impl<'a> ParseContext<'a> { pub fn new(dimensions, units_ctx) -> Self }   // no model context
pub fn parse_var<'a, MI, F>(ctx: &ParseContext<'_>, v: impl Into<VariableSource<'a>>,
                            implicit_vars: &mut Vec<datamodel::Variable>, module_input_mapper: F)
                            -> Variable<MI, Expr0>;
pub fn variable_source(db: &dyn Db, var: SourceVariable) -> VariableSource<'_>;  // db/input.rs
impl<'a> From<&'a datamodel::Variable> for VariableSource<'a> { .. }
```

`eqn` moves onto the struct even though a module has none (`None` there), because
every consumer that reads it already declined for modules. `equation` is the one
`Cow` field: the salsa producer substitutes a conveyor stock's §7.2 explicit init
list with its constant raw-sum placeholder, which the `datamodel::Variable`
producer deliberately does not (it parses synthesized implicit variables and the
`ModelStage0` oracle, neither of which is ever a conveyor). `parse_var` and
`parse_var_with_module_context` collapse into the one `parse_var`; `ParseContext::new`
is the former's no-model-context spelling.

**Dimension matcher (`dimensions.rs`, Phase 6).**

```rust
pub enum AxisMatch { Exact, Mapped { via: CanonicalDimensionName }, Subdimension, BySize }
pub struct Axis<'a> { pub name: &'a str, pub len: usize, pub indexed: bool }
pub trait AxisRelations {           // every method defaults to "no relation"
    fn maps_to(&self, from: &str, to: &str) -> bool;
    fn mapping_parent_of(&self, from: &str, of: &str) -> Option<CanonicalDimensionName>;
    fn common_mapping_target(&self, a: &str, b: &str) -> Option<CanonicalDimensionName>;
    fn is_subdimension(&self, child: &str, parent: &str) -> bool;
}
pub fn match_axes_partial(source: &[Axis<'_>], target: &[Axis<'_>], relations: &dyn AxisRelations)
    -> Vec<Option<(usize, AxisMatch)>>;
pub fn match_axes(source: &[Dimension], target: &[Dimension], ctx: &DimensionsContext)
    -> Option<Vec<(usize, AxisMatch)>>;
```

Precedence is exact name, then declared mapping (either direction, or both
mapping to a common dimension), then subdimension, then size, staged flat
across all source axes and allocated one-to-one. `match_axes` is the total
answer over two declared dimension lists; `match_axes_partial` is the general
one, over bare axes, because two callers have no `Dimension` to give it --
`compiler::mod`'s view join compares two `ArrayView`s, which carry a name and
a length per axis, and `ast::Expr2`'s bounds unification reaches its dimension
facts through `Expr2Context`.

Which RULES can fire is the caller's `AxisRelations` projection, and that is
the part the plan's original "union of what the existing matchers do" could
not be: two of the rungs are INDIRECT correspondences, where the paired
element is not the target axis's own ordinal, and a caller that resolves an
element by ordinal reads a neighbouring row if it acts on them. So
`DimensionsContext` answers the three mapping questions; `DirectMappingsOnly`
withholds `mapping_parent_of`; `SubdimensionRelations` adds `is_subdimension`;
`NoAxisRelations` answers nothing. `dimensions::axis_match_tests` is the
table-driven test, one row per arm of each replaced matcher, and it rows the
projections too.

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
`module_input_set(prefix, refs)` owner used by `enumerate_module_instances_inner`
-- `build_var_info`'s `·` splits classify dependency strings rather than
extract wiring, and Phase 8's `DepRef` replaces them); `Cargo.toml` drops
`bumpalo`.

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
the redundant `canonicalize` calls on already-canonical inputs in
`ClassifyVisitor`, and `NamedDimension::index_of` for the callers that already
hold an `Ident<Canonical>` (the case-folding `get_element_index` stays for the
rest). The `compiler/context.rs` reference resolver is Phase 3's
`Context::resolve`, not this phase's, and `normalize_subscripts3`'s
canonicalize-on-canonical calls are deferred to Phase 6a, which rewrites
`compiler/subscript.rs`'s inline matching anyway.

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

**The materializer (part b), as designed.** Lowering is structural all the
way down: `Expr2 -> Expr3` resolves wildcards and gives every bare array
reference its subscripts, `Context::lower` returns one `compiler::Expr` per
equation body, and `Var::new` expands an apply-to-all or arrayed equation into
one assignment per element with `expand_per_element`. Nothing before
`compiler/array_operand.rs` allocates a temp. That pass runs once over the
whole lowered, constant-folded fragment -- every element's code at once --
and is the only caller of `TempAllocator::alloc`. It reads the signature
table for its positions: an `ArgKind::Array` operand that is not already a
view is moved into a temp, a `ResultKind::Array` call (the five
array-producing builtins) is moved into a temp wherever it appears, and a
`LOOKUP` whose table operand is a multi-element array (the per-element
arrayed-GF apply) is the one non-`ResultKind::Array` call that also writes a
temp. A materialized value is read back whole where an array is wanted and at
the assignment's own element where a number is; a scalar equation has no
element to read, so an array value in its one slot stays the loud codegen
refusal.

Once per equation against once per element is decided by structural identity
of the lowered body, source positions included: the elements of an
apply-to-all body or an EXCEPT default are lowerings of one text under
different active subscripts, so two elements' bodies are equal exactly when
nothing in the body resolved through the element, and two explicit arms
share only when they spell the body identically at the same offset. A body two
or more elements read is SHARED -- one id, its `AssignTemp` emitted ahead of
the element code -- and a body one element reads is RECYCLED on an id the
elements reissue (`TempAllocator::element_scopes`); shared ids are numbered
below the recycled range so no element can clobber one. Sharing is sound
because a fragment writes only its own variable's slots and its own temps, so
nothing evaluated between two elements can change what a shared body read.
A subscript naming a dimension is an `IndexOp::ActiveDimRef` all the way to
`compiler::subscript`, which allocates the active positions ONE TO ONE across
a reference's subscripts and resolves the element through the one mapped read;
`project_var_index_to_temp` pairs a temp's axes to the variable's by the same
two rules, which is what makes `o[D,D] = square[D,D]` read the cell rather
than the diagonal on every spelling.

A resolved recurrence SCC is the one place a shared temp meets reordering.
`db::assemble::segment_member_by_element` is the single statement of where a
member's code splits: its PROLOGUE and one segment per element, together
with the set of elements that READ a prologue temp. The prologue is the
leading run of whole temp-writing blocks whose temp two or more elements read,
or a later prologue block reads (a shared body materialized from a shared
operand) -- decided in reverse, so a once-written temp only the first element
materializes stays in that element's segment beside the shared ones the
materializer emits ahead of it. The element graph wires the prologue's
current-value reads into exactly those readers, and `combine_scc_fragment`
emits the prologue once, immediately before the first of those readers in
`element_order`, refusing loud-safe if the order it was handed does not
evaluate every in-SCC read first. An element the prologue reads but that
reads nothing of it therefore runs before it, and a body that reads the
recurrence's own readers is an element self-loop rather than a silently
misplaced write.
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
`units_infer.rs`, `units_check.rs`, and the database-free
`ModelStage0::new_in_project` oracle the `stages_tests` compare against) moved
to per-variable queries and the memos deleted; engine `CLAUDE.md`,
`docs/architecture.md`, `docs/design/engine-performance.md` rewritten as
current state.

**Dependencies:** Phase 7 (captures change what an implicit dependency is).

**Done when:** `rg "BTreeSet<String>" src/simlin-engine/src/db` finds no
dependency set; `model_stage0`/`model_stage1` do not exist; docs describe the
pipeline as it is; final ledger row; this document's ledger complete.

**Phase 8.1: the `Expr2` lowering scope reads dependency shapes.**
`ast::LoweringScope { dimensions, shapes, model_name }` is what one equation's
`Expr0 -> Expr2` lowering knows: the project's dimensions and, for every name
the equation can reference, the `DepShape` the fragment compiler resolves it
through -- the same `FragmentInput::deps` map, which each constructor builds
first and then lowers under, so the `Expr2` tier and the compiler read one
answer for a dependency's dimensions and a parse-synthesized helper lowers
under the shapes its parent's equation lowers under. A module output `m·x` is
never a key and lowers without bounds: `compiler::Context` resolves it through
the instance's `DepKind::Module` shape, a cross-module read has one resolver,
and the artifact stays byte-identical. The whole-model `model_stage1` lowers
each variable under the model's own variables' shapes
(`ModelStage0::lowering_shapes`), so it reads no other model's stage.
`model::lower_variable`'s module arm resolves an instance's wiring through
`db::build_module_inputs`, the one owner of that wiring; a module input's
`src` is not validated at lowering -- the user-facing `BadModuleInputSrc` is
`model_module_wiring_diagnostics`', read off the salsa inputs -- and the
unit-inference scope (`model_scope_models`) follows `model_name` edges only.
Every lowering runs under the shapes of the names its equation references;
the dependency classification runs on the typed `Expr1` and needs no scope.

**Phase 8.2 and 8.3: one lowered memo per variable.** Every variable is
lowered to `Expr2` exactly once, by a per-variable salsa memo:
`db::lowered_source_variable(var, model, project)` for an explicit variable
and `db::lowered_implicit_variable(model, project, name)` for a
parse-synthesized helper, each returning an `Arc<Variable>`. The memo lowers
under the DIMENSIONS of the names the equation references, resolved through
one name resolver (`db::var_fragment::DeclaredName`, over the per-name
firewall queries); the fragment constructors resolve the same names to the
compiler's shapes (a module instance's sub-model layout included) and BORROW
the memo (`FragmentInput::target` is a `Cow`), so compiling a variable retains
no second lowered tree, and a dependency's graphical-function tables reach a
fragment through the tracked `db::variable_tables` projection, so an
equation-only edit recompiles the edited variable alone. The two whole-model
consumers read handles: `db::model_lowered_variables(model, project)` is an
`Arc<HashMap<Ident, Arc<Variable>>>` assembled from the memos -- the one map
builder, whose entry for an element-scoped helper is the memo's element-pinned
projection, the read the describers classify -- and both the unit pass
(`check_model_units`, through a stack-local `units_check::UnitModel` per scope
model) and the LTM describers and causal graphs read it;
`db::lowered_variable_by_name` is the salsa firewall over it. The unit
inference scope, `db::model_scope_models`, is an iterative name-only worklist
over the explicit `Module` variables and `model_implicit_var_info`'s module
entries, so a module cycle -- which the unit pass reaches -- terminates; the
memo never reads a sub-model's layout (a recursive query) for the same reason,
and the pinned projection, which resolves a helper's heads to the compiler's
shapes and so reads that layout for a module head (`x[d] = SMTH1(m.out + y[d],
1)`), is taken only where `project_module_graph(..).cycle_error_from(..)` is
`None`: under a cycle the map holds the memo's handle, which the unit pass
reads identically (a subscript index carries no units) while the diagnostics
gate reports the cycle
(`units_tests::a_module_cycle_reached_through_a_per_element_helper_still_unit_checks`).
A rename (`patch.rs`) reads neither tier: it is syntactic, over each equation
string as written, because the parse memo's tree is the expanded one and the
lowered tree is absent for an equation the compiler refuses. The wiring of a
module instance resolves under the model's canonical name, so a root model
spelled `Main` wires its instances as `main` does. There is no whole-model
lowered copy and no database-free lowering oracle: what pins unit semantics is
the unit suites over production diagnostics, and what pins the sharing is
`Arc::ptr_eq` against the production memos plus `ProbedDb` body counts
(`db::lowered_variable_tests`, `db::units_tests`). The memos are what a
compile-only caller retains without a diagnostics pass to amortize them
(pysimlin's `Model.simulate()` used alone, a C/Go embedder holding a project
without `get_errors`, serve's transient `simulate_sync` as peak only: +6.5 MiB
on C-LEARN, ledger rows `8.2+8.3` and `8.3b`). An `Expr2` node's bounds slot
is `ast::NodeBounds`, an `Option<Box<ArrayBounds>>` stated once: a bound is
`None` on every scalar subexpression, so an inline `ArrayBounds` (72 bytes)
would be paid by every node of a retained tree for the few that carry one,
and the box makes a node 64 bytes and a `None` one pointer, in the memos and
the LTM handle maps alike; `Expr3`, the structural rewrite of `Expr2`, carries
the same slot and copies it as it stands. Readers go through
`get_array_bounds`, a copy clones the slot, and the box is spelled only where
a bound is produced (`Expr2::from` and the compiler's bare-reference rewrite),
so no consumer knows the representation. What the memos retain beside the
trees -- the `Variable` around each (its `eqn`, `units`, `tables` and `errors`
duplicate the parse memo's), the resolved heads and the handle map -- is the
larger share of the compile-only row and is the next slimming.

**Phase 8.5: one structured dependency representation, classified once on
`Expr1`.** `variable::classify_dependencies` is the one dependency walk, over
a projection (`DepExpr`) of either typed tier: the dependency query runs it on
the variable's `Expr1` (`ast::typed_ast`, the first half of `lower_ast`: builtins
resolved and `dimension·element` spellings folded, before any array bound
exists, which is all the walk reads) and LTM's per-slot readers run it on a
retained `Expr2` subtree. The typed tier runs twice per variable -- once in the
dependency query, once inside the lowering memo's `lower_ast` -- and that
second typing is what stands where the base lowered every variable to `Expr2` a
second time, under an empty scope, for its dependencies. The walk records each
read as a `DepOccurrence { ident, lag }` -- `Current`, `Previous` (a `PREVIOUS`
argument) or `Initial` (an `INIT` argument and a `PREVIOUS` fallback, which
the initials phase populates) -- and each `LOOKUP` holder as a typed table
reference beside the reads.

`db::variable_direct_dependencies` attaches the phase and resolves each name
into a `DepRef { target: DepTarget { module_path, variable, stock_output },
phase, lag }` through `DepScope::resolve`, reading the owning model only
through the per-name projections: the head of a `·`-spelled name is a hop only
where it is a module instance -- an explicit `Module` variable, the parse's own
or a sibling parse's implicit instance -- every further segment but the last a
`Module` variable of the model the previous hop instantiates
(`project_model_by_name`, `model_variable_by_name`), the last segment the
variable whose kind in that model the same walk reads, a leading `·` XMILE's
parent-scope spelling read as the bare name, and a spelling that fails a hop
one local name (that `·` stripped) the compiler refuses at lowering (which is
what a `dimension·element` of an undeclared dimension is; a declared one folds
before the walk). One `DepRefs` value carries the reads of an explicit
variable and of a helper alike, with one set of projections: `phase`, `heads`,
`reads_local`, `dt_previous_only` (`Previous - Current`).

Every consumer reads those projections and none splits a name: the dependency
graph's `ordering_edges` (a `Dt`/`Current` read orders the dt phase unless its
target is a sub-model stock at any depth, which the dt phase reads from the
prior step; every init-phase read but `Previous` orders initials; `Dt`/`Initial`
reads seed the initial snapshot), the causal-edge builder, the output-port
scans, the pinned-loop and statelessness gates, the lowering memos' `heads`
(the resolved `DeclaredName` of each read's head, tables and stock flows
included), the layout (a module read is drawn to the module box) and the
libsimlin surface (`simlin_model_get_incoming_links` lists variables of the
model: a module read lists nothing, a module instance lists its input
sources). The causal-edge builder records the sub-model outputs each node
reads once (`CausalEdgesResult::module_outputs_read`, shared by the
`CausalGraph`s built from it), and both LTM modes select a loop's exit port at
a reader from it (`unique_module_output`: the one output of the module the
reader reads, an equation and a module instance wired from it alike; several
are ambiguous). Run invariance is the compiler-local walk
(`compiler/invariance.rs`, every reference invariant, no callback) joined with
the `Dt`/`Current` reads and the called tables. `Expr2` issues no temp id: an
`ArrayBounds::Temp` is a shape, and the one temp counter is the fragment's
`TempAllocator`.

**Phase 8.5 semantic divergences.** Seven, each pinned. (1) The previous-only
relation is `Previous - Current`: an `INIT(x)` read -- an `INIT` beside the
`PREVIOUS`, or the fallback of the `PREVIOUS` itself -- is a frozen snapshot,
not an instantaneous read, so it does not cancel the lag. Scheduling is
unchanged (an `Initial` read never ordered the dt phase either way); the
consequence is LTM's: the stockless cycle `y = x * 2`, `x = PREVIOUS(y,
INIT(y)) + 1` is state and instrumented (the `x→y` and `y→x` link scores and
the loop score read 1 from the second step) where it was stateless and
bypassed. XMILE 1.0 3.5.6: `PREVIOUS(price, 0)` "returns the value of price in
the last DT, or zero in the first DT", and INIT is the "initial value (i.e.,
value at STARTTIME) of a variable" -- after the first DT `x` reads `y`'s
last-DT value and the fallback is a constant, so the loop carries one DT of
memory. Such a cycle compiles only with its init phase broken: the fallback is
an init-phase read of `y`, whose initial value otherwise reads `x`, so `y`
carries an initial equation of its own (without one both binaries refuse the
model as `circular_dependency`). Pinned by
`dep_ref_tests::an_initial_read_does_not_cancel_a_previous_only_lag` and the
`both_lagged_*` / `previous_fallback_*` rows of
`variable::test_classify_dependencies_matrix`. (2) Run invariance reads the
source relation, so a conditional whose literal condition the compiler folds
away keeps both arms' reads: a flow such as `IF 1 THEN k ELSE TIME` stays in
the per-step program (`db::invariance::constant_selected_dynamic_branches_are_conservatively_variant`);
the direction is under-hoisting, which changes no number, and no corpus model
has the shape. (3) A qualified spelling no hop proves (`ghost.x` with no
module `ghost`, or `m.x` where `m` is an aux) is one local name the model
declares nowhere, reported as `UnknownDependency` at its reference site rather
than as the compiler's `DoesNotExist`
(`dep_ref_tests::an_unproven_qualified_spelling_is_one_local_name`); the model
does not compile either way, and no corpus diagnostic moves. (4) A module
input wired from a lookup-only table is refused at the instance with
`LookupReferencedWithoutArgument` under XMILE's parent-scope spelling
`from=".g"` as under the bare `g`
(`lookup_only_tests::module_input_wired_from_lookup_only_is_compile_error`):
an input port copies its source's slot each step and a table has none, so the
wiring is the bare read the check names, where a leading `·` the base's
dependency set kept un-stripped let the wiring through to an opaque assembly
refusal (`failed to compile fragments for variables: profile`). The corpus's
one such model, `sir_social_distancing_mixnot.stmx`, is refused either way (an
unknown builtin and a sub-model conveyor) and gains that one row. (5) A
qualified read of a sub-model STOCK is a prior-step read at any depth:
`stock_read = m.n.level` does not order `stock_read` after `m` in the dt
phase, so `feeder = stock_read * 2` wired into `m` runs before the instance
and `m` reads the current `feeder`. The base resolved one hop (`n·level` looked
up whole in `mid`, never found), kept the edge, and with `m`'s input fed from
`feeder` closed the ordering cycle `stock_read -> m -> feeder -> stock_read`
that the cycle relation cannot see (a module is a sink there); the sort
emitted `m` before `feeder`, so `m·feed` read an unwritten slot every step
after the first (`2, 0, 0, ...` where `feeder` is `2, 2.2, 2.42, ...`) -- the
#591-c1 stale-input class at depth two, where the one-hop `m.level` was
already right. A nested stock reader with no other dt read joins the initials
runlist as a one-hop reader does. Pinned by
`dep_ref_tests::a_nested_stock_read_is_a_prior_step_read_at_any_depth`; no
corpus model has the shape. (6) The output-port scan
(`ltm/loops.rs::find_model_output_ports`) reads every helper's `Dt` reads, so a
sub-model output whose only reader is a stdlib instance with bare-identifier
arguments (`sm = SMTH1(m.a, tau)`: no hoisted-argument helper, the instance
wired straight from `m·a`) is a port: the sub-model is instrumented
(`$⁚ltm⁚path⁚inp⁚0`, `$⁚ltm⁚composite⁚inp`, the `inp→a` / `inp→b` link scores)
and a loop through it selects the `level→m⁚via⁚a` exit override. The base
scanned a variable's helpers only when one of them was not a module instance,
while `db.rs::model_module_output_ports` already scanned uniformly. LTM-only
and additive: every user series identical. Pinned by
`ltm_module_tests::a_stdlib_instance_with_bare_arguments_reads_an_output_port`;
no corpus model has the shape. (7) `simlin_model_get_incoming_links` on a
module instance lists its input sources under the parent-scope spelling
`from=".driver"` (as `driver`) as it does the bare `k`; the base matched the
un-stripped `·driver` against the model's variables and dropped a real
dependency by spelling. Pinned by the module row of
`test_get_incoming_links_lists_variables_of_the_model_not_module_reads`.

**Loops That Matter: the generated-text boundary and one module-instance
owner.** LTM is a consumer of every unification above, and its compile path
has exactly one place where engine-generated text is parsed: a generator
prints the ceteris-paribus guard form around a wrapped `Expr0` and
`db::ltm::equation::LtmArm::new` parses that text once, at the emitter, into
the arm the fragment compiles (the GH #965 boundary). Everything before that
point is typed. A helper carries no `Variable::eqn` (`capture.rs`: its body is
the subtree, and the generators read a target's axes and body off its lowered
`ast()`, keeping the `eqn` arm for source variables, whose text is
user-authored); a target whose `Expr0 -> Expr2` lowering failed has no body to
differentiate and is declined loudly (`PartialEquationErrorKind::MissingTypedTarget`,
one warning, no score, dependent loops dropped -- the edge into it stands
because dependencies are classified on the typed tier, and the project does
not compile); an aggregate node carries its reducer as the classified
`BuiltinFn<Expr2>` and projects it to `Expr0` (`AggNode::reducer_expr0`) for
its own equation (`LtmArm::from_typed`) and for the feeder link scores, so the
feeder generators are infallible on the text they used to parse; the
per-element emitters pin element subscripts on the wrapped tree
(`subscript_idents_in_expr0`) and read their completeness guard
(`unresolvable_dimension_index`) off the one arm they emit, so every arm is
parsed once. `AggNode::reducer_key`, the canonical printed reducer, survives
as an identity (the dedup key, the reference-site IR's routing key, the
wrap's live-reducer match, the polarity substitution) because `Expr2` is `Eq`
but not `Hash`; nothing parses it. One spelling of a generated equation's
builtins (`PREVIOUS(..)` as the wrap inserts it) reaches the characterization
goldens, so a pinned arm and an unpinned one read alike.

The LTM constructors lower under shapes like every other constructor:
`lower_ltm_variable` classifies a generated equation's reads on its typed
`Expr1`, resolves them through `DepScope::resolve` to the same `DepShape` map
the fragment compiler reads (`ltm_dep_shape`: a model variable's, an LTM
helper's, an LTM synthetic variable's), and lowers under that map; a helper an
LTM equation's parse synthesized is always a capture (the equation is generated
from an already-expanded tree and contains no module-function call), so
`db::ltm::LtmImplicitVarMeta` says captures only and the module universe under
LTM is the source models' own. That universe has one owner:
`db::assemble::enumerate_module_instances` walks a model's explicit `Module`
variables and the instances its parses synthesized as one candidate shape
(`ModuleInstanceCandidate`), records each under `(target model, bound-port
set)` with `module_input_set`, and descends into a target once. A reference's
port is one reading, `port_of(dst)` (the `dst` with the instance's prefix
stripped); the bound-port set and the lowered wiring (`build_module_inputs`)
are projections of one rule over it (`bound_port`: `port_of(dst)` when the
`src` is outside the instance's namespace), so an instance is compiled under
exactly the ports its inputs write, and `model_module_wiring_diagnostics`
validates `port_of(dst)` for every reference -- internal ones included -- and
warns (`BadModuleInputSrc`) about a reference to one of the instance's own
ports whose `src` is inside the instance's namespace, which binds nothing. A
`src` inside the namespace with a `dst` in ANOTHER instance is the connect a
writer records on the source instance (Stella's `<connect
to="lynxes.hare_density" from="hares.hare_density"/>` on `hares`, five corpus
models), which the `dst` check reports; it is not an internal reference. XMILE
1.0 section 4.7.1 places every connection at the lowest common ancestor of the
submodel hierarchy, which is where an instance-qualified `from` arises;
whether a connect from an instance to that same instance has a defined meaning
is unverified. The other derivations of a
target model are different facts and stay separate: `project_module_graph`
(explicit `Module` kinds and target names, parse-free, the cycle gate every
recursive query consults), `module_functions`' macro-registry cycle gate
(macro-to-macro edges), `DepScope::instance_target_model` (which model a
READ's head instantiates), `model_scope_models` (the unit pass's target
closure, no ports), `compute_layout`/`flattened_offsets` (an instance's slot
count and key prefix), and `model_causal_edges`' `dynamic_modules` (an
implicit instance's target for the causal graph); none derives a bound-port
set. C-LEARN's plain and LTM artifacts, the corpus sweeps in both modes, the
corpus-wide LTM variable sets and detected loops, and the LTM value goldens
are identical against the base; the natural shapes that move are the
divergences below.

**Phase LTM semantic divergences (V9a).** Four, each pinned. (V9a-1) A generated
equation lowers under the shapes of its reads, so a frozen-whole reducer
over a BARE arrayed argument in an apply-to-all body lowers as execution
lowers the target: per element. `x[region] = other + SUM(pop * w[*]) / 1000`
reads `pop[region]` under `SUM` (the plain spelling's rule; the wildcard
co-source `w[*]` keeps the reducer out of the aggregate hoist, where a plain
`SUM(pop)` is hoisted like its iterated twin), and the
ceteris-paribus partial for `other -> x` freezes the reducer whole into a
structural capture over `[region]` with body `sum(pop * w[*])`; lowered under
shapes that capture is `2 * pop[e]` per element, as the executed `x[e]` reads
it, where the bounds-free lowering summed the whole array (600 against a
read of 200) and scored a link that moves `x` by one part in a hundred far
past 1. Direction: the base's number was wrong. Pinned by
`ltm_unified_tests::a_frozen_whole_reducer_over_a_bare_arrayed_argument_lowers_per_element`
(the capture's series equals `2 * pop[e]`, `x[e]` equals
`other[e] + 2 * pop[e]/1000`, the score is bounded by 1); no corpus model has
the shape (C-LEARN's LTM artifact is identical). The same fixture's `w -> x`
edge, with `w` varying, is V9b-5. The same mechanism has a loud face on a project that
does not compile: an LTM score's copy of a target body that the compiler
refuses under shapes now fails the way the target fails, where the
bounds-free copy compiled and read a whole-array value into a score for a
project that never simulates (one or two more `failed to compile; constant 0`
discovery-mode warnings on such a project; no compiling model affected).
(V9a-2) A model with a reference internal to a module instance
(`<connect to="bridge.input" from="bridge.output"/>`) compiles, the reference
binding nothing and warned as `BadModuleInputSrc`; with two owners of the
bound-port rule the instance's compilation identity counted the port while
its wiring wrote nothing, and `Vm::new` panicked looking up the compiled
child (`vm.rs` `key_to_idx`, "no entry found for key") -- a model the base
refused with an abort now compiles. Pinned by
`assemble_tests::internal_module_reference_is_not_a_bound_input`,
`assemble_tests::an_xmile_internal_module_reference_compiles_and_binds_nothing`
and `module_wiring_tests::internal_src_warns_that_it_binds_nothing` (through
`open_xmile`, alone and beside a bound port), with
`module_wiring_tests::a_cross_instance_reference_on_the_source_instance_is_a_dst_report_only`
pinning that the warning is not raised for a cross-instance connect recorded
on the source instance (the corpus's only instance-qualified sources; no
corpus model carries an internal reference, and every corpus model's
diagnostics are unchanged). (V9a-3) The assembly refusal of a project with several missing
module targets names the first in name order (`alpha_missing` before
`zeta_missing`, whatever the declaration order) rather than the first in
`HashMap` order; pinned by
`assemble_tests::a_missing_module_target_is_refused_in_name_order_per_namespace`
for both namespaces. (V9a-4) A target whose `Expr2` lowering failed is
declined with a `Warning` (`MissingTypedTarget`) beside the target's own
`MismatchedDimensions` error, which `simlin simulate --ltm` prints on a
stateful model where the base printed the error alone and silently emitted a
`0`-bodied score; `first_error_code` is unchanged everywhere (a stateless
model takes `model_ltm_variables`' early return and reaches neither). Pinned
by `ltm_unified_tests::a_target_whose_lowering_failed_is_declined_not_scored`
(an aux target, a flow target and an element-scoped helper target, with an
unaffected edge's score still emitted).

**Loops That Matter: the IR describes the read execution performs.** Every
dimension-named subscript the compiler resolves through
`build_view_from_ops` -- the element's name on the source axis first, then
the declared element map in either declaration direction -- is described by
one correspondence, `DimensionsContext::executed_read_correspondence`
(`resolve_mapped_read` per element, no admission gate of its own), read by
the axis classifier (`ltm_agg::classify_axis_access`), the aggregate slot
remap (`iterated_axis_slot_elements`), the per-element row derivation
(`per_element_row_for_target`) and the dependency pins
(`dep_element_pins`, per axis and per spelling). Every bare arrayed read is
described by one pairing of the two declared dimension lists,
`db::bare_axis_pairing` (`match_axes_partial`, the compiler's own matcher,
with each mapped pair carrying its executed correspondence), under the
relations the spelling's own lowering consults (`db::BareSpelling`): pass
0's `DirectMappingsOnly` for a read in an equation body, which withholds a
mapping declared on a parent dimension, and the full context for a stock's
flows (`get_implicit_subscript_off`). It is read by the element graph's
`expand_same_element`, discovery's from-node projection, the arrayed
score's admission (`link_score_dimensions`, which pairs a stock's
structural inflow/outflow edge under the wiring's relations and every
other edge under the equation's) and the dep pins' bare row; the
flow-to-stock score spells a flow declared over other dimensions than its
stock's bare, the reference the compiler resolves through the wiring's
pairing (a flow the compiler refuses to integrate -- an arrayed flow of a
scalar stock, `array_reference_needs_explicit_subscripts` -- has no
executed read, and its fragment is refused with the stock's own update).
The two meet in `db::ltm_ir`: an all-iterated subscript is `Bare` exactly when
the lists reproduce the pairing it spells, and `PerElement` carrying its
axes otherwise; a reducer's bare argument that pairs with no axis of the
iteration is `Wildcard`, the whole array, as `SUM(src[*])` is. A `Bare`
verdict is therefore a promise the lists keep, and the element edges of a
`Bare` or `PerElement` site are exactly the executed reads. Two shapes
keep a superset, each with a loud decline of its score: a plain bare read
of a shared-name pair under no mapping (`bare[region] = s` over
`s[other]`: the compiler resolves it by name through its implicit
subscripts, the lists pair nothing, the graph broadcasts), and a plain
bare read related to its iteration only through a parent mapping
(`z[suba] = src` over `src[dimb]`, `dimb -> dima`: pass 0 pairs nothing and
the implicit subscripts then read through the parent). The consumers
follow the site: a frozen `PerElement` occurrence keeps its subscript
(`OtherDepVerdict`: the bare spelling is a different read, one that need
not lower), a superset source read through a subrange has rows for the
subrange's elements and no other, a reducer's bare arrayed argument reads
what pass 0 spells for it (`compute_read_slice`: `match_axes_partial`
under `DirectMappingsOnly` against the enclosing iteration; whole-array
with none in scope or inside an array-producing builtin), stored spelled
on the node (`AggNode::reducer`) so the agg's equation, the classified body
and the feeder freezes pin it per slot, and that spelled reducer is the
synthetic node's identity (`AggNodesResult::synthetic_by_key`): two owners
whose spelled reducers print alike read the same slices and share a node,
and `SUM(pop)` in a scalar owner (`sum(pop)`, the whole array) beside
`b[region] = SUM(pop) * k` (`sum(pop[region])`) are two -- the target's own
text is not an identity, and a reducer's text is resolved to a node only
among its owner's own nodes (`by_var`). The wildcard argument of a reducer
the hoist declines is held live in its own edge's partial
(`occurrence_realizes_shape`: the edge's `DynamicIndex` site is realized by
the walker's `Wildcard` occurrence inside the reducer), the co-sources
frozen around it. The GH #779/#788/#789 declines rest on a premise the VM
refutes (`growth[r] = SUM(matrix[r,*] * frac[r])`, no `|D1|` factor) and are
gone, with a bare spelling emitting its iterated twin's variables and series
bit for bit. The one executed read the describers leave undescribed is the
ordinal fallback of an undeclared pair with disjoint element names at equal
cardinality (GH #527, a read Vensim rejects): it is declined loudly, never
described positionally, and the attribution surfaces hold no positional
derivation of their own. A bare argument in a per-element (`Ast::Arrayed`)
slot reads the row the slot's element pins (`slot[a] = SUM(matrix) * 0.001`
is `matrix[a, *]`'s sum, measured); the only node its identity mints is the
whole array, so the per-element-owner rule (GH #792,
`unhoisted_reducer_source_read`'s `Ast::Arrayed` arm) sees the bare
argument whether or not a node was hoisted for it and the slot's edge is
declined loudly, never scored against that node -- the exact per-slot
description (a node per slot with the element pinned) is a tracked
follow-up. C-LEARN's plain and LTM artifacts (the
`bytecode_profile` blocks, the 6193-variable LTM set) and the plain corpus
sweep are identical against the base; the corpus `--ltm` sweep moves one
natural model's diagnostics (FREE6, V9b-3) and no values.

**Phase LTM semantic divergences (V9b).** Seven, each pinned against the
compiler's executed read. (V9b-1) The iterated spelling under a declared
element map that PERMUTES the ordinal diagonal is described as the map's
diagonal: `target[State] = x[State] * w` with `s1 -> b, s2 -> a` reads
`x[b]` in slot `s1` (measured), so the element edges are the map's alone,
the arrayed `x -> target` score reads `x[b]` there (its recorded series
equals the ratio the recorded series give, and differs from the ordinal
alternative), and the loops through it score, where the base withheld the
score and dropped the loops as a "disagreeing" pair. A sliced reducer
over the same map hoists with the map's slots (`matrix[east,*]` feeds
`agg[ca]` under `CA -> east`). Pinned by
`ltm_array_agg::a_permuted_mapped_pair_scores_the_maps_diagonal` and
`element_mapped_sliced_reducer_hoists_and_scores_its_loops`;
`element_graph_tests::bare_mapped_dims_project_the_executed_correspondence`
and `ltm_augment_pin_tests::dep_element_pins_projection_enumeration` pin the
element graph's and the pins' halves. (V9b-2) An undeclared pair sharing
element names is read by NAME in every spelling, and described so:
`plain[Region] = stock[Region]` over `stock[Other]` (`Other` the same names
in the opposite order) is `PerElement` with `stock[north] -> plain[north]`
and no other edge, scored per element, where the base classified the site
`DynamicIndex`, declined `stock -> plain` loudly, and let the ceteris-paribus
partial of a sibling edge read a wrong element (`stock -> grow` under
`grow[Other] = stock[Other] * 0.1 * plain[Other]`: the base's 0.667 is
neither the by-name ratio 0.333 the series give nor the ordinal one; the
tree's is the by-name ratio). Pinned by
`ltm_element_instance_tests::qualified_index_edge_follows_the_plain_equations_name_first_read`
(the plain and the helper spelling, against the VM) and
`ltm_ir_tests::ir_undeclared_shared_names_iterated_subscript_is_per_element`.
(V9b-3) A subrange-named read of a superset-dimensioned variable inside an
apply-to-all over the subrange -- FREE6's `Energy Carbon
Emissions[nonrenewable] = Energy Production[nonrenewable] * Carbon
Content[nonrenewable]` over `Energy Production[source]` -- is `PerElement`
by name: the edges are `energy[coal] -> prod[coal]` and
`energy[oilgas] -> prod[oilgas]` (the base's cross-product minted
`energy[hn] -> prod[coal]` and phantom loops through it), the per-element
scores exist, a frozen `energy[nonrenewable]` keeps its subscript in the
sibling partials (a bare `energy` does not lower under a `nonrenewable`
iteration; collapsing it turned `carbon_content -> energy_carbon_emissions`
into a `failed to compile; constant 0` score), and the other dependency
`ref[nonrenewable]` over `ref[source]` is pinned by name. The natural
mover: FREE6's `--ltm` diagnostics (five pair-level declines gone, the
per-element scores emitted; the model's LTM run falls back on the base and
the tree alike for unrelated fragments). Pinned by
`ltm_array_agg::a_subrange_named_read_of_a_superset_variable_is_read_by_element_name`,
`ltm_ir_tests::other_dep_verdict_rule_covers_every_branch` (the `PerElement`
row) and the subrange row of `dep_element_pins_projection_enumeration`.
(V9b-4) A bare arrayed reducer argument in an apply-to-all body is hoisted
and scored exactly as its iterated spelling: `growth[D1] = SUM(matrix[D1,*]
* frac)` emits `frac[D1]`'s variables and series bit for bit (the feeder's
changed-last share and the rows' changed-first shares of one slot sum to
one), for `SUM`, `MEAN`, `MIN`, `MAX` and `STDDEV`; `growth[D1] = SUM(other)
* frac` hoists an aggregate over `D1` whose slot `e` is `other[e]`, with the
`frac -> growth` and `agg[e] -> growth[e]` partials equal to the ratios the
recorded series give; `growth[D1] = local + SUM(pop)` scores both terms with
shares summing to one. The base declined every one of these edges loudly
(GH #779/#788) on the `|D1|`-factor premise of GH #789, which the VM
refutes (`growth[r] = Σ_d2 matrix[r,d2] * frac[r]`). Pinned by
`ltm_array_agg::a_bare_reducer_feeder_is_hoisted_like_its_iterated_spelling`,
`bare_reducer_feeders_hoist_across_the_reducer_class`,
`a_bare_reducer_argument_in_a_product_scores_per_element` and
`an_additive_bare_reducer_argument_scores_its_own_element`; no corpus model
has the shape. Two owners of one text under different iterations are two
nodes (`two_owners_of_one_reducer_text_under_different_iterations_get_their_own_nodes`,
both declaration orders, under debug assertions;
`a_row_sum_and_a_column_sum_of_one_text_get_their_own_nodes`,
`a_per_element_owner_beside_an_a2a_owner_of_one_text_keeps_the_a2a_node`,
`a_row_reducer_owner_and_a_whole_reducer_owner_of_one_text_get_their_own_nodes`),
and a bare argument related to its iteration only through a parent mapping
reads the whole array (`a_parent_mapped_bare_argument_reads_the_whole_array`:
the six element edges, the loud decline; `a_directly_mapped_bare_argument_reads_its_slot`
the control). (V9b-5) A live reducer argument beside a wildcard co-source,
the reducer un-hoisted -- `x[region] = other + SUM(pop * w[*]) / 1000` with
`w` varying -- scores `w -> x` as the partial with `w[*]` live and `pop`
frozen (`sum(PREVIOUS(pop) * w[*])`), 0.0155 at the first step against the
recorded series, where the base hoisted the reducer whole-array (305 against
a read of 203, `agg -> x[north]` 5.48, V9a-1's wrong number) and a partial
that froze the reducer whole with its live source inside scored 0 at every
step with no diagnostic. Pinned by
`ltm_array_agg::a_live_wildcard_argument_of_an_unhoisted_reducer_stays_live`
(`w -> x` and `pop -> x` against the series ratios); no corpus model has the
shape. (V9b-6) A bare reducer argument in a per-element (`Ast::Arrayed`)
slot -- `slot[a] = SUM(m) * 0.001` beside `z = SUM(m) * 0.0001` -- reads the
row its element pins (`m[a,*]`'s sum, measured) while the node its identity
mints is the whole array `z` reads; the slot's edge `m -> slot` is declined
loudly (one warning, no `agg -> slot[e]` score, the loops through `slot`
dropped), where the base scored `agg -> slot[a]` at 2050.5 against the
whole array with no diagnostic. `z`'s `agg -> z` and the A2A `row`'s own row
node are untouched. Pinned by
`ltm_array_agg::a_per_element_slots_bare_reducer_argument_is_declined_not_scored`
and the per-element arm of
`a_per_element_owner_beside_an_a2a_owner_of_one_text_keeps_the_a2a_node`;
no corpus model has the shape. (V9b-7) A flow feeding its stock through a
PARENT mapping -- `inflow[dimb]` into `level[suba]`, `dimb -> dima`, `suba`
inside `dima` -- is integrated element by element through the parent
(`level[a1]` from `inflow[b1]`, `level[a3]` from `inflow[b3]`, measured);
the score admission pairs the structural edge under the wiring's relations,
so `inflow -> level` is one arrayed score over `suba` whose slot equals the
flow-to-stock ratio the recorded series give, the element edges being the
two the wiring makes; the base declined the score loudly ("dimensions do
not correspond"). Pinned by
`ltm_array_agg::a_flow_feeding_its_stock_through_a_parent_mapping_scores_per_slot`
(the parent-mapped stock and the directly-mapped control); no corpus model
has the shape.
<!-- END_PHASE_8 -->

<!-- START_PHASE_9 -->
### Phase 9: One diagnostic payload

**Goal:** one typed payload from a raising site to `collect_all_diagnostics`,
context attached once, and exactly-once emission by construction (DoD 8).

**Phase 9: one diagnostic payload from raising site to collection.**
`diagnostic::Diagnostic { model, variable, owner, severity, error:
DiagnosticError }` is the payload and the salsa accumulator; `DiagnosticError
::{Equation(EquationError), Model(Error), Unit(UnitError), Assembly(String)}`
is the typed error exactly as its raising site produced it, and `code()`,
`category()` (`DiagnosticCategory`: the four arms with `Unit` split by
`UnitError`'s three), `location()`, `reason()` and `is()` are the projections
every consumer reads, so a consumer never restates the arm-by-arm plumbing and
the sum carries every field its site had. Context attachment is a type: a
`Variable` carries the context-free `DiagnosticError`s its parse and lowering
raised (`Variable::diagnostics`, one channel where a `Unit` entry is a
malformed `<units>` string compiled past and every other entry stops the
variable, `Variable::fatal_diagnostics`), and a `Diagnostic` is built at the
salsa layer's raising sites, where the model, the variable and the severity
are known -- the fragment constructors (`explicit_fragment_input`, whose
`ExplicitFragment { diagnostics, input: Option<..> }` carries every row the
constructor raised beside the input it built or did not, and
`implicit_fragment_input`), the per-model advisories, the unit pass and the
LTM `ltm_warning` -- so nothing between a site and the drain re-attaches
context and nothing asserts it.

**Phase 9: presentation.** `variable` is the physical name a row is filed
under (a helper's `$⁚…` name, the identity the layout and the drain's
de-duplication use) and `owner` the variable a helper was synthesized for --
a user variable, or for an LTM helper the synthetic link score -- which is
the name a consumer presents the row under: `errors::FormattedError`, the one
presentation adapter, presents `owner.or(variable)` and carries the category
in place of a second unit-kind spelling (libsimlin's `SimlinUnitErrorKind`
derives from it). The unit-inference umbrella is a `Unit(InferenceError)`
filed under no variable, formatted by the same arm as every inference error,
each involved variable named once. The drain collapses identical rows by full
equality after attribution -- one fragment's two phases, one parent's
per-element helpers -- and any row differing in a field survives.

**Phase 9: facts, not accumulation.** A recursive or multiply-keyed query
never accumulates: `model_ltm_variables` records its warnings and its
declined edges in one `LtmWarnings` sink and returns them as
`LtmVariablesResult::diagnostics`, `model_ltm_fragment_diagnostics` returns
its `Vec<Diagnostic>`, `shaped_link_score` carries a declined
edge's warning as `ShapedLinkScore::Unscoreable`'s payload, and the
dependency graph records its cycle as `ModelDepGraphResult::cycle_variables`.
The non-recursive `model_all_diagnostics` emits each model's facts once, in
the accumulator walk's order -- a memo's own rows, then each input's in the
order it was first read: the rows the owner files itself (the duplicates and
the advisories), then the variables' rows, the cycle rows (a cycle member's
own failure comes first, the row libsimlin's `SimlinError.code` reports, and
a cycle whose every member is fatal is reported like any other), the
helpers', the unit pass, the wiring check and the LTM facts, each a tracked
child read in that order -- and `collect_all_diagnostics` emits the project
facts (unit declarations, the macro set, module cycles) from their memos.
When several arms of an arrayed equation fail to lower, the arm reported is the first in the dimensions'
declared element order, the first the compiler would have compiled
(`ast::lower_arrayed_arms`, a walk only the failure path pays for; the
default arm's error takes precedence over any element arm's), and codegen's
array-operand refusal names the expression kind it cannot read
(`walk_expr_as_view`).

**Phase 9 semantic divergences.** Eight, each pinned. (1) The unit-inference
umbrella presents as `units inference warning in model '{m}' involving
{vars}: unit_mismatch -- {reason}` rather than `warning in model '{m}':
ModelError{unit_mismatch: {reason}}`: the same row (model, no variable,
`UnitMismatch`, Warning, `FormattedErrorKind::Units`, inference kind, the
same bare reason and zero offsets) under the inference arm's summary line.
The 33 corpus models carrying an umbrella move exactly that line, plain and
`--ltm` alike
(`errors::tests::inference_umbrella_presents_as_a_unit_inference_warning`,
`unit_checking_test::inference_umbrella_detail_is_user_facing`). (2) An
arrayed equation with several failing arms reports the first in declared
order on every database, the default arm's error over any element arm's, and
the smallest key among failing arms naming no declared element; no corpus
model has two failing arms
(`db::diagnostic_payload_tests::the_first_failing_element_arm_in_declared_order_is_reported`).
(3) The codegen refusal reads `an array operand here must be a variable, a
subscripted array or an array temp, but it is an arithmetic or comparison
expression` rather than the base's `Cannot push view for expression type
Discriminant(N)`; no corpus row carried the old text
(`db::diagnostic_payload_tests::a_codegen_refusal_names_the_expression_kind`).
(4) A generated helper's row presents under its owner:
`FormattedError.variable_name` -- libsimlin's `SimlinErrorDetail.variable_name`,
pysimlin's `ErrorDetail.variable_name` and the TypeScript `variableName` the
diagram attaches errors by -- is the parent for a user helper's codegen
refusal (`$⁚aggx⁚0⁚arg0` -> `aggx`) and the synthetic link score for an LTM
implicit helper's fragment failure, where the base presented the `$⁚…`
name; 16 `--ltm` corpus rows in four models (`conveyor_containers.xmile`,
`covid19_severity.stmx`, `arrays_cname.xmile`, `arrays_varname.xmile`) move
that field and no CLI text
(`errors::tests::a_helpers_row_presents_under_its_owner`,
`ltm_unified_tests::test_model_ltm_fragment_diagnostics_covers_implicit_helpers`).
(5) A sub-model's LTM warnings are reported once per project, not once per
model reaching it (GH #866): under two parents, or two instances in one
parent, the base reported a circuit-budget flip, a bogus pin, a declined edge
or an ungenerable partial equation three or two times
(`db::diagnostic_payload_tests::every_warning_family_is_emitted_once_across_revisions`,
every family under one parent, two parents and two instances). No corpus
model has the shape. (6) A cycle's row follows the rows of the variables in
it, and a cycle whose every member is itself fatal is reported: the base
accumulated the cycle inside the dependency graph, where it reached the
drain only through the first non-fatal member's fragment. The mixed shape
`a = b + bogus`, `b = a` reports `a: UnknownDependency` then
`b: CircularDependency` on both, and libsimlin's `SimlinError.code` is
`UnknownDependency` on both; the all-fatal shape `a = b + bogus`,
`b = a + bogus` reports the cycle beside the two members' own failures where
the base reported the members only. One corpus model carries a cycle beside
variable rows, `test/metasd/beer-game/RealBeer4-Sterman13.mdl`: its cycle row moves
from first to after them, plain and `--ltm` alike, order-only
(`db::diagnostic_payload_tests::a_cycles_row_follows_its_members_rows`,
libsimlin `test_error_code_of_a_failing_cycle_member_is_its_own_failure`).
(7) A model's rows come out in the accumulator walk's order above, the LTM
facts one child read last in derivation order where the base's
`model_ltm_fragment_diagnostics` led its own fragment-failure rows over the
derivation's; order is no contract (GH #1036), and 8 `--ltm` corpus
models' stderr lines permute, each sorted-line-identical. (8) A per-variable inference row names each involved
variable once (`errors::involved_names`) where the base joined every source,
so a variable at two locations read "involving x, x"; message-only, and no
corpus row has the shape
(`errors::tests::an_inference_row_names_each_involved_variable_once`).
<!-- END_PHASE_9 -->

## Additional Considerations

**Team process.** One checkout holds the branch; at most three agents run at
once, every teammate on the same model as the lead. A chunk is: an implementer
works with the tree uncommitted and runs the relevant engine test subsets,
`cargo clippy`, and `cargo fmt` before declaring done; the lead verifies the
tree (`git status`, `git diff --stat`, a targeted test run); ONE fresh reviewer
reads the diff adversarially and reports material findings -- defects,
semantic changes not pinned by a test, violated invariants, duplication the
chunk was meant to remove, false claims in docs or comments -- separately from
nits; the same implementer fixes every material finding and folds in nits that
are trivial; the implementer commits through the pre-commit hook (never
`--no-verify`) and the lead confirms the commit in `git log`. A second review
round happens only when the fixes themselves are substantive. The next chunk
starts in an isolated worktree (its own cargo target directory, no commits)
while the previous one is in review; the lead applies its patch to the main
checkout once that commit lands. Measurement per phase is the cheap channel --
retired instructions plus the `bytecode_profile()` artifact block -- recorded
in the ledger; a delta under about one percent is recorded, not investigated,
and a later perf pass owns it. A phase that claims identical semantics also
builds the pre-change commit from `git archive` into the scratchpad and runs
both CLIs on hand-written probe models covering the shapes the corpus lacks,
diffing the simulation output.

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

**One compiler, in tests too.** There is no whole-model test oracle beside
`compile_project_incremental`. A test that needs a variable's lowered form
reads it through the production per-variable lowering
(`test_common::TestProject::flow_exprs`, over
`db::var_fragment::explicit_fragment_input` +
`compiler::fragment::lower_fragment`); a test that needs a refusal's
location reads `TestProject::error_diagnostics`; loop-discovery tests drive
`ltm_finding::discover_loops_with_graph` with the graph and context
`analysis::analyze_model` builds. A second whole-model path would be a second
implementation of layout and metadata that cannot lower resolved SCCs and
makes every compiler change twice; differential checking belongs at the
artifact level (the genuine-output corpus, the fragment goldens, the
wasm-vs-VM parity run). The `ModelStage0`/`ModelStage1` memos are read by unit
checking only, and Phase 8 retires them with it.

**Phase 7 investigation output.** The enumeration the phase required is in
`docs/design-plans/2026-08-26-compiler-unification-phase7-investigation.md`
(every claim with `file:line` at commit `cf534130`). What it settled, and the
decisions taken on it:

- Of the sixteen decisions `builtins_visitor.rs` makes, exactly three read
  model-level state: D1 "is this identifier module-backed" (`module_idents`),
  D2 its pre-classifier `collect_module_idents`/`equation_is_module_call`
  (which re-parses every Aux/Flow equation of the model), and D3 "is this bare
  subscript index an element not shadowed by a variable" (`model_var_names`,
  LTM path only). Everything else reads project-global inputs (dimensions, the
  macro registry, `macro_body_owner`) or nothing, so a parse keyed on
  `(variable, project)` needs no model key; D8 (is this call inside the macro
  that owns it) is answered by the project-keyed `macro_body_owner`.
- D1 moves to lowering: a `PREVIOUS`/`INIT` argument that is a bare name is
  resolved by `compiler::context::Context::snapshot_storage` from the
  dependency's shape ("Phase 7.4 context-free parse"), and `SnapshotArg::access`
  is the one statement of which SPELLINGS address storage, read by the parse
  over the source argument and by codegen over the lowered one so the graph
  and the bytecode cannot drift (the GH #568 class). D3 stays in the parse,
  decided by the LTM parse's variable-name set alone (chunk 7.5a generalizes
  it).
- The parse reads no module-ident set: it is one memo per `(variable,
  project)`, and every consumer -- compile, diagnostics, analysis, LTM,
  layout, libsimlin, the CLI -- reads that memo. The causal graph reads the
  input-agnostic dependency set (every `isModuleInput` branch) where an
  instantiated sub-model's compile graph reads the branch-selected one; the
  parse they share has no input-set key.
- Every helper the parse synthesizes carries parsed data, never equation text:
  a `PREVIOUS`/`INIT` capture and a hoisted module-call argument are `Expr0`
  subtrees with positional identity `(parent, id)`, and a stdlib or macro module
  instance is its target model plus its input wiring. The re-parse inventory is
  therefore EMPTY -- no production site and no test oracle lexes a helper back
  from text -- so the printer and the lexer do not have to agree on every
  spelling for a model to compile (GH #913's class). A helper's name is still
  its identity at the twenty-one name-keyed sites, and it is derived in ONE
  place (`capture::synthetic_ident`); see "Phase 7.2 captures" and "Phase 7.3a
  implicit modules" below.
- AC3.1 needs the parse key and nothing else. The two other causes on the
  record are gone or were never real: the whole-model module map the fragment
  compilers cloned was deleted with Phase 3 (module shapes come from
  `model_shape` per sub-model), and `project.models` changing on a stdlib
  splice does not happen at all, since `db/sync.rs` splices every stdlib model
  on every sync. Chunk 7.1's execution-count probe measured both, and the
  remedy is stated under "Phase 7.1 probe" below.
- The runlist contract a capture reproduces: a PREVIOUS capture is a flows-only
  unit with no edge from its parent in either phase; an INIT capture is seeded
  into initials through `all_init_referenced`, keeps the parent->capture
  initial edge (load-bearing: `LoadInitial` reads `curr` during initials),
  loses the dt edge, and is also recomputed in flows. Byte-identical runlists
  and layout additionally need the capture's runlist ident to sort exactly as
  today's `$⁚{parent}⁚{n}⁚arg0[⁚{suffix}]` does, with `n` assigned in the same
  argument-first walk order and the same shared-`n`-per-module rule.
- LTM's PREVIOUS helpers become captures too, but keep their append-by-presence
  placement (sound because `LoadPrev` reads the snapshot); they do not enter
  `model_dependency_graph` in this plan.

Phase 7 is therefore executed as: 7.1 the execution-count probe and the single
D1/D3 predicate (a plain function, not a projection: the parse still decides
and the dep stage still reads the parsed helper list, so no projection removes
a whole-model read); 7.2 captures for PREVIOUS/INIT with today's
capture set, names, and walk order held fixed (the goldens are the defect
detector); 7.3a `ImplicitModule` with per-element expansion and shared `n`, which
covers macro CALLS for free because `expand_module_function` is one expansion
for both; 7.3b macro passthrough and GH #554, the paths that deliberately do
not reach it; 7.4 the context-free parse -- `ModuleIdentContext` and every
empty-context twin call site deleted, D1 resolved at lowering (the captures
it synthesized for `PREVIOUS(module-call aux)` and `PREVIOUS(m·scalar_port)`
gone, a bare `PREVIOUS(sub)` refused loudly, a bound port's snapshot read
from its own slot), the AC3.1 flip, and the sub-model initials rule that
closes GH #1028; 7.5 the shape changes, one commit and ledger row each:
generalising the D3 bare-element rule from the LTM path to user equations,
together with the QUALIFIED-element form `PREVIOUS(vals[Dim.elem])`, which
captures unless the qualified dimension is among the variable's own declared
dimensions or their map chains, because the parse narrows its dimensions
context to those (measured in chunk 7.1, "Phase 7.1 predicate"); taking INIT
captures out of the flows runlist; keeping `ApplyToAll` instead of rewriting
to `Arrayed` for an apply-to-all body that merely contains `PREVIOUS`/`INIT`
(D14); and hoisting `DELAYN`'s duplicated input argument once.

**Phase 7.2 captures.** A `capture::Capture` is a `PREVIOUS`/`INIT` argument
hoisted into its own unit of evaluation: `(id, kind, arg, suffix, dims)`, where
`arg` is the argument's `Expr0` subtree exactly as the parse's walk left it,
`id` is the walk counter the visitor was at, `suffix` is the active
apply-to-all element when the parent is expanded per element, and `dims` is
non-empty only for the GH #541 arrayed capture. `capture::ImplicitVar` is the
ordered list a parse produces: a `Capture`, a `HoistedArg` or an
`ImplicitModule` ("Phase 7.3a implicit modules" below).

`capture::synthetic_ident(parent, n, part, suffix)` is the single statement of
how EVERY synthesized helper is named -- captures, module instances, and their
hoisted arguments alike -- so `rg "arg0" src/simlin-engine/src` finds exactly
one production derivation. The name is an external key, not the identity:
every runlist is a lexicographic sort, and the layout's implicit section and
the results offset map are name-sorted, so a helper filed under a different
string sorts elsewhere and moves the artifact. Internal code addresses a
capture by `(parent, id)`.

`ImplicitVar::parsed_variable` is the one constructor of a helper's parse-stage
`Variable` at every consumer (`db::implicit_deps`,
`db::fragment_compile::lower_implicit_var`, the lowering memos and both
`db::ltm::compile` sites). A helper carries no `Variable::eqn`: that field is
user-authored source text by definition, and a helper has none -- its body is
the subtree. The one reader that needs a target's axes as names,
`ltm_augment::target_equation_dims`, takes them off a source variable's `eqn`
(datamodel casing) and off a helper's lowered `ast()` (canonical), and the
generator reads a target's body from its lowered `ast()` only: a target with
none -- a scope-dependent lowering refusal such as `MismatchedDimensions`,
which leaves the causal edge into it standing because dependencies are
classified on the typed tier -- is declined loudly
(`PartialEquationErrorKind::MissingTypedTarget`) rather than scored around a
body the compiler refused. The compile path has no round trip; the engine has
one, off it: LTM's link-score generation prints the target's LOWERED body
(`patch::expr2_to_expr0` + `print_eqn`) around the guard form and parses that
text once in `db::ltm::equation::LtmArm::new`, the GH #965 generated-text
boundary, which applies to every variable and to captures alike. The capture
arm runs the body through `instantiate_implicit_modules`, whose per-element
gate fires on a bare `PREVIOUS`/`INIT` as well as on a module call, so an
arrayed capture holding one becomes an `Ast::Arrayed` of identical elements
rather than staying an `Ast::ApplyToAll` (D14 -- keeping the `ApplyToAll` is a
shape change with its own ledger row).

One representation difference survives, and it is not observable. A capture
keeps the SOURCE spelling of an identifier where a re-parse kept the lexer's:
`PREVIOUS(vals[d.e2], 0)` captures `RawIdent("d.e2")` where re-parsing
`print_eqn`'s output produced `RawIdent("d·e2")`. `Expr0` -> `Expr1` lowering
canonicalizes every identifier and `common::canonicalize` maps an unquoted `.`
to `·`, so the two are one identifier from that point on;
`db::capture_tests::a_captures_fragment_is_its_argument_compiled` is the
measurement rather than the argument, requiring that row's capture and an
ordinary aux holding the same expression compile to identical bytecode.

Two identical helpers are one helper: `Capture::same_definition` and
`Expr0::eq_ignoring_loc` answer the dedup question without consulting source
positions, which is what lets the apply-to-all expansion collapse the N copies
of one cloned body, and what stops a whitespace-only difference between an
element's equation and its initial equation from becoming two helpers claiming
one name. `PartialEq` keeps positions, because salsa uses it to decide whether
a re-parse changed anything and a moved span changes the diagnostics.

**Phase 7.3a implicit modules.** A stdlib or macro module-function call
expands into values on the same ordered `ImplicitVar` list a capture rides:
one `capture::ImplicitModule` -- the instance, its target model plus the
`references` wiring each input port to the variable feeding it -- and one
`capture::HoistedArg` per argument that is not a bare identifier, carrying the
argument's `Expr0` subtree. A bare identifier argument wires straight to its
port, so the wiring and the hoisted arguments do not correspond one-to-one and
the `arg{i}` in a name is the argument's position in the CALL. Both
constructors derive their name from `(parent, id, call name, suffix)` through
`capture::synthetic_ident`, and an instance shares one walk counter with its
arguments. `ImplicitVar::parsed_variable(dims)` is the one exhaustive
conversion to a parse-stage variable, so no consumer (`db::implicit_deps`,
`db::fragment_compile`, `db::stages`, `db::analysis`, both `db::ltm::compile`
sites, the `ModelStage0::new_in_project` oracle) lexes a helper back from text.

`capture::insert_implicit_var` is the one rule for two helpers claiming one
name -- a same-definition repeat is idempotent, a different helper is refused
as `DuplicateVariable` before it can overwrite the first -- applied inside one
walk, across the per-element walks of an arrayed parent, and across the dt and
initial passes of one variable. A macro named `ARG1` invoked as
`ARG1(k, k * 2)` reaches the refusal from ordinary source: its instance and its
second argument's helper both derive `$⁚out⁚0⁚arg1`.

A hoisted argument is not rewritten before the hoist: the helper carries the
element of the apply-to-all body it is one element of (`variable::ElementScope`
on `VarKind::Aux`, `capture::HoistedArg::scope`), and the compiler lowers it
under that element (`Context::element_scope_context`), the same `Context` the
parent's own element is lowered under, so there is ONE resolution of a
cross-dimension spelling and the helper reads what the plain equation reads
(GH #1035; "Phase 7.5c structural captures and element-scoped helpers"). The
hoisted-argument column of `mapped_reference_semantics_tests` holds every row
of that module's matrix, its no-mapping controls and its two-axes section to
the plain equation's verdict, values and refusal codes alike, for the hoisted
twin and for the snapshot-captured twin. A parse-time replay of the compiler's
rule is what must not be added: two resolvers of one spelling drift exactly
where the rule is non-trivial.

**Phase 7.3b macro fall-throughs.** `MacroRegistry::resolve_call(call,
enclosing_model)` is the one routing decision for a parsed call --
`Expand(descriptor)`, `Passthrough(descriptor)`, `RenamedBuiltinSelfCall`,
`Unresolved` -- read by `BuiltinVisitor::walk` and by the registry's own
recursion analysis, so the expansion and the macro-call graph cannot disagree
about which calls expand. The precedence it states -- a project macro shadows
a like-named builtin -- is the engine's rule: unverified against Vensim's
macro documentation, which says nothing about a macro named after a builtin,
and the opposite of XMILE 1.0 3.2.2.5, which reserves builtin names --
they "cannot be used as vendor- or user-defined namespaces, macros, or
functions" -- and says a conflict SHOULD be flagged as an error.
Changing the shadowing semantics is out of this phase's scope; the rule is
kept and the routing it needs is stated once. A genuine passthrough (`:MACRO: INIT(x) =
INITIAL(x)`, stored as `init = init(x)` after the importer's rename) lowers as
the builtin it names at an external call site, under the macro's declared
arity; inside its own body that same call is the enclosing macro's renamed
builtin and lowers under the builtin's arity. The false `init -> init`
recursion edge and the infinite re-resolution of GH #554 are both absent
because they were one decision. Pinned by
`macro_expansion_tests::issue_554_model_imports_registers_and_routes_both_init_calls_to_the_builtin`
(the issue's model through production MDL import) and
`module_functions::tests::resolve_call_covers_every_arm_and_the_self_call_takes_precedence`.

**Phase 7.3 semantic divergences.** Three refusals are new; the first two are
of models the base compiled to a value no rule stated, the third of a model
the base aborted on. All are pinned:

1. Two helpers of one call claiming one name. A macro named `ARG1` invoked as
   `ARG1(k, k * 2)` mints its instance and its second argument's helper as
   `$⁚out⁚0⁚arg1`; a last-wins helper map let the instance replace the helper
   and wired the instance's second port to itself, so `out` simulated to 3
   where `k + k * 2` is 9. Refused as `DuplicateVariable` naming the helper.
   Pinned by
   `macro_expansion_tests::a_macro_named_arg1_cannot_alias_its_own_hoisted_argument`.
2. A passthrough macro's external call with the builtin's wider arity.
   `:MACRO: PREVIOUS(x) = PREVIOUS(x)` called as `PREVIOUS(input, 0)` compiled
   as the two-argument builtin behind a macro that declares one parameter;
   refused as `BadBuiltinArgs` naming the macro's contract. Pinned by
   `macro_expansion_tests::a_passthrough_macro_keeps_its_declared_arity_at_an_external_call_site`.
3. A stdlib call with more arguments than its model has ports (`SMTH1(k, 2,
   5, 7)`) indexed the port list out of bounds, a process abort; refused as
   `BadBuiltinArgs` over the call before any argument is hoisted. Pinned by
   `db::implicit_module_tests::a_call_with_more_arguments_than_ports_is_refused_before_any_hoist`.

A hoisted argument reads what the plain equation reads and is refused with
its code ("Phase 7.5 semantic divergences", items 7-9).

One diagnostic-only change: a helper whose body fails to LOWER is reported by
`compile_implicit_var_fragment` as an equation error on the PARENT variable,
at the span of the argument inside the parent's equation (a helper's subtree
was written there, so its spans index the parent's text), where before
nothing reached `collect_all_diagnostics` and the compile result's batch
message was the only trace. `collect_model_diagnostics` collapses identical
rows, so one parent's per-element helpers report one row -- and eight corpus
models that printed one diagnostic several times over (`unrecognized_token`
ten times on `test_subscript_mixed_assembly`, `empty_equation` twice on the
`hares_and_lynxes` module port) print it once. Pinned by
`db::implicit_diag_tests::implicit_helper_lowering_failure_is_an_equation_error_on_the_parent`.

**Phase 7.4 context-free parse.** `db::parse_source_variable(db, var,
project)` is the one source parse query. It reads the variable's own fields,
the dimensions its equation names (narrowed, so a dimension edit re-parses
only the variables that read it), and the project-global contexts: the units
context, the macro registry, and `macro_body_owner` for which macro, if any,
the variable is the body of. It reads nothing of the owning model -- not its
variable set, not which of its names are module instances, module-call auxes
or bound input ports -- so no edit to a sibling variable re-keys or
re-executes it, and one memo per variable serves compilation, diagnostics,
analysis, LTM, layout, libsimlin and the CLI alike. `ParseContext` carries
those project-global contexts plus the `SnapshotIndexFacts` a
`PREVIOUS`/`INIT` subscript index is decided with ("Phase 7.5a static
element snapshots").

What a `PREVIOUS`/`INIT` argument's NAME denotes is therefore lowering's
question, answered by `compiler::context::Context::snapshot_storage` from the
dependency's shape: a bare module instance has no storage of its own and is
refused (`NotSimulatable`, naming the instance), a bound input port reads its
OWN slot -- the port's fragment assigns the parent's value to it every phase,
so the slot's `prev_values`/`initial_values` entry is exactly what a capture
of the port held -- and every other name (a plain variable, a scalar
module-call aux, a qualified `m·port`) is a fixed slot, which
`codegen::static_slot` already addressed. The parse keeps exactly one
module-backed decision of its own, the one it can make without reading the
model: a module instance synthesized EARLIER IN THE SAME WALK
(`PREVIOUS(SMTH1(x, 3))`) is captured. `SnapshotArg::access` remains the one
statement of which spellings address storage, read by the parse over the
source argument and by codegen over the lowered one.

A model that is INSTANTIATED AS A MODULE -- an explicit `Variable::Module`'s
target anywhere in the project (`project_module_graph`), a stdlib template, or
a macro -- evaluates every value-bearing variable in its initials runlist
(`model_dependency_graph_impl`'s `needed` predicate). XMILE's model is one
flat graph in which a module is a namespace, so a parent's initials phase may
read any port of an instance -- `level = INTEG(.., sub·output)` over a
stockless sub-model, a stdlib instance fed from one, `INIT(sub·output)` --
and the value has to exist when the read happens. The rule is one flat bit
per model rather than a per-instance propagation of the ports each parent
reads: the sub-model's own graph cannot see those reads, the initials closure
already orders the members (a stock does not break an init chain; a module
pulls in its input sources), and the cost is one initial evaluation per
sub-model aux. A root that no other model instantiates keeps exactly the
seeds it always had; a root some model does instantiate takes the rule like
any target -- initial evaluations, never a different number (pinned by
`db::fragment_input_tests::a_root_that_another_model_instantiates_simulates_as_it_does_alone`).
This closes GH #1028.

**Phase 7.4 semantic divergences.** Four shapes changed, all pinned: the D1
slice the phase set out to drop, a refusal of a silently wrong number, a
correction of another, and the helper renaming the first implies.

1. `PREVIOUS`/`INIT` of a scalar module-call aux (`PREVIOUS(smoothed, 0)`
   over `smoothed = SMTH1(k, 2)`), of a qualified scalar or array-element
   module output port (`PREVIOUS(sub·output, 0)`, `INIT(sub·arr[2])`), and of
   a bound input port read from inside its sub-model (`PREVIOUS(input, 0)`,
   `INIT(input)`) synthesize no capture and read the slot directly; the values
   are identical to the capture's. A macro formal parameter is the last of
   those, so a macro body's `INIT(x)` captures nothing either. Pinned by
   `db::prev_init_tests::module_snapshot_arguments_are_resolved_at_lowering`,
   the `PREVIOUS(smoothed, 0)` row of
   `every_prev_init_argument_shape_agrees_between_the_parse_and_codegen`,
   `db::fragment_determinism_tests::a_submodel_reading_a_bound_port_through_previous_parses_once_and_compiles`
   (a wired port WITHOUT `access="input"`, which the base refused: its layout
   came from one parse and its runlists from another) and
   `macro_expansion_tests::issue_554_model_imports_registers_and_routes_both_init_calls_to_the_builtin`.
2. `PREVIOUS(sub)`/`INIT(sub)` of a bare module instance is refused as
   `NotSimulatable` on the referencing variable, in direct and LTM lowering
   alike. The base captured the instance into a scalar helper whose fragment
   read flattened slot zero of the instance -- whichever sub-model variable
   the layout put first -- and simulated that. Pinned by the refusal rows of
   `module_snapshot_arguments_are_resolved_at_lowering` and
   `db::ltm_tests::test_ltm_bare_module_snapshot_is_refused_at_lowering`.
3. The `input -> stock` link score of a `DELAY3` instance whose input varies.
   The score's numerator holds the flow's two-step lag,
   `PREVIOUS(PREVIOUS(input))`, whose inner lag is a capture helper inside
   the instance with the bound port `input` as its body. The base lowered
   that port to `Expr::ModuleInput`, `static_slot` refused it, and the LTM
   tail -- which appends helpers by bytecode presence -- dropped the helper
   with no diagnostic, so the score read an unwritten 0 for the lag: `4, 10,
   24` for a unit ramp with delay 2 at dt 1, where the formula gives `1, 2,
   4`. Lowering reads the port's own slot, the helper is `0, 3, 4, 5, 6`, and
   the score is right at the main level and inside a sub-model alike. A
   silently wrong number at the base, corrected; on C-LEARN the one such
   helper (`stdlib⁚delay3` under `{delay_time, input}`) fed a score that is
   byte-identical either way, which is why the artifact-only reading was
   tempting and wrong. Pinned by
   `db::ltm_tests::delay3_input_to_stock_link_score_uses_the_two_step_lag_of_the_bound_port`.
4. Helper names shift when a capture drops out of a walk. The walk counter is
   shared by every helper of one equation, so `INITIAL(input) * parameter +
   SMOOTH(input, 2)` in a macro body files the `SMOOTH` instance as
   `$⁚expression_macro⁚0⁚smth1`, where the capture of the port took `⁚0⁚` and
   pushed it to `⁚1⁚`. A helper's name is an external key (the `Results`
   offset map, the LTM causal graph libsimlin surfaces), so this is an
   artifact-naming change; the values are unchanged. Pinned by
   `macro_expansion_tests::a_macro_body_snapshot_of_its_formal_parameter_reads_the_bound_port`.

One value change that is a fix, not a divergence: a parent reading a
stockless sub-model's port during its initials gets the port's t=0 value
(GH #1028). Pinned by
`db::fragment_input_tests::stock_initialized_from_a_stockless_modules_output_reads_its_t0_value`
(the issue's repro: `level = 30`, `sm = 60`),
`stock_initialized_through_a_nested_stockless_module_reads_its_t0_value`,
`stock_initialized_from_an_active_initial_module_output_reads_the_initial_equation`
(the sub-model's initials evaluate its `ACTIVE INITIAL` equations, so the
parent reads 300 where the port's own series is 30) and
`stocks_initialized_from_module_ports_read_the_flat_model_values` (a sub-model
aux over a sub-model stock, one model instantiated twice with different input
sets, and stdlib instances inside a sub-model -- the shapes a per-instance
propagation would get wrong). The artifact grows by
those initial fragments: every sub-model aux gains one, which is the
`producer::input`/`producer::output` change in
`db/fragment_char_golden/modules.txt`.
**Opcode operand tables.** `compiler::symbolic::SymbolicOpcode` and
`bytecode::Opcode` are each declared by a table macro
(`symbolic_opcode_table!`, `opcode_table!`) holding one row per opcode, and
every per-variant fact about OPERANDS is derived from the rows rather than
restated in a hand-written match. A row is the variant exactly as declared
plus one column: on the symbolic side the concrete twin it resolves to
(`LoadVar { var: SymVarRef } => LoadVar { off: var }`; a twin field that names
a symbolic operand takes that operand's resolution, and a symbolic operand the
twin does not list -- `LookupDirect::table_count` -- is symbolic-only), on the
concrete side the effect on the arithmetic stack (`=> (pops, pushes)`, which
may read the variant's operands: `Apply` pops its builtin's arity, `EvalModule`
its `n_inputs`). An operand's TYPE is its kind, and the kinds are the id
aliases the fields already carried: `SymVarRef` (resolved by `resolve_opcode`,
untouched by renumbering, reported by `var_ref`), `LiteralId`/`ModuleId`/
`ViewId`/`DimListId` (a checked `u16` add of the fragment's base),
`TempId` (a checked `u8` add), `GraphicalFunctionId` with its `TableCount`
(the remap; `gf_run` reports the pair, and a base without a count is a
compile error), `PcOffset` (reported by `jump_offset`), anything else data.
`resolve_opcode`, `renumber_opcode`, `gf_run`, `var_ref` and `jump_offset` on
the symbolic side and `stack_effect`, `name` and `jump_offset` on the concrete
side are generated by one generator macro per table from per-kind helper
macros (`resolve_operand!`, `renumber_operand!`, the shared
`bytecode::operand_of_kind!` muncher); the table macro is a callback
(`symbolic_opcode_table!(generator)`), so the tests derive from the same rows:
`symbolic_table_tests` instantiates every row with per-kind sentinels and
checks resolution (1:1 onto the named twins), renumbering (each id moved by
exactly its kind's base, everything else byte-identical) and the three
accessors against its own per-kind expectations, `bytecode::tests` does the
same for the jump accessors (and checks `name` against the identifier the
derived `Debug` prints), and the merge proptest's `blank_resource_ids` oracle
is the table under a blanking rule. What stays hand-written is semantics: the
`vm.rs` and `wasmgen::lower` execution arms, the peephole and three-address
fusion patterns, `extract_assign_curr_offsets` (which opcodes write `curr`),
and the `db/` scans -- `db::dep_graph::ordering_reads` classifies
current-value READS (`SymLoadPrev` never, `SymLoadInitial` only in the
initial phase, the `Assign*` writes never) and `db::assemble`'s
`temp_read_by`/`temp_written_by` split temp reads from writes (`BeginIter`
gated on `has_write_temp`) -- which are read/write classifications the table
deliberately does not carry, so no kind accessor can replace them. What the
table can give the `db/` scans is an every-row classification test derived
from `symbolic_opcode_table!`: every row with a `TempId` operand is
classified by exactly one of `temp_read_by`/`temp_written_by` (with
`BeginIter`'s flag both ways), and every row with a `SymVarRef` operand is
either a read `ordering_reads` inserts or a write it ignores, stated in the
test -- turning those scans' `_ =>` catch-alls' silence on a new row into a
red test. The two hand-written `temp_uses` oracles (`symbolic_merge_proptest`,
`db/combined_fragment_proptest`) stay hand-written: they are the net
independent of the table's typing, and the `Sum`-strategy one is what catches
a resource id a row declares as bare data. `Opcode` keeps its variants, fields, field order and 8-byte size, the bytecode
is byte-identical on C-LEARN plain and under LTM, the checked adds, the remap
bound and the `temp_off` narrowing keep their error text, and the dead-code
lint still denies a row nothing constructs. No semantic divergence.

**Phase 7.5 results printing.** `Results::print_tsv` (the CLI's `simulate`
output) prints one column per key of the results-offset map, in slot order;
a slot the map has no key for -- a standalone lookup table, a helper slot the
map hides -- is not printed, because the map is the contract every reader of
a series shares and an unnamed slot holds a backend's scratch value.

**Phase 7.5a static element snapshots.** A `PREVIOUS`/`INIT` argument whose
subscripts pin one declared element -- bare `vals[e1]` or qualified
`vals[Dim.e1]` -- reads that slot directly in a user equation, as it does in a
generated LTM equation. The decision stays in the parse: a capture cannot be
un-minted at lowering, and always capturing costs a hidden slot and a flow
evaluation per read. The parse asks the owning model exactly two per-name
facts through `builtins_visitor::SnapshotIndexFacts::Axes`: the referenced
variable's declared axis at that position (`model_variable_by_name` and
`variable_dimensions`, reached through the variable's owning model) and, for
a qualified name, whether the project declares that element
(`project_has_qualified_element`, a per-name projection of
`project_dimensions_context`, so a dimension edit re-parses only the
variables whose spelled element appears or disappears). The owning model is
the `owner_model` name the sync sets on every `SourceVariable`, resolved by
the `variable_owner_model` projection -- a name rather than a `SourceModel`
handle because a model's variable map is a constructor argument of the model,
so the variables exist before it does and a salsa input field can only be set
afterwards through `&mut`, which the fresh sync path does not hold; the same
projection says which macro a body variable belongs to, so no project-wide
map of macro bodies exists. Precedence follows XMILE 1.0 section 3.7.1 and
footnote 9: an element of the referenced axis wins over a same-named
variable (`dimensions::resolve_axis_index_name`, the compiler's own rule for
the same index), and a qualified position may come from an unrelated
dimension and is applied positionally to the referenced axis
(`resolve_axis_index_position`). The generated LTM parse keeps its
whole-surface rule (`SnapshotIndexFacts::ModelNames`: an element of any
dimension that no variable of the model shadows) because a generated equation
may subscript an LTM synthetic variable or a helper, neither of which is a
`SourceVariable` with a declared axis to ask; where the two rules disagree one
mints a capture the other does not, and the difference is observable only
through a consumer that reads the base's extent (divergence 5 below). A
helper's scalar body is not re-walked when its
parse-stage variable is built: every decision of the parent's walk is final,
and a second walk without the model's facts could only re-decide a direct
read into a nested helper.

**Phase 7.5b capture phase demand.** A capture's kind is its phase demand
(`CaptureKind::{Previous, Init, PreviousAndInit}`). `PREVIOUS` storage is
refreshed in flows and never seeded into initials -- nothing reads its
initials value, since `LoadPrev` takes the fallback until the first step
commits. `INIT` storage is populated in initials and enters flows only when a
per-step definition's transitive current-value closure reads it
(`model_dependency_graph`, over `dt_dependencies` with every `INIT`- and
`PREVIOUS`-only edge already stripped, so a read from another INIT-only
capture promotes nothing). Identical positional storage the dt and
active-initial parses mint for different consumers is one capture whose
demand is the union (`Capture::merge_same_definition`). A helper's raw `INIT`
referents are initialization roots of their own
(`ImplicitVarDeps::init_referenced_vars` into `all_init_referenced`), so a
flow-only `PREVIOUS` capture over `INIT(x) + 1` still finds `x` frozen. An LTM
helper's compiled phases are its kind too (`compile_ltm_implicit_var_fragment`,
`ltm_helper_phases_present`), which is its runlist membership since assembly
appends LTM helpers by presence. An INIT-only capture keeps its layout slot --
initials write it and `LoadInitial` reads the frozen copy -- but has no key in
the results-offset map (`flattened_offsets`, the static-table precedent):
nothing writes the slot per step, so the VM's zeroed step chunks and wasm's
retained linear memory would put different scratch values under one exposed
name while every value a model reads agrees. It is hidden by kind, whether
or not a current read promotes it into flows, because promotion is decided per
module instance and the map is per model. For the same reason it is no
causal node: an `INIT` read is a snapshot, not a per-step link, so
`model_causal_edges` takes no edge into or out of an INIT-only capture, and
no link score reads the hidden slot (a score into one read the VM's zeroed
chunk against the initial value, `2 dk / (0 - v0)` where wasm computed 0).
LTM does not promote the capture into flows to keep such an edge: the loop
it would close runs through a frozen value and is no feedback.

**Phase 7.5c structural captures and element-scoped helpers.** An apply-to-all
body is walked once for what it needs (`builtins_visitor::per_element_requirements`,
the maximum over its calls of `None`, `SnapshotOnly`, `ModuleInstance`, routed
through `MacroRegistry::resolve_call` and `module_functions::stdlib_descriptor`
so it cannot disagree with expansion). A snapshot-only body is captured
structurally: ONE `CaptureShape::ApplyToAll` capture over the parent's
dimensions whose body is the source subtree (`$⁚p⁚n⁚arg0`, one slot per
element, keyed per element in the results map like any arrayed variable,
GH #1033), lowered by the compiler per element exactly as the parent's body
is. A module-bearing body is expanded per element, and every helper it hoists
-- a computed argument, a bare arrayed identifier, a bare dimension name, a
snapshot argument (`CaptureShape::Element`) -- is a scalar whose body is one
element of the parent's: `ElementScope` names the element, and
`Context::element_scope_context` lowers the body under it, so a subscript
naming the iterated dimension, a mapped foreign dimension or a repeated target
dimension resolves through the compiler's one rule. An explicit `Ast::Arrayed`
slot keeps its own element context; a snapshot-only default expression is
captured once and its capture read inserted into every missing slot, and a
module-bearing default is materialized per missing slot. A snapshot argument
subscripted by a dimension the parent's axes answer for -- one of them by
name, or one they relate to through a declared mapping or subdimension --
reads its slot directly; `index_spans_a_dimension` puts that question to the
compiler's own matcher under the projection `active_dim_ref` uses
(`match_axes_partial` under `DirectMappingsOnly`), so the parse captures
exactly where lowering cannot resolve a slot. A helper body the compiler
refuses is the parent's equation error, with the compiler's code and the
argument's span (`compile_implicit_var_fragment`), so the hoisted spelling and
the plain spelling refuse identically; a codegen refusal stays an assembly row
on the helper. The LTM describers see a scoped helper with every read its
element pins spelled as the static index the compiler resolves it to
(`FragmentInput::element_pinned_target`, `Context::pin_element_reads`: the
helper's own fragment context running `normalize_subscript_ops` and
`build_view_from_ops`, the steps `lower_subscript` runs), so the element graph
and the scores name the slot the helper's fragment reads for a proper
subdimension, a shared-name axis and an element map alike; a substituted
spelling is what must not stand in for that index, since a qualified
`dimension·element` of a foreign axis folds to an ordinal the compiler does
not read. The IR's description of a PLAIN read is the same resolution
(V9b below: `DimensionsContext::executed_read_correspondence` is the one
correspondence every dimension-named subscript is described by).
`db::ltm::endpoint_dimensions` is the
projection of `model_dep_shape` onto plain variables, read by the causal
edges, the element graph, every score emitter, the loop builders and the pins;
one shape answer is not one emitter -- an element-bound helper is scalar
storage with an edge at one element of its parent, so the scalar-source
emitters admit only explicit scalar sources and
`try_implicit_scalar_to_arrayed_link_scores` scores that edge -- and
`ltm_dep_shape` is `model_dep_shape` plus the two kinds only a generated
equation references. An edge whose every reference site sits in an
`Ast::Arrayed` slot of the target, over a strict subset of the target's
elements, is target-restricted (`EdgeShapesResult::target_restricted_edges`):
`classify_cycle` keeps its cycles on the element-level path and
`build_element_level_loops` emits one loop per circuit, so a loop that exists
at one slot is not reported over the whole dimension.

**Phase 7.5d sparse delay initial ports.** `DELAYN`/`SMTHN` rewrite to the
canonical order-1 and order-3 stdlib models and wire only the ports the call
names: `[input, delay_time]` when no initial value is given, the stdlib
model's `isModuleInput(initial_value)` guard then falling back to the input as
XMILE 1.0 section 3.5.3 states ("If initial value is not provided, the initial
value of input will be used", `docs/reference/xmile-v1.0.html#_Toc439926074`);
an explicit fourth argument is an independent `initial_value` port. `DELAY`
renames to `DELAY1` and nothing else. Two or five arguments, an order other
than the literal 1 or 3, and a non-literal order refuse before any argument is
hoisted.

**Phase 7.5 semantic divergences.** Thirteen changes, each pinned: 7.5a and
7.5b's six (four to the artifact and the results map, one to a value, one to
the LTM causal graph), then 7.5c and 7.5d's seven.

1. A user equation's bare or qualified element snapshot reads the slot
   directly, and the capture the base minted for it is gone (26 on C-LEARN).
   Pinned by `db::prev_init_tests::user_element_snapshots_are_direct_for_both_intrinsics`,
   `snapshot_element_name_matrix_covers_both_intrinsics`,
   `an_active_dimension_that_is_also_an_axis_element_spans_first_for_both_intrinsics`
   and the bare and qualified rows of
   `every_prev_init_argument_shape_agrees_between_the_parse_and_codegen`; the
   generated-LTM boundary by
   `db::ltm_tests::a_bare_element_snapshot_captures_on_the_generated_path_only_when_shadowed`,
   the per-name incrementality by
   `db::fragment_char_tests::module_helper_add_reparses_only_the_added_variable`
   and `db::dimension_invalidation_tests::a_qualified_snapshot_index_depends_on_its_own_element_only`.
2. A capture is a causal node, so removing one changes the LTM score
   topology: a source element's edge into an arrayed target is one score
   arrayed over the target's declared dimensions instead of a scalar
   source->capture and capture->target pair per target element, with equal
   values where the reads are the same value. Pinned by
   `db::prev_init_tests::ltm_snapshot_element_reads_preserve_score_topology_and_values`;
   on C-LEARN 52 scalar scores become 17 arrayed ones (330 slots), pinned
   together with divergence 6's counts by `clearn_ltm_var_count_guardrail`.
3. An INIT-only capture has no flow fragment and no results key, and a
   PREVIOUS capture no initial fragment, LTM helpers included. Pinned by
   `db::prev_init_tests::every_capture_kind_has_the_right_phases_for_every_storage_shape`,
   `an_init_only_capture_dependency_does_not_promote_its_input`,
   `a_current_value_consumer_promotes_an_init_capture_into_flows`,
   `a_previous_capture_flow_seeds_its_local_init_referent`,
   `a_bound_module_init_capture_is_initials_only`,
   `wasmgen::module_tests::compile_simulation_init_capture_without_flow_refresh_matches_vm_and_reruns`,
   `db::ltm_tests::generated_ltm_capture_helpers_are_flow_only` and
   `ltm_capture_helpers_compile_exactly_the_phases_their_kind_demands`, and
   the `prev_init`, `ltm_loop_exhaustive` and `ltm_loop_discovery` goldens.
4. A dt equation and an active-initial equation that mint the same capture
   for different consumers compile as one shared capture; the base refused
   the pair as two helpers claiming one name. Pinned by
   `db::prev_init_tests::a_positional_capture_shared_by_init_and_previous_unions_its_phases`.
5. A consumer that reads its operand's EXTENT sees the referenced variable
   where it saw a one-slot capture: `VECTOR ELM MAP(PREVIOUS(vals[e1], 0),
   offs[d])` with `offs = 1` reaches the prior step's `vals[e2]` (`0, 20, 21,
   22`) where the base's mapping ran off the end of the capture (`:NA:`, a
   NaN) -- exactly what the numeric spelling `vals[1]` already computed.
   Pinned by
   `db::prev_init_tests::an_elm_map_over_a_bare_element_snapshot_ranges_over_the_variable`
   and, relative to the numeric spelling,
   `array_operand_materialization_tests::every_row_of_the_issue_995_table_compiles`.
6. An INIT-only capture is no LTM causal node: `model_causal_edges` takes no
   edge into or out of one, so no link score reads the hidden slot and no
   loop closes through a frozen snapshot. On C-LEARN 921 scalar scores do not
   exist -- one capture->parent score per INIT-only capture (207, identically
   0 since the parent reads the frozen value) and 714 source->capture scores
   from 104 sources, 21 of which the base scored `1` on every step because it
   re-evaluated the capture per step -- nor do the 14 array-freeze helpers of
   the scores into `$⁚last_set_target_year⁚0⁚arg0`; the 10 loop scores are
   unchanged. 7,163 -> 6,193 LTM variables, 30,123 -> 29,398 slots, and the
   all-slot digest 20,892 -> 20,221 LTM slots with 3,141 -> 3,106 ever
   non-zero (the 21 scores and the 14 freeze slots). Pinned by
   `simulate_ltm_wasm::an_init_only_capture_is_no_ltm_node_and_every_key_matches_wasm`
   (no edge through the capture, VM == wasm on every results key under LTM),
   `clearn_ltm_var_count_guardrail`, `clearn_ltm_slot_maxima_digest` and
   `clearn_with_ltm_simulates_model_vars_identically`.
7. A hoisted module-call argument under an apply-to-all body reads what the
   plain equation reads: the eight cells the base read by ORDINAL (a subscript
   naming the iterated dimension under a permuted, many-to-one,
   reverse-cardinality or shared-element-name map, and a repeated target
   dimension) follow `resolve_mapped_read` name-first, as the plain equation
   does, and a stock's smoothed flow under a copied dimension reads the
   element NAMED like the active one (`stock[Region]` over `Other` declaring
   the same names reversed reads 20, not the positional 10). Pinned by
   `mapped_reference_semantics_tests::a_hoisted_argument_reads_what_the_plain_equation_reads`
   (equality in every cell, hoisted and captured twins) and
   `db::ltm_element_instance_tests::qualified_index_edge_follows_the_plain_equations_name_first_read`
   (the VM, and the element graph naming the element the helper reads) and
   `db::element_scope_tests::a_hoisted_read_of_a_proper_subdimension_is_scored_at_its_own_element`
   (a subdimension read inside a loop: the edge, the score and the loop all
   at the helper's element).
8. A bare arrayed identifier or a bare dimension name as a module-call
   argument under an apply-to-all body compiles as an element-scoped helper
   (`$⁚out⁚0⁚arg0⁚e1 = aux vals in d=e1`) where the base wired the name to the
   scalar port and refused (`NotSimulatable`) or wired the qualified element.
   Pinned by `db::implicit_module_tests` (the bare-arrayed, bare-dimension and
   bare-scalar rows) and the hoisted column's `target[State] = x` cells.
9. A helper the compiler refuses is reported on the PARENT as the plain
   equation's error -- `MismatchedDimensions` with the argument underlined --
   where the base reported `DimensionInScalarContext` on the parent or an
   assembly row on the helper. Pinned by
   `db::implicit_diag_tests::implicit_helper_lowering_failure_is_an_equation_error_on_the_parent`,
   `array_tests::unresolvable_helper_fails_loudly_not_silently` and the
   no-mapping refusal cells of the hoisted column.
10. A snapshot-only apply-to-all body is ONE structural capture, keyed per
    element in the results map, where the base minted one scalar capture per
    element with an element suffix: on C-LEARN 24 `INIT` parents' 150
    per-element captures are 24 arrayed ones over the same 150 slots (-126
    initial programs, -165 literals, every plain results key and value
    identical), and under `CLEARN_LTM=1` the 630 per-element helper keys are
    630 per-element keys of structural captures, every other count and every
    value identical. A snapshot argument subscripted by a dimension the
    parent's axes relate to through a declared mapping (`PREVIOUS(x[Other],
    0)` under `Region`, `Other maps_to Region`) reads its slot directly, as
    the base's per-element substitution did. Pinned by `db::capture_tests`
    (the apply-to-all rows), `an_apply_to_all_captures_slots_are_keyed_per_element`,
    `db::prev_init_tests::ltm_snapshot_element_reads_preserve_score_topology_and_values`,
    the mapped-foreign-dimension row of
    `every_prev_init_argument_shape_agrees_between_the_parse_and_codegen`,
    `db::element_scope_tests::a_per_element_capture_scores_its_own_element_only`
    (an element-bound helper scores its own element and no other, values
    pinned on an asymmetric fixture), and
    `db::ltm_element_instance_tests::an_arrayed_capture_helper_is_not_treated_as_element_bound`
    and `an_arrayed_capture_helpers_scores_compile`.
11. A loop through an `Ast::Arrayed` target that reads its source in a strict
    subset of its slots is reported at those slots only: the value-gate
    golden's `r2[1]` and `r2[2]` rows (identically zero, a loop claimed at
    Boston and LA that exists at NYC alone) are gone and `r2` is one scalar
    loop. Pinned by `db::analysis::classify_cycle_tests::target_restricted_bare_edge_forces_slow_path`,
    `partial_slot_arrayed_reference_takes_the_slow_path` and the regenerated
    `ltm_value_golden/value_gate.txt`.
12. `DELAYN`/`SMTHN` without an initial value wire `[input, delay_time]`; the
    base hoisted the input a second time into `initial_value`. `DELAY` is
    `DELAY1`. Arities 2 and 5 refuse with `BadBuiltinArgs`, and an order other
    than the literal 1 or 3 with `UnknownBuiltin`, before any hoist. Pinned by
    `db::implicit_module_tests` (the `SMTHN`, `DELAYN`, explicit-initial and
    `DELAY` rows, `delayn_and_smthn_refuse_bad_arities_and_unsupported_orders`,
    `mdl_delay_n_and_smooth_n_import_their_initial_value_as_the_fourth_port`)
    and `simulate::delayn_and_smthn_omitted_initial_values_are_the_input_on_both_backends`
    (equal to the explicit-initial twins on the VM and on wasm, and across a
    reset). C-LEARN's `DELAY N`/`SMOOTH N` calls all carry an initial value,
    so its artifact does not move.
13. Four shapes compile that the base refused: an explicit `Ast::Arrayed`
    slot beside a module-bearing EXCEPT default keeps its own element context,
    a snapshot-only default is captured once and read in the missing slots, a
    module-bearing default is materialized per missing slot (the base refused
    all three as `dimension_in_scalar_context`), and a snapshot nested in a
    snapshot under an apply-to-all body (`PREVIOUS(INIT(vals[d] * 2) + 1,
    0)`) captures structurally twice (the base refused it as
    `duplicate_variable`, two per-element walks claiming one capture) -- each
    equal to its plain twin on the VM. The wildcard spelling `SMTH1(vals[*],
    3)` under an apply-to-all body compiles too, reading the active element
    as the plain `vals[*]` does. Pinned by `db::element_scope_tests` (the four
    rules) and
    `mapped_reference_semantics_tests::a_wildcard_argument_under_an_apply_to_all_body_reads_the_active_element`.

**Phase 7.1 probe.** The execution-count probe chunk 7.1 owed AC3.1 is
`db::exec_probe::ProbedDb`: a `SimlinDb` built over salsa storage carrying an
event callback, which records every `EventKind::WillExecute` database key and
reports query name -> `(bodies run, distinct keys)` over a measured region. It
counts every tracked query at once, where `db::fragment_compile`'s
`note_fragment_execution` counts the four fragment compilers by instrumenting
their bodies; both are needed, because only the second says WHICH variable
recompiled. What the probe establishes about
`implicit_helper_add_is_tight_but_module_helper_add_is_not`'s two edits, on the
model `k = 3`, `probe = k * 2`:

| query | plain `PREVIOUS` helper added | `SMTH1` added |
|---|---:|---:|
| `parse_source_variable_with_module_context` | 1 | 13 |
| `variable_direct_dependencies` | 1 | 13 |
| `compile_var_fragment` | 1 | 8 |
| `compile_implicit_var_fragment` | 1 | 2 |
| `model_variable_by_name` | 2 | 9 |
| `var_runlist_membership` | 3 | 8 |
| `model_module_ident_context` | 1 | 3 |
| `model_implicit_var_info` / `model_implicit_var_by_name` | 1 / 1 | 2 / 2 |
| `model_dependency_graph`, `compute_layout`, `assemble_module`, `model_flows_invariant`, `implicit_var_runlist_membership` | 1 each | 2 each |
| `variable_dimensions`, `variable_size`, `variable_relevant_dimensions`, `source_var_is_table_only` | 1 each | 6 each |
| `model_shape`, `model_ltm_implicit_var_info` | 0 | 1 each |
| `project_module_graph`, `assemble_simulation` | 1 each | 1 each |

Every count is one execution per distinct key. The control region -- an
identical re-sync -- runs nothing at all in either case, so the numbers are the
edit's.

The `SMTH1` column is two different things added together, and the distinction
is what 7.4 needs. Five of the eight `compile_var_fragment` runs and ten of the
thirteen parses are the `stdlib⁚smth1` template compiling for the first time
(its five variables, parsed under the two contexts assembly demands for it: the
model's own and the per-instance one widened by the instance's module-input
names), along with `model_shape` and the second `compute_layout` /
`assemble_module`. That is a new sub-model, not saturation. The saturation
proper is `k` and `probe`: two fragments and two parses that should have been
reused (the added `smoothed` itself is new work).

**One cause, and it is not the one the pinning test named.** Two experiments
isolate it, both in
`module_helper_add_saturates_only_through_the_module_ident_context`:

- Adding a plain aux to a model that ALREADY instantiates a module -- stdlib
  template compiled, instance wired -- is completely tight: one fragment, one
  parse.
- Adding a SECOND `SMTH1` to that same model re-parses and recompiles `k`,
  `probe` and the first `smoothed`, while the `stdlib⁚smth1` template's own
  variables do not recompile at all.

So the module instance, the wired ports and the spliced template are all
innocent; growing the model's module-ident set is the whole cause.
`model_module_ident_context` derives that set from `model.variables(db)`, mints
a new interned `ModuleIdentContext` when it grows, and that handle is both a
KEY of `parse_source_variable_with_module_context` and `variable_direct_dependencies`
and a VALUE read inside `explicit_fragment_input`, `build_var_info` and
`model_implicit_var_info`. A changed key cannot backdate -- there is no prior
memo to compare against.

Both of the other causes on the record are gone or were never real. The
whole-`model_module_map` clone the investigation named is gone: Phase 3 deleted
that query, and it appears in neither column. `project.models(db)` changing is
not a cause and never was: `db::sync` splices every stdlib model on every sync
and calls `set_models` only on a changed map, so `stdlib⁚smth1` is in the map
from the first sync of a project that never mentions it, and the probe asserts
the map is identical across both edits.

Classified against the remedies AC3.1 has: fixed by the parse-key rule --
`parse_source_variable_with_module_context`, `variable_direct_dependencies`,
`compile_var_fragment` and `model_implicit_var_info` for `k`/`probe`, all four
through the single `ModuleIdentContext` edge. Fixed by a projection -- nothing;
the projections AC3.1 needs (`var_runlist_membership`,
`model_implicit_var_by_name`, `model_variable_by_name`) are already in place and
already backdate, which is why `var_runlist_membership` runs eight times and
recompiles nothing. Inherent -- the new sub-model's first compile, and the
per-model queries of the edited model itself. **So 7.4 needs exactly one thing
for AC3.1 to flip: delete `ModuleIdentContext`, from the parse key and from
every caller that derives it (the eleven `model_module_ident_context` call
sites and the empty-context twins, which is chunk 7.4's list). No new
projection is warranted.**

**Phase 7.1 predicate.** `snapshot_arg::SnapshotArg::access` is the one
statement of what `PREVIOUS`/`INIT` can read directly: a reference into one
variable's storage whose every index resolves before the run is a `Slot` when
the indices pin one element and a `View` when a dimension is left standing;
everything else is a `Capture`. `BuiltinVisitor::snapshot_arg` classifies the
source `Expr0` argument into it (replacing `needs_temp_arg` and
`arg_is_array_shaped`, which is the `View` arm) and `codegen::lowered_snapshot_arg`
classifies the lowered `Expr` (feeding `static_slot` and
`Compiler::snapshot_static_view`), so the parse's capture decision and
codegen's direct-read decision cannot drift -- the GH #568 class. Codegen keeps
its three refusals of an argument that IS addressable (a view naming one
dimension twice, an array-valued `PREVIOUS` with a non-default fallback, an
array-valued call in a scalar position outside an iteration): those are
questions about what the reference means where it sits, not about whether it
addresses storage.

`db::prev_init_tests::every_prev_init_argument_shape_agrees_between_the_parse_and_codegen`
derives its rows from both replaced rule sets and reads every verdict through
production. It records four shapes where the two sides classify the same
argument differently. Three are the parse being STRICTER -- a capture where
codegen would have read the slot directly, so wasted slots and fragments rather
than wrong numbers: `PREVIOUS(module-call aux)` (D1, whose capture the
context-free parse does not synthesize -- "Phase 7.4 context-free parse");
a bare element index, `PREVIOUS(vals[e1])` (D3, a 7.5 item); and a QUALIFIED element index
`PREVIOUS(vals[Dim.elem])` in a **scalar** user equation (7.5 too). The last is worth its own line: the qualified form is documented as
folding to a constant regardless of context, but the branch of `index_is_static`
that recognizes it needs the qualified dimension in `dimensions_ctx`, and the
parse narrows that context to the variable's own relevant dimensions and their
map chains (a dimension edit must not re-parse every unrelated variable) --
empty for a scalar -- so the same argument captures in a scalar equation, or in
an apply-to-all over an unrelated dimension, and does not in an apply-to-all
over `Dim`. The fourth runs the other way: `PREVIOUS(arr)` for
an arrayed `arr` in a scalar position, where the parse calls a bare name whole
storage because `Expr0` carries no arity and codegen refuses the array-valued
read. That one costs nothing -- the equation is ill-typed with or without the
`PREVIOUS` (`x = arr` refuses too, with a different message) and the refusal is
loud -- and it closes the same way the other three do, by giving the decision
the dimensions, which is what moving it to lowering does.

**Phase 8.2 semantic divergences.** Two corrected shapes, neither in the
corpus. (1) The `main` rule of module wiring -- a parent-scope `·x` source is
stripped in the root model (`db::build_module_inputs`) -- compares CANONICAL
model names, so a root model whose display name is spelled `Main` wires an
instance fed from `.x` exactly as one spelled `main` does. Every lowering
resolves the wiring under the canonical name (`lowered_source_variable`,
`lowered_implicit_variable`, the LTM helper constructor); pinned by
`db::lowered_variable_tests::module_wiring_strips_the_parent_scope_prefix_under_a_display_cased_main`,
which simulates both spellings. (2) A rename is syntactic (`patch.rs`): an
equation the compiler refuses (`bad = a + b` over `a[d]`, `b[p]`) is renamed
rather than left holding the old name, which turned the refusal into an
unknown dependency on every patch surface, and a module-function call keeps
its call (`SMTH1(x, 3)` renames to `smth1(w, 3)`, the parser's spelling of the
builtin's name on either tier) rather than being replaced by the instance's
output read (`"$⁚y⁚0⁚smth1·output"`); pinned by
`patch::tests::rename_rewrites_an_equation_the_lowering_refuses` and
`rename_keeps_a_module_function_call_as_written`, and no pinned rename output
elsewhere changes spelling. Nothing else moves: the C-LEARN artifacts, the
sweeps and the `test/` diagnostics corpus are identical (ledger row), and the
LTM describers read the same `Expr2` the compiler reads -- lowered under
dependency shapes rather than bounds-free -- which changes no describer
answer, since none reads an `ArrayBounds`
(`ltm_agg::AggNode` strips them from its cached key, now load-bearing:
`ltm_agg_tests::the_carried_reducer_is_normalized_so_offset_only_edits_backdate`).
Incrementality gained, each pinned by `ProbedDb` body counts: an equation
edit re-lowers and recompiles the edited variable alone (a dependency's tables
reach a fragment through the tracked `variable_tables`;
`fragment_char_tests::equation_only_edit_recompiles_only_the_edited_fragment`,
`lowered_variable_tests::an_equation_edit_relowers_only_the_edited_variable`),
and a module target's edit re-executes its instantiators' unit checks and
re-lowers only the edited variable
(`units_tests::a_module_targets_edit_invalidates_the_unit_check_and_not_the_instantiators_lowering`).

**Phase 8.1 semantic divergences.** One mechanism, pinned by one
enumeration: a parse-synthesized helper lowers under its parent's dependency
shapes (`db::fragment_compile::implicit_fragment_input`), so it reads, and is
refused, exactly as the plain spelling of its body is -- the invariant GH
#1035 established for hoisted arguments, holding at the `Expr2` tier as well.
The base lowered every helper bounds-free, so the compiler's bare-reference
rewrite (`lower_pass0`) never ran on a helper's arrayed references, and
wherever that rewrite decides the answer the helper and the plain spelling
disagreed. The arms:

1. Values, `db::lowering_scope_tests::a_helper_reads_what_the_plain_spelling_reads`:
   a reducer over a bare arrayed name in an apply-to-all body reads the
   element the plain spelling reads, for every reducer in {`SUM`, `MAX`,
   `MEAN`, `SIZE`} x helper kind {`PREVIOUS` capture, `INIT` capture,
   per-element hoisted argument, capture inside a module-bearing body} x
   target rank {1-D over a 1-D source, 2-D over a 2-D source, 1-D over a 2-D
   source, a row} -- 48 rows, each pinned against its plain twin, the twins
   pinned apart from the whole-array spelling (`R(x[*])`, `R(x[*, *])`).
   `agg[region] = PREVIOUS(SUM(pop))` is the lagged `pop[region]`,
   `SMTH1(SUM(pop2), 1)` under `[region, product]` smooths the cell,
   `PREVIOUS(SIZE(pop))` is 1; the base's helpers read the whole array
   (`SUM(pop[*])`, `SIZE` = 2, the 2-D total 1200) where the plain spelling
   read the element. What the plain spelling reads is the engine's
   apply-to-all rule, which rests on XMILE 1.0 section 3.7.1 -- "when all
   indices are dimension names, they can be omitted": `revenue = sales` is
   identical to `revenue[Location, Product] = sales[Location, Product]`, and
   each element's equation has the dimension name bound to its index --
   applied to a bare name in a reducer argument, a position the spec does not
   address separately; what Stella computes for `SUM(pop)` in an apply-to-all
   body is unverified. The divergence rests on the helper == plain-spelling
   invariant, not on the spec.
2. Refusals, `db::lowering_scope_tests::a_helper_is_refused_where_the_plain_spelling_is`:
   a helper is refused exactly where the plain spelling is, with the refusal
   where the Phase 7.5 rule puts it -- an `Expr2` refusal on the parent at
   the argument's span, a codegen refusal on the helper.
   `SMTH1(sales[Cities] + prices[Products], 1)` is the plain spelling's
   `MismatchedDimensions` on the parent for a scalar parent (base: the helper
   reached codegen and was refused `NotSimulatable` on the helper) and for an
   apply-to-all parent over either axis, hoisted or captured (base: RAN,
   pairing the unrelated axis by ordinal, `sm_a2a[Boston] = 11`, `[Seattle] =
   22`); and `aggx[region] = PREVIOUS(SUM(pop * scale))` is refused by codegen
   on the helper, as `plainx[region] = SUM(pop * scale)` is on both trees
   (base: ran, 600, the whole-array sum times the scale).

No corpus model has any of these shapes: the sweeps and the diagnostics corpus
are identical. Three things did not move, and each is pinned so that stays
true: helpers with well-formed arrayed operands lower to the same numbers with
or without bounds (`a_hoisted_argument_pairs_axes_as_the_plain_equation_does`:
transposed operands pair by axis name either way); a module-output read
carries no bounds on the compile path and the unit path alike
(`the_expr2_tier_reads_dependency_shapes`,
`stages_tests::stage1_lowers_a_module_output_read_without_bounds`); and a
lowering refusal outranks an unknown name on the same variable
(`a_lowering_refusal_outranks_an_unknown_dependency`). On the unit path a
variable whose only `MismatchedDimensions` arrives through a module-output
read keeps its AST and is unit-checked -- `mism[d] = m.arr + prices` reports
both the compiler's refusal and the `unit_mismatch` warning
(`a_module_output_read_the_compiler_refuses_is_still_unit_checked`); the
`test/` corpus has no such variable and reports the same diagnostic rows with
the same per-code distribution before and after (ledger row). A module
target's edit re-executes its instantiators' unit checks and not their
lowered stages
(`stages_tests::a_module_targets_edit_invalidates_the_unit_check_and_not_the_instantiators_stage`).

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
   and `rank_and_vector_sort_order_read_the_same_operand_in_both_spellings`.
   The same fact closed a divergence between the LTM scoped-relower gate
   (`db/ltm/compile.rs`) and the materialization set it restated: the gate
   classified `RANK` as non-decomposing while the argument was decomposed
   (GH #995), so an LTM fragment embedding `RANK(<computed array>, d)` took
   the unscoped lower. The gate restates no set (Phase 6b); the LTM side is
   pinned by
   `scoped_relower_gate_tests::every_ltm_fragment_compiles_on_both_sides_of_the_scoped_relower_gate`.
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
   `db::temp_allocation_tests::default_arm_beside_an_override_reads_its_own_operand_temp`,
   `explicit_arm_beside_another_reads_its_own_operand_temp`,
   `default_arm_with_two_operand_temps_reads_both_of_its_own`, and the 2-D
   `two_d_override_beside_a_shared_default_reads_its_own_operand_temp`.

**Phase 5a semantic divergences.** Deriving the results-offset map from the
layout changed how three shapes are keyed. None occurs in the corpus (C-LEARN's
offsets maps, plain and under LTM, are identical key for key and slot for
slot); the first is a bug and is pinned, the other two are named because no
generator produces them:

1. A sub-model containing a standalone lookup-only table (empty equation plus a
   graphical function, GH #606). The table reserves a slot in the sub-model's
   layout, so every parent variable laid out after the module instance -- and
   the implicit and LTM sections after it -- sits one slot further on than the
   sum of the sub-model's exposed series. The running-count flatten advanced
   the parent by that sum, keyed everything after the module one slot early
   per such table, and `Vm::get_series` read a neighbouring slot's data
   (`zzz` read `trailing`'s 42 and `trailing` read the root table's reserved
   slot). The layout-derived map
   keys them at their layout slots. Pinned by
   `db::assemble_tests::results_offsets_are_the_assembled_layouts_offsets_on_a_module_bearing_model`
   (every key equals the composed layouts; the VM reads 42 through the name).
2. A module-typed LTM implicit helper's sub-variables are keyed `helper·sub`,
   the separator every other module instance uses, where the running-count
   flatten spelled them `helper.sub`. `model_ltm_implicit_module_refs`'s
   rustdoc records that LTM equations never contain module-function calls, so
   the map is empty in practice.
3. An LTM synthetic variable whose name carries a quoted identifier with a
   literal period (`"a.b"`) is keyed by its canonical name like every other
   variable; the running-count flatten keyed it through `to_source_repr`,
   which rewrote the period sentinel to `.`, a spelling `Ident::new` reads back
   as the module separator.

**Phase 6b semantic divergences.** Making `compiler/array_operand.rs` the one
materialization pass, and deleting the `Expr3` decomposition it duplicated,
changed how nine shapes compile. C-LEARN's artifacts move (ledger row 6b) and
one corpus model moves; each is pinned:

1. **Once per equation is decided on the lowered body, not on the equation.**
   An array value two or more elements of an apply-to-all or arrayed equation
   read is materialized ONCE, hoisted ahead of the element code; one that
   differs per element is materialized per element on an id the elements
   REISSUE. The old rule classified the whole equation
   (`expression_depends_on_active_dimension` lowered every element twice to
   compare), so an equation with one element-invariant subexpression beside one
   element-varying one took the per-element regime for both, and a per-element
   hoist spent one id per ELEMENT. Identity includes source positions, so two
   explicit arms share a body only when they spell it at the same offset --
   the shape `a[t1] = SUM(VECTOR SORT ORDER(input[D], 1))` beside
   `a[t2] = a[t1] + SUM(VECTOR SORT ORDER(input[D], 1) * 1)` evaluates two
   sorts, as it always did. Values are unchanged -- the five array-producing
   builtins are pure and the fragment writes nothing they read -- and every
   temp count falls: C-LEARN's LTM artifact goes from 441 temp slots to 28, and
   a 300-element per-element hoist over a materialized operand from 600 ids to
   2, which takes GH #583's `u8` namespace out of reach of any equation shape.
   The C-LEARN `sorted target` shape -- `VECTOR ELM MAP(Src[COP,t1], Target
   Order[COP,Target])` over `[COP,Target]` -- stays one map per equation,
   because a vector builtin's operand is promoted to the whole axis whichever
   element is active and the fixed-column slice is the same view for every
   element. Pinned throughout `db::temp_allocation_tests`, whose rows are
   derived from the pass's two enumerations (where it fires; which regime the id
   gets), by
   `array_operand_materialization_tests::a_per_element_hoist_over_a_shared_operand_costs_two_ids_at_any_size`,
   `an_elm_map_over_a_fixed_column_slice_is_materialized_once_per_equation` and
   `a_sort_order_over_the_iterated_axis_is_materialized_once_per_equation`;
   the refusal itself keeps its own pin at
   `symbolic::tests::test_resolve_static_view_temp_past_the_id_namespace`.

2. **A subscript naming a dimension resolves through one rule, whichever
   dimension it names.** Pass 1 folded a subscript naming an ACTIVE dimension to
   that dimension's ordinal and indexed the source's storage raw, while a
   subscript naming any other dimension went to
   `DimensionsContext::resolve_mapped_read` (the active element's own name on
   the source axis, then the declared element map, then a mapped parent). With
   Pass 1 gone both take the second route, and reading by ORDINAL survives only
   as its last resort, where the two dimensions declare no correspondence at
   all. This is the largest behaviour change in the phase and it is a
   CORRECTION: `test/test-models/tests/subrange_merge/` ships genuine Vensim
   output beside its `.mdl` and nothing simulated it (the corpus list is built
   from `.xmile` files and that directory has none), and
   `lower values[lower] = value per layer[lower]` over `lower: Layer2, Layer3,
   Layer4` moves from `1, 2, 3` to Vensim's `2, 3, 4`. Pinned by
   `simulate.rs::a_subrange_subscript_reads_the_element_it_names`, and cell by
   cell over the (mapping kind x spelling x declaration direction) matrix by
   `mapped_reference_semantics_tests`, whose expectation table is keyed on the
   mapping kind alone -- all four spellings agree, and
   `a_bare_equation_reference_and_a_flow_reference_agree` says so directly where
   its predecessor pinned the disagreement.

   A second corpus consequence, measured in the sweep:
   `sdeverywhere/models/arrays_cname` and `arrays_varname` are subject to
   GH #859, the importer's nondeterministic dimension ordering, and on the base
   that coin flip reached the VALUES -- canonicalizing each run by sorting its
   columns leaves TWO distinct forms. On the tree it leaves one. Reading by
   element name rather than by the active element's ordinal is what makes a
   permuted declaration order stop changing which element a reference reads.

   The LTM attribution surfaces describe the iterated spelling by the same
   rule (V9b: `DimensionsContext::executed_read_correspondence` is the one
   correspondence; it differs from the ordinal diagonal only for a declared
   element map that permutes at equal cardinality, and the element-attribution
   tests in `db::analysis::element_graph_tests`, `ltm_agg_tests`,
   `ltm_augment_pin_tests` and `db::ltm_ir_tests` pin the map's answer).

3. **Each occurrence of a repeated active dimension reads its own axis.**
   `o[D,D] = square[D,D]` (and the bare `o[D,D] = square`, which pass 0 rewrites
   into it) read the DIAGONAL `square[d_i,d_i]`, disagreeing with the wildcard
   spelling `square[*,*]`. `compiler::subscript::normalize_subscripts3` now
   allocates the active positions one to one across a reference's subscripts,
   and `compiler::project_var_index_to_temp` pairs a temp's axes to the
   variable's the same way, so all three spellings read the cell. Vensim rejects
   the declaration outright ("DimA appears more than once on LHS", measured in
   Vensim DSS 2026-08-04 on `vensim-probes/repeated_dimension.mdl`) while XMILE
   v1.0 exemplifies it and says nothing about the reference, so the reading is
   Simlin's to define. Not in the corpus; pinned by
   `simulate.rs::each_occurrence_of_a_repeated_active_dimension_reads_its_own_axis`
   (VM values plus `ensure_wasm_matches`) and by
   `array_operand_materialization_tests::a_repeated_dimension_read_directly_reads_each_axis`,
   which is not a residual on the execution side.
   `db::analysis::expand_same_element`'s repeated-target residual is the same
   root cause on the LTM side and is unchanged.

4. **`VECTOR SELECT`'s two array positions are REDUCED, not WHOLE.** The
   signature table classified them as whole-array operands, which only ever
   reached lowering when the enclosing equation ALSO held an array-producing
   builtin (that is what selected the wildcard-preserving mode); every other
   spelling took the ordinary per-element path. With one mode the classification
   decides every call, and the corpus says which one is right: genuine Vensim
   output in `test/sdeverywhere/models/vector/` runs
   `q[DimB] = VECTOR SELECT(e[DimA!,DimB], c[DimA!], 0, VSSUM, VSERRNONE)` and
   `r[DimA] = VECTOR SELECT(e[DimA,DimB!], d[DimA,DimB!], :NA:, VSMAX,
   VSERRNONE)`, where the `!` marks the reduced axis and the LHS's own dimension
   is an ELEMENT of the operand. `VECTOR SELECT` reduces to a scalar, so
   `ArgKind::Array { whole: false }` is both the Vensim rule and the reading the
   corpus already exercised. `VECTOR ELM MAP`, `VECTOR SORT ORDER`, `RANK` and
   the `ALLOCATE` family stay whole, which is what their own ground truth says
   (`test/test-models/tests/vector_order/`, `test/sdeverywhere/models/allocate/`,
   C-LEARN's `Target Order[COP,Target]`). Pinned by the `[*]`-spelled rows of
   `array_operand_materialization_tests::vector_select_positions` and by
   `the_gh_1001_user_shape_compiles_and_reads_the_previous_row`, which reads one
   row of the previous step's matrix per element.

5. **Two operand shapes neither containing the other broadcast into their cross
   product, in every position.** `SUM(a[*] + h[*])` over disjoint named
   dimensions has always been the 3 x 3 cross-product sum Vensim's own output
   gives (`test/sdeverywhere/models/sum/sum.xmile`), because `Expr3`'s
   decomposition unioned the axes left to right; the post-lowering materializer
   declined the same shape, so which answer an equation got depended on which
   pass saw it -- `out[X,Y] = RANK(a[*] + b[*], 1)` compiled and
   `out[X,Y] = RANK(a[X] + b[Y], 1)` was refused, for one operand. One rule
   answers both: `compiler::join_array_views` takes the containment join where
   there is one and the left-to-right union otherwise, which is the same order
   `ast::Expr2` already assigns to the expression's bounds. What Vensim's own
   output establishes is the REDUCER case only (`sum.dat`, the 198 above); that
   `RANK` and `VECTOR SORT ORDER` should read a cross-product operand at all,
   and in that axis order, is UNVERIFIED -- Vensim has no such example and the
   spelling that reaches it here is Simlin's `[*]`, not Vensim's `!`. The rows
   pin what this engine does, in both operand orders, so a future ground truth
   moves a test rather than a silent number. A view that leaves
   an axis unnamed or names one TWICE cannot be paired by name and still
   declines. Pinned by
   `array_operand_materialization_tests::incomparable_operand_shapes_broadcast_into_their_cross_product`
   (which rows both operand orders, because the axis order is the axis
   `VECTOR SORT ORDER` sorts along) and
   `builtin_signature_tests::rank_and_vector_sort_order_read_the_same_operand_in_both_spellings`.

6. **An array-producing builtin nested inside a computed operand is
   materialized first.** `VECTOR SORT ORDER(VECTOR ELM MAP(a, b) + c, 1)` was a
   disclosed residual: the `Op2` became a temp whose `BeginIter` body still held
   the inner call, which codegen cannot emit. Materializing every array value in
   its own array-valued position closes it, and the inner temp is written before
   the body that reads it. Pinned by
   `array_operand_materialization_tests::a_nested_array_producing_builtin_inside_arithmetic_materializes_first`.

7. **A lookup's TABLE argument lowers like a reducer's operand, and an array
   value in a one-value position gets one diagnostic.** The table position used
   to lower in the enclosing context, so inside an apply-to-all body a free axis
   of the table collapsed and `out[COP] = LOOKUP(g, Time)` over a `g[COP, ROW]`
   holder compiled to a fabricated positional read; it now keeps the free axis,
   is materialized as the per-element arrayed-GF apply it is, and -- having no
   correspondence to project the element through -- is read WHOLE and refused as
   an array in a one-value position. OBSERVABLE BEHAVIOUR IS UNCHANGED: the base
   refuses `out[COP] = LOOKUP(g, Time)` over a `g[COP, ROW]` holder with the
   identical diagnostic, Phase 6a having already unified the two arms onto one
   message, so this item is a change of ROUTE (the table keeps its free axis and
   is materialized, rather than collapsing to a fabricated positional read that
   some other check would have to catch) and not of what a user sees. Pinned by
   `per_element_gf_tests::array_valued_table_apply_assigned_to_one_slot_is_refused_not_aborted`,
   whose rows cover both the `StaticSubscript` and the `TempArray` arm.

8. **A shared materialization inside a resolved recurrence SCC is evaluated
   before every element that reads it.** The combined fragment emits a
   member's segments in `element_order`, and a temp two or more elements read
   is emitted ONCE as the member's prologue, immediately before the first of
   its readers in that order; `symbolic_phase_element_order` wires the
   prologue's current-value reads into exactly those readers. A hoist that
   rides on one element's segment is instead written wherever that element
   lands: an EXCEPT default `SUM(VECTOR SORT ORDER(v[t] * a[t3], 1))` shared by
   `a[t1]` and `a[t2]` beside the override `a[t3] = 7` puts the hoist in
   `a[t1]`'s segment with its `a[t3]` read attributed there, leaves `a[t2]`
   with no ordering edge, and schedules `a[t2]` FIRST -- a well-formed program
   reading zero-initialised temp storage on step 1 (`a[t2] = 0` at `t = 0`,
   3 afterwards, because the temp region persists across steps; measured on
   the pre-change CLI over an XMILE spelling of that model). It is `[3, 3, 7]`
   on every step now. The same mechanism seen from the override's side is a
   base FALSE refusal: `a[t1] = 7` beside a default `SUM(VECTOR SORT ORDER(v[t]
   * a[t1], 1))` for `t2, t3` put the hoist in `t1`'s segment reading `t1`, a
   self-loop, and was refused as `CircularDependency`; it is `[7, 3, 3]` now.
   Wiring the reads into the readers only, rather than into every element, and
   lifting only what two or more elements read, is what keeps every recurrence
   with an acyclic element graph compiling: a prologue that reads an element
   which reads nothing of it orders that element first, a prologue that reads
   one of its own readers is an element self-loop, and a once-written temp
   only the first element materializes (`a[d1] = SUM(VECTOR SORT ORDER(v[D], 1))
   + SUM(VECTOR SORT ORDER(w[D] * a[d3], 1))` beside a shared default) stays
   that element's, so its `a[d3]` read orders `a[d3]` before `a[d1]` alone.
   Pinned by
   `dep_graph_tests::a_shared_default_reading_an_override_element_runs_the_override_first`
   and `a_prologue_read_of_a_non_reader_element_orders_that_element_first`
   (every step, through the VM), `simulate.rs::a_shared_materialization_inside_a_recurrence_scc_precedes_every_reader`,
   `two_arms_each_materializing_a_temp_inside_a_recurrence_compile_in_both_orders`
   (the shape a guard on "a temp touched by two segments" would refuse) and
   `a_private_materialization_beside_a_shared_one_stays_in_its_element` (the
   two rows above; all three VM plus `ensure_wasm_matches`), the refusal
   `dep_graph_tests::a_prologue_reading_an_scc_member_refuses_the_recurrence`,
   and the combiner's own rows in `db::combined_fragment_tests`, including
   `a_private_block_beside_a_shared_temp_stays_in_its_element`.

9. **A temp axis the target does not name is read through the declared
   correspondence, and refused without one.** `out[D] = VECTOR SORT ORDER(w[E],
   1)` materializes a `[E]`-shaped temp that `out[D]` reads back per element.
   `compiler::project_var_index_to_temp` pairs the temp's axes to the
   variable's by name first; an axis the variable does not name is resolved
   per element through `DimensionsContext::resolve_mapped_read` -- the
   element's own name on the temp's axis, then the declared map, then a mapped
   parent -- which is the rule an ordinary reference `x[E]` resolves by (GH
   #997). With `E -> D` declared, and likewise for two indexed dimensions,
   `out` is the sort order `[1, 2, 0]` of `w = [3, 1, 2]`; two named
   dimensions declaring nothing have no correspondence, so the temp is read
   WHOLE and refused as an array in a one-value position. The old projection
   read coordinate 0 on every such axis, so all three spellings gave `[1, 1,
   1]`: a plausible array that answered a question the model did not ask. The
   refusal is the one new loud refusal of a base-compiling shape in this
   phase, and it replaces a silent wrong number. Not in the corpus; pinned by
   `simulate.rs::a_temp_axis_the_target_does_not_name_is_read_through_the_declared_correspondence`
   (the mapped and indexed rows through the VM plus `ensure_wasm_matches`,
   and the unrelated pair's `NotSimulatable`).

The LTM lowering scope is EMPTY, and there is no scoped re-lower.
`lower_ltm_variable` lowers every LTM equation once with no model variables in
scope; the scope only ever fed `ArrayContext::get_dimensions`, which computes
`Expr2` `ArrayBounds`, and no bound is load-bearing for the fragment compiler:
every dependency's shape reaches lowering through `FragmentInput.deps`,
materialization is decided on the lowered `compiler::Expr`, and the remaining
bounds consumers are refusals (Phase 8 audit, section 4). The evidence is the
artifact, not the suite: C-LEARN's `CLEARN_LTM=1` `bytecode_profile` block is
byte-identical with the populated-scope re-lower and without it, and the LTM
goldens (`db/ltm_char_golden`, `db/fragment_char_golden/ltm_*`,
`db/ltm_value_golden`) are unchanged. The re-lower had been gated by a
signature-table scan for array-position builtins (`BuiltinFn::arg_kinds()`,
`SIZE` included), which was the right gate for a pass that read the bounds;
the drift GH #738's second round recorded belonged to the text-scan gate that
preceded it. With no bounds consumer left, the scan and the re-lower go
together, and the LTM compile channel falls with them (ledger row 6b).

**Phase 3 semantic divergences.** Feeding every fragment through one
`FragmentInput` changed how one shape compiles. It does not occur in the
corpus (C-LEARN artifacts are identical, plain and LTM); it is pinned:

1. A stdlib call whose hoisted argument reads a module output --
   `sm = SMTH1(sub·output * 2, 2)`, hoisting `$⁚sm⁚0⁚arg0 = sub·output * 2`
   -- compiles and simulates. The implicit-helper compiler used to hand a
   NON-module helper's lowering an empty module map, so the cross-module read
   inside the helper was `DoesNotExist` and the whole model failed to compile
   (`implicit variable '$⁚sm⁚0⁚arg0' ... could not be lowered:
   SimulationError{does_not_exist}`); the helper now resolves `sub` through
   its own dependency shapes like every other fragment. Pinned by
   `db::fragment_input_tests::smooth_argument_reading_a_module_output_compiles`
   with the values derived from the rules (the helper, the instance's input
   and the smooth are 60 on every step: `producer` evaluates `output` in its
   initials because every value-bearing variable of an instantiated model is
   an initials member -- "Phase 7.4 context-free parse", GH #1028 -- so the
   instance's stock starts at 60).

Every other path is byte-identical by construction and by measurement: each
emitter is its constructor's `FragmentInput`, lowered and emitted
(`db::fragment_input_tests`, one row per constructor), the fragment and LTM
goldens are untouched, and the determinism suites are green.

**Phase 4 semantic divergences.** Making one `Variable`, one borrowed parse
input and one owner per twin changed nothing observable: C-LEARN's artifacts are
byte-identical plain and under `CLEARN_LTM=1`, and every fragment and LTM golden
is unchanged. One latent DISAGREEMENT between two twins is closed rather than
preserved, and it is worth naming because the two verdicts feed different halves
of the compiler:

1. "Is this variable a standalone lookup-only table" had two implementations --
   `variable::var_is_lookup_only`, read by `parse_var` to set
   `VarKind::Aux::is_table_only` (which `Var::new` consults), and
   `db::source_var_is_table_only`, read by the layout and the dependency graph.
   They agreed on every shape except one: a per-element arrayed table holder
   whose declared dimension has ZERO elements. The parse-side twin derived "has
   a table" from the tables `build_tables` actually produced, which for that
   variable is an empty list, so it answered `false`; the salsa-side twin
   derived it from the presence of a `gf` on the variable or on any element, so
   it answered `true`. The layout therefore reserved no value slot for a
   variable the fragment compiler would still try to compile. The one
   `variable::is_lookup_only(eqn, gf)` both now call answers `true`, the
   salsa side's reading, so layout and compilation agree. (That a zero-element
   dimension yields an empty table list is the crate's own recorded behaviour:
   `dimensions::subscript_iter_tests::test_subscript_offset_iter` pins
   `[empty_dim] -> []`, and `reorder_arrayed_element_tables` collects that
   iterator.) No generator produces a zero-element dimension, which is why the
   disagreement had never fired.
   Pinned by `variable::is_lookup_only_tests::is_lookup_only_covers_every_equation_shape`,
   whose rows are the cross product of `datamodel::Equation`'s three variants,
   the equation contents (real / empty / whitespace / the legacy `"0+0"` MDL
   sentinel), and the two places a graphical function can sit.

One intended asymmetry survives between the two `VariableSource` producers, and
is pinned rather than removed: the salsa producer substitutes a conveyor stock's
§7.2 explicit init list with the constant raw-sum placeholder
(`conveyor_compile::explicit_init_list`) so the parse-only diagnostic path
accepts exactly the lists the runtime accepts, while the `datamodel::Variable`
producer leaves the equation as written -- it parses synthesized implicit
variables and the `ModelStage0` oracle, neither of which is ever a conveyor.
`db::tests::variable_source_rewrites_a_conveyor_init_list_only_on_the_salsa_path`
states it; `variable_source_producers_agree_for_every_source_variable_kind`
pins that everything else agrees, one row per `SourceVariableKind`.

**Phase 5b semantic divergences.** Giving `EquationError` a `details` field
changed no compiled artifact (C-LEARN's `bytecode_profile` blocks are
byte-identical plain and under `CLEARN_LTM=1`) and added or removed no
diagnostic: the `test/` corpus reports the same 501 rows with the same
per-code distribution before and after. What changed is the TEXT on 200 of
them -- 209 rows carried a payload before, 409 after. Of those 409, 372 are a
SENTENCE and 37 are a bare identifier (see "where the reason stops" below).
Three of the changes are user-visible enough to name:

1. An equation diagnostic's summary line gains a ` -- {reason}` tail wherever a
   reason exists, matching the shape the unit arms already used
   (`format_equation_error` and `format_diagnostic` both compose it through the
   one `errors::code_and_reason`). One test pinned the old text and is
   deliberately re-baselined: `errors::tests::equation_error_formats_snippet`,
   which now reads `unknown_dependency -- 'bogus' is not a variable of model
   'main'` and additionally pins `FormattedError::details`. No other golden or
   snapshot pins diagnostic text, and there was no `GOLDEN MISMATCH` in the
   suite.
2. A unit-definition error raised while RESOLVING a unit equation
   (`units::resolve_equation_unit`) now carries the offset inside that equation
   and the declaration it came from. It used to be re-stamped as a span-less
   copy of its own `ErrorCode`, discarding both. The offset is inert on this
   path -- these diagnostics name no model, so
   `format_diagnostic_with_datamodel` finds no datamodel variable and renders no
   snippet -- which is exactly why the declaration text has to ride in the
   reason: `no_app_in_units` on its own points at nothing the modeler can see.
   Both arms of a rejected declaration are annotated identically, the one that
   fails to lex and the one that fails to resolve, and both are rows of
   `every_diagnostic_stage_keeps_its_message`.
3. The CLI renders diagnostics through `collect_formatted_errors` rather than
   the snippet-free `format_diagnostic`, so it prints the offending equation
   with the span underlined. This is what makes the parse row's rule ("a parse
   error writes no reason, because the snippet IS the reason") true on every
   Rust surface rather than on three of four: libsimlin (through
   `format_diagnostic_with_datamodel`), both MCP servers (through
   `simlin-mcp-core`) and the CLI all render the snippet. The CLI's
   `a_parse_error_prints_the_equation_it_could_not_parse` pins it.

The FFI and TypeScript boundaries did not move. `SimlinErrorDetail` already had
a `details` field, fed from `FormattedError::details`, and
`src/engine/src/internal/types.ts` already mirrored it; the `Equation` arm was
simply always passing `None`. `cbindgen` reproduces `src/libsimlin/simlin.h`
byte for byte, and the numbered `SimlinErrorCode` enum is untouched.

**Where the reason stops.** Two boundaries, both of them outside this phase's
files:

- *The web app.* An equation reason reaches the FFI and every Rust surface, and
  no further: `src/diagram/project-controller.ts` builds a core `EquationError`
  from `{code, startOffset, endOffset}` and drops `details`, the core type
  (`src/core/datamodel.ts`) has no field to hold it, and
  `src/diagram/VariableDetails.tsx` renders `errorCodeDescription(code)`. Unit
  errors already carry `details` through the same path, so the fix is the
  equation arm catching up (GH #1030).
- *Bare-identifier payloads.* 37 of the 409 corpus rows carry an identifier
  rather than a sentence, because that is what their raising site had:
  `sim_err!(EmptyEquation, var.ident())` in `compiler/mod.rs` (18 rows,
  rendering `empty_equation -- hare_density`), `sim_err!(MismatchedDimensions,
  id)` in `compiler/context.rs` (14 rows), and
  `sim_err!(ArrayReferenceNeedsExplicitSubscripts, ident)` in the same file (5
  rows). An identifier is more than a bare code and less than an explanation.
  Those two files are Phase 6(b)'s to rewrite, and the sentences belong in that
  rewrite rather than ahead of it: the sites move.

**Phase 6a semantic divergences.** Making `dimensions::match_axes` the one
axis-matching precedence, and fixing GH #1027, changed how eight shapes
compile and deliberately WITHHELD two rungs that the plan's original "union of
what the existing matchers do" would have admitted. C-LEARN's artifacts are
identical, plain and under `CLEARN_LTM=1`; each of the ten is pinned, and the
values below were re-derived by running a pre-change and a post-change CLI
over the same model.

1. A star range over a subdimension changed in two directions at once, because
   its axis is now NAMED for the subdimension. Read through a TEMP it
   evaluates its own selection instead of NaN; read into an apply-to-all whose
   own axis is the PARENT it is refused instead of silently reading rows the
   selection excludes.

   The NaN half is GH #1027, and it is larger than the issue states: the
   issue's root cause (`ArrayView::transpose` cloning `sparse` without
   renumbering `SparseInfo.dim_index`) is real but is not why the probe read
   NaN. Two statements of one fact disagreed -- `ast::Expr2`'s bounds name a
   star range's axis for the SUBDIMENSION while
   `compiler::subscript::build_view_from_ops` named it for the PARENT (its own
   `TODO` said so) -- so the temp a reference materializes into and the source
   view it is filled from carried different `dim_id`s, the VM's broadcast
   matcher paired nothing, and every element read the NaN the temp was filled
   with. That reaches six spellings, not one: `SUM(arr[*:Sub, *]')`,
   `SUM(arr[*:Con, *]')`, `SUM(arr[*:Sub, *] * 2)`, `SUM(arr[*:Con, *] * 2)`,
   `SUM(row[*:Sub]')` and `SUM(row[*:Sub] * 2)` were all NaN, transposed or
   not, contiguous subdimension or not. Both defects had to be fixed together:
   with only the name aligned the transposed read reaches `flat_offset` with a
   stale mapping and indexes a 2-element `parent_offsets` at 2 -- a bounds
   panic, which under `panic = abort` takes the host process.

   The refusal half follows from the same rename and is the price of it. With
   `Parent{A,B,C,D}`, `Sub{A,C}` and `Other{X,Y,Z}`, `out[Parent,Other] =
   arr[*:Sub, *]` over `arr[Parent,Other]` used to compile: the range's axis
   was named `Parent`, name-matched the target's `Parent`, and resolved each
   element through it, so `out[b,x]` was 21 and `out[d,x]` was 41 -- rows `B`
   and `D`, which `Sub` does not select. Now the axes do not pair and the
   positional read fails its length check (2 selected rows against a 4-element
   target): `MismatchedDimensions` on the variable. The transposed spelling
   `out_t[Other,Parent] = arr[*:Sub, *]'` moves the same way (`out_t[x,b]` was
   21), and so does a target axis that reaches `Parent` through a declared
   mapping rather than by name (`out[DimM,Other]` with `DimM -> Parent`:
   `out[m2,x]` was 21). A target mapped onto `Sub` instead is refused on both
   sides of the change.

   Pinned by
   `simulate.rs::transposed_star_range_over_a_noncontiguous_subdimension_reads_its_own_selection`
   (VM values derived from the rules, plus `ensure_wasm_matches`),
   `simulate.rs::a_star_range_over_a_subdimension_is_refused_by_a_parent_target`
   (all four spellings, on the whole diagnostic vector),
   `ast::array_view::tests::transpose_renumbers_a_sparse_axis` and its
   `reorder_dimensions` and two-mapping siblings, and the transposed
   star-range row of `wasmgen::views::tests::static_view_geometries_address_like_vm`.
   The element-wise consumer (`t_elem[Other, Sub] = arr[*:Sub, *]'`) was
   already correct and is rowed so a future change cannot fix one path and not
   the other. Measured on the corpus: `test/test-models/tests/subscript_transposition`
   has genuine Vensim DSS 7.3.4 ground truth checked in beside it
   (`output.tab`) that nothing asserts, and its transposed `output2` moves
   from all-NaN to exactly Vensim's `109, 1090, 109, 141, -13`.

2. WITHHELD: the subdimension rung is OPT-IN (`AxisRelations::is_subdimension`,
   admitted only by `SubdimensionRelations`, whose one caller is the
   dynamic-range arm that picks which axis to compare positions against and
   never resolves an element through the answer). Admitting it everywhere --
   the literal union the plan first described -- would break both directions,
   in different arms. `out[Sub] = src` over `src[Parent]` would break in
   `make_dimension_subscripts`, the only arm affected: what it can emit for a
   paired axis is a dimension-name subscript, so the reference becomes
   `src[Sub]`, which resolves to the ACTIVE dimension's ordinal and reads
   `src`'s SECOND element for `Sub`'s `C` rather than its third (GH #1029,
   pre-existing and filed). `out[Parent] = src` over `src[Sub]` would break in
   the Subscript arm's element step: pairing the axes skips the positional
   length check, and the element step then falls back on the target axis's
   ordinal -- `Parent.get_offset(B)` is 1 and `Parent.get_offset(D)` is 3,
   indexing a two-element `src`. Both are loud `MismatchedDimensions` refusals
   today, and the rung trades them for silent wrong numbers, so it waits on
   #1029. Pinned by `axis_match_tests`'s
   `a_subdimension_does_not_pair_for_a_caller_that_does_not_admit_the_rung`
   and `a_subdimension_does_not_pair_for_an_ordinal_resolving_caller_either`,
   beside the two rows that do admit it.

3. WITHHELD: for the same reason the mapping-onto-a-PARENT rung is kept from
   `make_dimension_subscripts` (`DirectMappingsOnly`), which can only emit a
   dimension-name subscript and so resolves the paired element by ordinal. A
   direct mapping is safe there -- the ordinal read is the documented
   bare-reference rule (GH #527 / #997,
   `DimensionsContext::executed_read_correspondence`'s rustdoc) -- while the parent one runs
   target -> parent -> source and is not the ordinal. Admitting it made
   `dst[SubA] = src` over `src[DimB]` with `DimB -> DimA` and `SubA` a
   subdimension of `DimA` read `DimB`'s first element instead of the mapped
   one, which
   `compiler::dimensions::tests::test_implicit_subscript_through_mapped_parent_dimension`
   catches. `axis_match_tests`'s
   `a_mapping_onto_a_parent_is_withheld_from_an_ordinal_resolving_caller` is
   what asserts the projection withholds it, beside
   `a_direct_mapping_still_pairs_for_an_ordinal_resolving_caller` and
   `a_common_mapping_target_still_pairs_for_an_ordinal_resolving_caller`,
   which assert it withholds no more than that.

4. Rungs are newly available where the replaced matcher lacked them: the
   common-mapping-target rung reaches `allocate_implicit_axes*`,
   `compiler::subscript`'s active-dimension resolution and
   `compiler::context`'s `resolve_iteration_element` (entry 9 below), and the
   mapping rungs reach `normalize_subscripts3`'s `IndexExpr3::Expr(Var)` arm
   and `ast::Expr2::unify_dims_with_names`, both of which were name-and-size
   only. In `unify_dims_with_names` this turns a refusal into a compile: with
   `a[X,DimB]`, `b_arr[DimA]` and `DimB -> DimA`, `out[X,DimB] = a[*,*] +
   b_arr[*]` was `MismatchedDimensions` because `DimA` paired with neither of
   `a`'s axes, and now the two operands unify at `[X,DimB]` and each element
   adds the mapped one -- `out[x1,b2]` is `12 + b_arr[a2]`, 212. A widening at
   an axis that was previously UNPAIRED can only turn a refusal into a
   compile; it is where a widening changes which axis is ALREADY paired that a
   number moves, which is entries 6 to 10. Rowed in `axis_match_tests` as
   `two_axes_mapping_onto_one_common_dimension_pair_via_it` and the two
   mapping rows, and pinned end to end by
   `simulate.rs::operand_bounds_unify_through_a_declared_mapping`.

5. `ast::Expr2::find_matching_dimension` tried SIZE before MAPPING, alone among
   the matchers; the one precedence tries mapping first. The two orderings are
   observable only across TARGETS and only through the reverse-mapping arm (a
   mapping is declared on named dimensions, the size rule needs indexed ones),
   so it takes an indexed source axis with an indexed same-length target beside
   a named target that maps onto it. Rowed as
   `a_declared_mapping_outranks_a_size_match_on_another_axis`. The same
   function's per-axis search also had no usage tracking, so
   `unify_dims_with_names` could report that `a[X(3), Y(3)]` matches `b[Z(3)]`
   with both X and Y claiming Z; the one-to-one allocation cannot.
   `compiler::view_contains` gains the same one-to-one property. It is not
   observable there: `named_axes` already declines a view that leaves an axis
   unnamed or names one twice, so both sides carry unique names, and exact-name
   matching over unique names is injective -- the two views also reach the
   identical-shape fast path before either matcher runs.

6. The order the mapping rung's four sub-rules run in changed, and with it
   which target a source axis takes. `allocate_implicit_axes` ran forward and
   forward-to-a-parent across ALL targets and only then reverse; the one
   `mapped` closure takes the first TARGET that any of the four relates the
   source axis to, which is what C, F, H, L and M already did. With `S -> T2`
   and `T1 -> S`, a stock `level[T1,T2]` whose inflow is `flow[S]` paired `S`
   with `T2` (`level[t1,u2]` accumulated `flow[s2]`, 2) and now pairs it with
   `T1` (`level[t1,u2]` accumulates `flow[s1]`, 1). Neither ordering is more
   correct than the other -- the old forward-before-reverse staging was one of
   the orderings being unified, and target order is the one that survived.
   Pinned by
   `simulate.rs::a_source_axis_takes_the_first_target_the_mapping_rung_relates_it_to`
   (a stock's flow wiring is the production caller of that allocation) and
   rowed at the matcher by
   `axis_match_tests::the_mapping_rung_takes_the_first_target_any_sub_rule_relates`.

7. `resolve_iteration_element` pairs the view's axes with the active ones
   POSITIONALLY, where it used a map keyed by dimension NAME. A shape that
   repeats a dimension collapsed onto whichever occurrence was inserted last:
   with `square[D,D]` holding `10i+j`, `o[D,D] = square[*,*]` read the DIAGONAL
   `square[d_j,d_j]`, so `o[d1,d2]` was 22 and is now 12. One source axis with
   two candidate targets breaks its tie toward the FIRST rather than the last,
   so `o_vec[D,D] = vec[*]` reads `vec[d_i]` where it read `vec[d_j]` -- which
   is what the subscript-less spelling `o_vec[D,D] = vec` has always read, so
   the two now agree. Pinned by
   `simulate.rs::a_repeated_active_dimension_pairs_each_occurrence_with_its_own_axis`
   (VM plus `ensure_wasm_matches`), rowed at the matcher by
   `a_repeated_target_dimension_is_two_axes_not_one`.

8. The same pairing is now ONE-TO-ONE, so two view axes cannot read one active
   axis. With `DimB -> DimA` and `DimC -> DimA`, `out[DimA,DimX] = src[*,*]`
   over `src[DimB,DimC]` had both mapped axes take `DimA` and read the diagonal
   `src[b_i,c_i]` (`out[a1,x2]` was 11); now `DimB` takes `DimA`, `DimC` is
   left unpaired and read positionally against `DimX`, and `out[a_i,x_j]` is
   `src[b_i,c_j]` (`out[a1,x2]` is 12). Pinned by
   `simulate.rs::two_mapped_source_axes_cannot_both_read_one_active_axis`.

9. The common-mapping-target rung reaching `resolve_iteration_element` moves
   numbers rather than only widening, because the axis it pairs was being read
   positionally before. With `DimX -> {DimB,DimC}` and `DimY -> {DimA,DimC}`,
   `out[Q,DimY] = src[*,*]` over `src[DimX,Q]` read `src[x_i,q_i]`
   (`out[q1,y2]` was 11) and now reads `src[x_j,q_i]` (`out[q1,y2]` is 21).
   The element step gained the matching translation: where every other rung
   resolves an element through ONE declared mapping, this one runs
   target -> `via` -> source, and the ordinal read the fallback would give is
   that element only while both mappings are positional -- with a permuted
   element map on `DimX -> DimC` the ordinal is a different row (11 where the
   chain gives 31). Which element a pair of dimensions mapping onto a third
   corresponds through is UNVERIFIED against Vensim and Stella: neither
   documents the two-mapping case and no model under `test/` spells it; what
   is not a judgement call is that the reference resolves through `via` if it
   resolves at all, since `via` is the only reason the matcher paired the axes.
   Pinned by `simulate.rs::two_axes_mapping_onto_a_common_dimension_pair_through_it`
   (the positional pair) and
   `a_common_mapping_target_resolves_through_its_element_maps` (the permuted
   one, VM plus `ensure_wasm_matches`).

10. The dynamic-range arm (`data[start:end]` with variable bounds, read per
    element) asks the one precedence which active axis to compare positions
    against, where it compared against axis 0. It is the one caller that
    admits the subdimension rung, and it also gains the size and
    common-mapping-target rules. With `data[I(3)]` and `out[J(4),K(3)] =
    data[lo:hi]`, `I` now pairs with `K` by size: `out[i,j]` is `data[j]`
    while `lo + j - 1 <= hi`, so `out[1,2]` moves from 10 to 20 and the whole
    `J`=4 row moves from NaN to the same three values. Comparing against
    axis 0 read `data[i]` and ran off the end of a three-element source at
    `J`=4. Pinned by
    `simulate.rs::a_dynamic_range_pairs_its_axis_by_size_not_by_position`
    (VM only -- the wasm backend cannot express a runtime view size).

Two further shapes were MEASURED as unchanged rather than argued: the
codegen refusal for an array value in a scalar position now carries one code
and one message for both arms (`NotSimulatable`, "an array of shape .. is used
where a single value is required"; the `StaticSubscript` arm's `Generic`
message, which named the opcode and the missing iteration context instead of
the shape, is gone), which
`per_element_gf_tests::array_valued_table_apply_assigned_to_one_slot_is_refused_not_aborted`
rows for both arms and `db::implicit_diag_tests` reads through the diagnostic
channel; and the Subscript arm's active-dimension lookup iterated a `HashMap`
for its mapping arms, so two active dimensions both matching one source axis
were separated by hash order -- the positional allocation is deterministic.

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
| 5a | `engine: one merger and one slot order for assembly` | 10.264 G (median of 5; range 10.260-10.281), -0.83% against the Phase 2c tree (`ca39c1c6` plus the staged monolith-deletion patch) re-measured in the same session (10.350 G, median of 5, range 10.339-10.353; interleaved pairs -0.85 / -0.82 / -0.66%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`: every count above, the 371 names and 7 modules, the full opcode histogram and the post-fusion stream counts (both `bytecode_profile` blocks byte-identical), and the results-offset map key for key and slot for slot (5058 keys plain, 16073 under LTM); same channel and flags as the baseline row. The saving is deleted assembly work: one `FragmentMerger` per module absorbs each fragment once (`standalone_program` for every initial, `concatenate` for the flows and the stocks, `into_side_channels` for the module's one table set), where assembly ran a GF-dedup pass, a context-aggregation pass and a per-phase resource-count pass over every fragment and a hand-rolled initials renumber beside them; and the fragment map borrows the salsa-cached fragments instead of deep-cloning each into a per-assembly `HashMap`. The results-offset map is `compute_layout` flattened (`db::layout::flattened_offsets`), and `assemble_module` emits each program through one `program_fragments` over every source (explicit, implicit, LTM synthetic, LTM implicit), pinned by `db::assemble_tests`. One corrected shape, not in the corpus: a sub-model holding a lookup-only table shifted every parent variable after the module instance one slot in the results map (Additional Considerations, "Phase 5a semantic divergences") Phase 5's remainder is part (b), the error-channel half (DoD 8); the module-instance input-set owner lands with Phase 3. |
| 3 | `engine: one fragment compiler over dependency shapes` | 8.974 G (median of 5; range 8.971-8.977), -13.3% against the Phase 2c tree re-measured in the same session (10.347 G, median of 5, range 10.337-10.356; interleaved pairs -13.31 / -13.20 / -13.23 / -13.22 / -13.24%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN: every count above, the 371 names and 7 modules, the full opcode histogram and the post-fusion stream counts -- the plain `bytecode_profile` block is byte-identical, and so is the LTM one (30123 slots, 908377 / 1477 / 28514 opcodes, 16741 literals, 441 temps, 2866 views); same channel and flags as the baseline row. The saving is deleted per-fragment work, none of it output-bearing: the explicit emitter lowers only the phases the variable's runlist membership admits (it lowered both phases of every variable and discarded the ungated one -- ~400 initial-phase lowerings on C-LEARN); an implicit helper builds its `FragmentInput` once instead of re-running the parse -> lower -> dependency-walk prologue once per gated phase; a sub-model's shape is one memoized `model_shape` per model instead of a stub symbol table rebuilt, arena and all, inside every fragment that reads the module; and no fragment clones `model_module_map`. One corrected shape, not in the corpus: a stdlib call whose hoisted argument reads a module output compiles and simulates (Additional Considerations, "Phase 3 semantic divergences"). The engine suite (lib 5656, integration 767), the wasm parity corpus, the 12-repeat determinism suites and every fragment/LTM golden are green with no regeneration |
| 4 | `engine: one Variable shape and a borrowed parse input` | 8.799 G (median of 5; range 8.794-8.804), -1.67% against the Phase 3 tree re-measured in the same session (8.949 G, median of 5, range 8.938-8.951; interleaved pairs -1.68 / -1.69 / -1.69 / -1.50 / -1.64%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`: every count above, the 371 names and 7 modules, the full opcode histogram and the post-fusion stream counts -- both `bytecode_profile` blocks are byte-identical; same channel and flags as the baseline row. The saving is deleted per-parse work: `datamodel_variable_from_source` rebuilt and deep-cloned a kind-tagged `datamodel::Variable` (equation, gf, units, flow lists, module references, compat) on every parse of every variable, and `parse_var` then cloned the equation again out of it; the parse now reads a borrowed `VariableSource<'_>` over the salsa inputs. `Variable` is one struct over a `VarKind` enum, so `model::lower_variable` maps over `kind` instead of hand-copying 9-11 fields per variant, and the five repeated fields are stated once. Twins retired: one `paren_if_necessary` over a shallow `NodeShape` classification shared by `print_eqn` and both LaTeX printers, one `render_latex` walker over a `LatexTier` trait replacing `latex_eqn`/`latex_eqn_expr0`/`latex_eqn_expr0_annotated`, one `is_lookup_only(eqn, gf)` replacing `variable::var_is_lookup_only` + `db::source_var_is_table_only`'s body, one `SourceVariableFields::from_datamodel` behind `db/sync.rs`'s fresh and incremental paths, and `NamedDimension::index_of` for callers already holding an `Ident<Canonical>`. One re-assembly of a kind-tagged `datamodel::Variable` survives, outside every parse path: `db::macro_registry::macro_body_variable`, whose consumer `MacroRegistry::build` walks whole `datamodel::Model`s. One corrected shape, not in the corpus: a per-element table holder over a zero-element dimension is lookup-only on the parse side too, so layout and `Var::new` agree (Additional Considerations, "Phase 4 semantic divergences"). Otherwise no semantic divergence: the engine suite (lib 5655, integration 767), the 12-repeat determinism suites and every fragment/LTM golden are green with no regeneration |
| 6a | `engine: one axis matcher and a decomposed subscript arm` | 8.819 G (median of 5; range 8.816-8.829), -1.5% against the seeded tree (Phase 3 staged on `1373f6f3`) re-measured in the same session (8.942 G, median of 5, range 8.939-8.952; interleaved pairs -1.54 / -1.38 / -1.66%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`: every count above, the 371 names and 7 modules, the full opcode histogram and the post-fusion stream counts -- both `bytecode_profile` blocks byte-identical; same channel and flags as the baseline row. The saving is deleted per-reference work, none of it output-bearing: `compiler::subscript` and `lower_from_expr3`'s dimension-as-value arm re-`canonicalize`d (and so re-interned) already-canonical dimension names once per subscript and once per candidate active dimension, and the Subscript arm ran two independent searches over the active dimensions -- one to decide whether an axis matched by name and a second to find which -- where the allocation now answers both once. `dimensions::match_axes` replaces seven matchers (`allocate_implicit_axes_partial` and `allocate_implicit_axes`, `match_dimensions_with_mapping`, `find_dimension_reordering`, `Expr2::can_all_match` and `find_matching_dimension` under `unify_dims_with_names`, `view_contains`/`named_dims`) and five inline searches in `compiler/context.rs` and `compiler/subscript.rs` -- the twelve rows of `axis_match_tests`. `allocate_implicit_axes_partial` and `allocate_implicit_axes` survive as the two projections of it that the LTM augmenter (`ltm_augment_post_transform.rs`) and `get_implicit_subscripts` call, so the matching is what was replaced, not the entry points; the Subscript arm is five named steps; `lower`/`lower_preserving_dimensions` are one function under `DimensionRefs`. Differential sweep of a pre-change and a post-change CLI over all 509 models under `test/`: 397 byte-identical, 110 refused identically, and the only movers were `subscript_transposition` (its transposed `output2` moves from all-NaN to exactly Vensim's `109, 1090, 109, 141, -13`) plus two models that flip between the same two outputs on BOTH binaries (GH #859, an importer nondeterminism this change does not touch). Ten divergences, pinned -- eight shapes whose compile changed and two rungs deliberately withheld (Additional Considerations, "Phase 6a semantic divergences") |
| 5b | `engine: diagnostics keep their message from parse to collection` | 8.829 G (median of 5; range 8.827-8.841), +0.36% against the Phase 4 tree re-measured in the same session (8.798 G, median of 5, range 8.791-8.802; interleaved pairs +0.44 / +0.32 / +0.28 / +0.49 / +0.38%) | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`: both `bytecode_profile` blocks are byte-identical, including the full opcode histogram, the post-fusion stream counts, the 371 names and 7 modules; same channel and flags as the baseline row. This phase changes diagnostics, not codegen, so the delta is cost rather than saving: `EquationError` grew an `Option<String>` from 8 to 32 bytes, and it is the error type of every `EquationResult` in the AST-lowering path and the element type of `Variable::errors`, which is cloned per parse. Under the one-percent bar the plan sets for recording rather than investigating; a perf pass follows this branch. Diagnostics on the `test/` corpus: the same 501 rows with the same per-code distribution, of which 409 now carry a payload where 209 did (`unknown_builtin` 0 -> 96, `unknown_dependency` 0 -> 48, `mismatched_dimensions` 0 -> 14, `generic` 0 -> 10, `bad_builtin_args` 0 -> 6, `array_reference_needs_explicit_subscripts` 0 -> 5, `empty_equation` 0 -> 18 of 58, `bad_binary_op_in_units` 0 -> 3). 372 of the 409 are a SENTENCE; the other 37 are a bare identifier, which is what their raising site had -- `empty_equation` (18), `mismatched_dimensions` (14) and `array_reference_needs_explicit_subscripts` (5), all raised in `compiler/mod.rs` and `compiler/context.rs`, which Phase 6(b) rewrites and which the sentences should follow. The 92 that remain payload-less are the parse stage (45), whose reason is the source snippet every Rust surface now renders from the span, plus three sites in files Phase 6a/6b own (43) and one (`db/dep_graph.rs`'s `cycle_diagnostic`, 4) that has no reason in hand. One deliberately re-baselined pin and three named divergences (Additional Considerations, "Phase 5b semantic divergences"), which also record the two places a reason still stops: the web app's equation arm (GH #1030) and the bare-identifier payloads above. The CLI renders through `collect_formatted_errors`, so the parse row's "the snippet IS the reason" rule now holds on every Rust surface -- libsimlin, both MCP servers and the CLI. The engine suite (lib 5661, integration 767), the CLI suite (9), the 12-repeat determinism suites, every fragment/LTM golden with no regeneration, and one capped `cargo test --workspace` are green; `cbindgen` reproduces `simlin.h` unchanged |
| 7.1 | `engine: one predicate for a direct PREVIOUS read` | 8.689 G (median of 5; range 8.685-8.698), -0.01% against the seeded tree (`6cf3660b`) re-measured in the same session (8.690 G, median of 5, range 8.679-8.692; interleaved pairs +0.10 / -0.03 / +0.07 / -0.05 / +0.02%), inside the channel's noise floor and not investigated | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`: the whole `bytecode_profile` block is byte-identical in both modes -- every count above, the 371 names and 7 modules, the full opcode histogram, the post-fusion stream counts and the fused-binop table; same channel and flags as the baseline row. A refactor plus a probe, so no saving is expected or found. `snapshot_arg::SnapshotArg::access` is now the one statement of what `PREVIOUS`/`INIT` addresses directly, called by `BuiltinVisitor::snapshot_arg` over the source argument (replacing `needs_temp_arg` and `arg_is_array_shaped`) and by `codegen::lowered_snapshot_arg` over the lowered one (feeding `static_slot` and `Compiler::snapshot_static_view`); `db::exec_probe::ProbedDb` counts every tracked query's executions from salsa's own events. Differential sweep of a base-tree and a working-tree CLI over all 509 models under `test/`, each run twice per binary: 396 byte-identical, 110 refused identically, and the only three models whose output is not identical are `subscript_transposition`, `arrays_cname` and `arrays_varname`, each of which flips between exactly the same TWO outputs on BOTH binaries (GH #859, an importer nondeterminism; resampled 12x per binary per model, which is what separates a flip from a move -- two samples per binary does not). No model moved. Four parse-vs-codegen divergences recorded, none of them introduced here and none changing an artifact (Additional Considerations, "Phase 7.1 predicate"); the probe's findings, including that `ModuleIdentContext` is the sole remaining cause of AC3.1's loose case, are under "Phase 7.1 probe" |
| 7.2 | `engine: captures carry their argument, not its text` | 8.6920 G (median of 5; range 8.6866-8.6976), -0.02% against the base tree (the 7.1 chunk staged on `68774a16`) re-measured in the same session (8.6937 G, median of 5, range 8.6834-8.6979; interleaved pairs -0.017 / +0.163 / -0.047 / -0.038 / -0.017%), inside the channel's noise floor and not investigated | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`: the whole `bytecode_profile` block is byte-identical in both modes -- every count above, the 371 names and 7 modules, the full opcode histogram, the post-fusion stream counts and the fused-binop table; same channel and flags as the baseline row. A representation change with the observable result held fixed, so no saving is expected and none is found. The exact per-capture delta at each of the six consumers that build a helper's parse-stage form is: a lex-and-parse of the helper's equation text is deleted, and one `print_eqn` (the `Variable::eqn` field is source text by definition) plus one `Expr0` subtree clone replaces it. The `instantiate_implicit_modules` walk is NOT part of the delta -- `parse_var` ran it on the old path too, and `Capture::variable_stage0` runs it for the same reason. On a model with 233 captures among 5215 slots that trade is a wash. `PREVIOUS`/`INIT` arguments are now `capture::Capture` values -- an `Expr0` subtree with positional identity `(parent, id)` -- carried on the parse result in a `capture::ImplicitVar` list beside the module instances and hoisted call arguments that are still text; `Capture::variable_stage0` is the one constructor of a capture's parse-stage variable, and `capture::synthetic_ident` the one derivation of every synthesized helper's name (`rg "arg0" src/simlin-engine/src` finds one production site). `ImplicitVar::Synthesized` is boxed, which is what keeps the enum the size of a capture rather than of a `datamodel::Variable` in a list salsa retains per variable and, under LTM, per synthetic variable. Differential sweep of the base-tree and working-tree CLIs over all 509 models under `test/`, each run twice per binary: 396 byte-identical, 110 refused identically, and the only three non-identical models are `arrays_cname`, `arrays_varname` and `subscript_transposition`, each resampled 12x per binary and each producing the SAME two-output set on both binaries (GH #859, the importer nondeterminism). No model moved. The engine suite (lib 5677, integration 776, CLI 4, wasm 2), the 12-repeat determinism suites and every fragment/LTM golden are green with no regeneration |
| 6b | `engine: one materialization pass over the lowered fragment` | 8.710 G (median of 5; range 8.708-8.716), +0.15% against the base `5c406dd5` re-measured in the same session (8.696 G, median of 5, range 8.687-8.709; interleaved pairs +0.14 / +0.28 / +0.01 / +0.08 / +0.24%), inside the channel's floor and not investigated | 5215 | 30682 / 1477 / 24658 | 1732 / 162 / 28 / 641 | The artifact moves, and the direction is smaller. Plain: 12 fewer flow opcodes and 2 fewer static views, all from ONE variable (`rs_ff_co2_ff_aggregated`, an element-invariant per-element arrayed-GF apply evaluated once instead of three times: `LookupArray` 12 -> 10, its `PushStaticView`/`PopView` pairs, and the `LoadVar`/`Op2`/`LoadGlobalVar` of the two dropped applies); `VectorElmMap` 8 -> 8, `VectorSortOrder` 2 -> 2, `BeginIter` 99 -> 99, 28 temp slots either side. Under `CLEARN_LTM=1`: 907851 / 1477 / 28514 opcodes (-526 flow), 16741 literals, 162 GFs, **28 temp slots against 441**, 2682 static views (-184), `VectorElmMap` 92 -> 12, `VectorSortOrder` 23 -> 3, `LookupArray` 29 -> 25, `BeginIter` 415 -> 415 -- a body one element reads is evaluated on an id the elements reissue rather than one id per element, and an element-invariant one is evaluated once per link-score fragment rather than once per element. The LTM lowering scope is empty (no scoped re-lower): that LTM block is byte-identical with the re-lower and without it, every LTM golden is unchanged, and the LTM compile channel (`CLEARN_LTM=1 CLEARN_PROFILE=compile CLEARN_COMPILE_ITERS=2`, three interleaved pairs) is 58.653 G against the base's 61.369 G, **-4.42%** (pairs -4.56 / -4.38 / -4.45%). The plain compile channel's cost is the subscript route (every subscript naming a dimension goes through `normalize_subscripts3` and `resolve_mapped_read` where Pass 1 folded an active one to an ordinal, divergence item 2); an exact-name fast path in `active_dim_ref` keeps it inside the floor. The run channel (`CLEARN_PROFILE=run CLEARN_RUN_ITERS=20`, same binaries) is 30.660 G against 30.776 G, -0.38% (pairs -0.37 / -0.39 / -0.38 / -0.39 / -0.37%). Differential sweep of the base and tree CLIs over all 509 models under `test/`: 396 byte-identical, 110 refused identically, 0 refused on one side only, and three not byte-identical: `subrange_merge` is deterministic on both and moves onto its checked-in Vensim `output.tab` (item 2); `arrays_cname` and `arrays_varname` have two order-free forms on the base and one -- one of the base's two -- on the tree (item 2: a subscript naming a dimension is read by element NAME, so the importer's permuted declaration order stops changing which element is read); `subscript_transposition` keeps the same two order-free forms on both binaries (GH #859, resampled 12x per binary). Nine divergences, pinned (Additional Considerations, "Phase 6b semantic divergences"); the engine suite (lib 5669, integration 782), the wasm parity corpus and the 12-repeat determinism suites are green with no golden regeneration |
| 7.4 | `engine: parse without model context; sub-model initials` | 8.3650 G (median of 5; range 8.3621-8.3749), **-3.72%** against the base `c8770abb` re-measured in the same session (8.6879 G, median of 5, range 8.6855-8.6943; interleaved pairs -3.72 / -3.63 / -3.73 / -3.79 / -3.64%) | 5215 | 30682 / 1477 / 24873 | 1750 / 162 / 28 / 641 | Plain: flow and stock streams byte-identical, initials +215 opcodes and +31 programs (1174 -> 1205), +18 literals, 371 names and 7 modules unchanged: every value-bearing variable of an instantiated model is now an initials member (GH #1028). Under `CLEARN_LTM=1`: 30123 slots and 2682 views unchanged, 907854 / 1477 / 28733 opcodes (+3 flow, +219 initial, 1976 initials), 16761 literals; the one delta beyond the #1028 members is `stdlib⁚delay3`'s LTM helper `$⁚$⁚ltm⁚link_score⁚input→stock⁚1⁚arg0` under the instance wiring `{delay_time, input}` (4 initial opcodes with its `Ret`, 3 flow), whose body snapshots the bound port `input`: the base could not lower the port (`Expr::ModuleInput` has no slot) and the by-presence LTM tail dropped the helper with no diagnostic, so every DELAY3 `input -> stock` link score read an unwritten 0 for its two-step lag -- a silent wrong number (`4, 10, 24` for a unit ramp where the formula gives `1, 2, 4`), corrected by lowering the port to its own slot and pinned under "Phase 7.4 semantic divergences" item 3; on C-LEARN that instance's score happens to be byte-identical, so all 30123 LTM series are identical between the two binaries. The run channel (`CLEARN_PROFILE=run CLEARN_RUN_ITERS=20`) is 30.5414 G against 30.6572 G, -0.38% (pairs -0.36 / -0.41 / -0.37 / -0.38 / -0.38%). The compile saving is deleted work: the per-model module-ident pre-scan that re-parsed every Aux/Flow equation once per context, and the second and third parses of every variable under the empty, per-instance-widened and stdlib-extended contexts. Differential sweep of the base and tree CLIs over all 509 models under `test/`: 396 byte-identical, 110 refused identically, 0 refused on one side only, 3 not identical: one GH #859 importer flipper (`subscript_transposition` or `arrays_varname`, whichever the sample lands on; each flips between the same two outputs on both binaries, resampled 6x per binary), and two GH #1028 movers -- `land_model.stmx`'s `real_gdp_growth_rate = TREND(..)` (TREND's `output` is an aux) now matches the checked-in Stella `output.tab` exactly (0.04, 0.0292888201074, 0.0200569598033 where the base printed 0, 6.957, 3.233), and `bobby/vdf/econ/mark2.mdl`'s `perceived mortgage balance = SMOOTH(interest earned - investments lost)` reads `defaults = DELAY1(..)` (DELAY1's `output` is a flow) during the parent's initials, so the smooth starts at its t0 input as Vensim's SMOOTH does (the base started 150,000,000 above it, with a nonzero t0 flow). Four divergences and one fix pinned (Additional Considerations, "Phase 7.4 semantic divergences"); the engine suite, libsimlin, the CLI, clippy and `cargo fmt --all -- --check` are green, with one golden regenerated (`modules.txt`, the two `producer` initial fragments) |
| V-opc | `engine: derive opcode operand handling from one table` | 8.6666 G (median of 5; range 8.6581-8.6701), -0.27% against `c8770abb` re-measured in the same session (8.6902 G, median of 5, range 8.6871-8.6934; interleaved pairs -0.24 / -0.27 / -0.35 / -0.37 / -0.26%), measured on the pre-review candidate (the fix round changed only which operands the accessors bind); recorded, not investigated (under the one-percent bar; nothing output-bearing changed) | 5215 | 30682 / 1477 / 24873 | 1750 / 162 / 28 / 641 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`, against `c972c9e9` (57032 opcodes plain, 938064 under LTM -- Phase 7.4's artifact): both `bytecode_profile` blocks are byte-identical; the run channel is -0.03% (inside the floor) and the sweep over all 509 models under `test/` has no mover (`arrays_varname` flips between the same two outputs on both binaries, GH #859), both measured on the pre-review candidate against `c8770abb`. `SymbolicOpcode` and `Opcode` are each one table (`symbolic_opcode_table!`, `opcode_table!`) from which `resolve_opcode`, `renumber_opcode`, `gf_run`, `var_ref`, `jump_offset`, `stack_effect` and `name` derive by operand kind, the accessors binding only the operand their kind reads (`bind_kind!`), with the every-row tests and the merge proptest's blanking oracle derived from the same rows; production -473 lines by file length (bytecode.rs -282, symbolic.rs -191), two `unused_variables` allows in production, no semantic divergence (Additional Considerations, "Opcode operand tables"). |
| 7.5ab | `engine: static element snapshots; capture phase demand` | 8.2010 G (median of 5; range 8.1969-8.2084), **-1.55%** against `015c98da` with the results-printing commit applied, re-measured in the same session (8.3302 G, median of 5, range 8.3266-8.3335; interleaved pairs -1.51 / -1.42 / -1.60 / -1.63 / -1.53%) | 5189 | 28505 / 1477 / 24795 | 1533 / 162 / 28 / 627 | Plain: -26 slots (the bare and qualified element captures 7.5a no longer mints, `init_c_in_deep_ocean_per_meter` and `target_year`), -2177 flow opcodes (those captures' fragments plus the flow fragments of the 207 INIT-only captures 7.5b takes out of flows), -78 initial opcodes and 1205 -> 1179 initial programs (one per removed capture), -217 literals, -14 views, 371 names and 7 modules unchanged; 4825 results keys where the base had 5058 (26 gone, 207 hidden, 0 renamed -- a removed capture renumbers the later helpers of the same equation, `$⁚v⁚1⁚..` -> `$⁚v⁚0⁚..`, which renames an exposed key only where a capture precedes another helper of the same equation, and no C-LEARN equation has that shape). Under `CLEARN_LTM=1`: 30123 -> 29398 slots and 7163 -> 6193 LTM variables (`ltm_var_dump`: 987 removed, 17 added -- the 26 element captures' 52 scalar scores become 17 direct scores arrayed over their targets, 330 slots; the 207 INIT-only captures' 921 scalar scores, 207 capture->parent and 714 source->capture, and the 14 two-slot array-freeze helpers of the scores into `$⁚last_set_target_year⁚0⁚arg0` do not exist because the edges do not, and 28 `PREVIOUS` helpers of the removed scores go with them; 36,138 slots free of the ceiling), 855713 / 1477 / 24795 opcodes (-52141 flow, -3938 initial, 1976 -> 1179 initial programs: the LTM PREVIOUS helpers' initial fragments), 14503 literals, 2166 views; the all-slot digest 20892 -> 20221 LTM slots and 3141 -> 3106 ever non-zero (the 21 removed scores the base held at 1 and the 14 freeze slots), `clearn_with_ltm_simulates_model_vars_identically` green. Run channel (`CLEARN_PROFILE=run CLEARN_RUN_ITERS=20`) 29.5656 G against 30.5294 G, -3.16% (pairs -3.17 / -3.15 / -3.16 / -3.18 / -3.14%); the plain artifact is byte-identical to the pre-review candidate's, which measured -1.19% against `c972c9e9`, so about two points of this are build layout. Sweep of 509 models, plain and `--ltm`: 394 / 393 identical, 110 refused identically, 0 one-sided; the movers are the GH #859 importer flippers (`subscript_transposition` on both channels, `arrays_varname` under `--ltm`; resampled 6x per binary, the same two digests on both) and, on both channels, `getdata`, `helper_recurrence`, `macro_init_recurrence` and C-LEARN, whose only difference is the hidden INIT-only capture columns and, under `--ltm`, the removed scores: every common column byte-identical (C-LEARN: 4825 plain, 14825 under `--ltm`; 1248 base-only keys = 26 + 207 + 987 + 28, 17 tree-only). Six divergences pinned ("Phase 7.5 semantic divergences"); three goldens regenerated; engine suite, libsimlin, CLI, clippy and `cargo fmt --all -- --check` green |
| 7.5cd | `engine: element-scoped helpers; structural captures` | 7.5449 G (median of 3; range 7.5440-7.5478), **-8.05%** against `11de2948` re-measured in the same session (8.2050 G, median of 3, range 8.1978-8.2051; interleaved pairs -7.98 / -8.05 / -8.01%) | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | Plain: flow and stock streams byte-identical, -126 initial opcodes and 1179 -> 1053 initial programs, -165 literals, 371 names and 7 modules unchanged, all 4825 results keys and values byte-identical (24 apply-to-all `INIT` parents' 150 per-element captures are 24 structural captures over the same 150 slots); under `CLEARN_LTM=1` 29398 slots, 855713 / 1477 / 24669 opcodes (-126 initial), 14503 -> 14078 literals, 2166 views, 6193 LTM variables, 14842 results keys of which 14212 are byte-identical and 630 per-element helper keys are renamed (`⁚elem` -> `[elem]`); LTM compile channel 50.8174 G against 54.3852 G, **-6.56%** (pairs -6.58 / -6.58 / -6.48%), run channel 29.0291 G against 29.5660 G, -1.82% (pairs -1.82 / -1.81 / -1.82%). The saving is deleted work: one structural walk of a snapshot-only apply-to-all body instead of N per-element walks, parse-stage variables and fragments, no parse-time dimension substitution of hoisted arguments, and no duplicated input hoist for `DELAYN`/`SMTHN`. Sweep of 509 models twice per binary: plain 398 identical, 110 refused identically, 0 moved; `--ltm` 386 identical, 110 refused identically, 10 movers that are all key renames (0 differing common columns each, the same key counts, C-LEARN included), the GH #859 flippers on the same two digests on both binaries in both modes (6x per binary), seven divergences pinned (7-13 under "Phase 7.5 semantic divergences"), one golden regenerated (`ltm_value_golden/value_gate.txt`, two phantom loop slots), engine suite (lib 5714, integration 789 plus the C-LEARN release gates), clippy, `cargo fmt --all -- --check` and the default-feature check green, with `mdl_equivalence::test_mdl_equivalence` and `test_clearn_equivalence` failing identically on the base tree (xmutil view element counts) |
| 8.1 | `engine: lower Expr2 under the fragment's dependency shapes` | 7.2241 G (median of 3; range 7.2234-7.2271), **-4.32%** against `4f3bf7db` re-measured in the same session (7.5501 G, median of 3, range 7.5461-7.5525; interleaved pairs -4.23 / -4.33 / -4.35%) | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | Artifacts byte-identical on C-LEARN, plain (every count, 1053 initial programs, 371 names, 7 modules, the full opcode histogram) and under `CLEARN_LTM=1` (29398 slots, 855713 / 1477 / 24669 opcodes, 14078 literals, 2166 views; LTM compile channel 82.8363 G against 83.3488 G, -0.62%, pairs -0.52 / -0.60 / -0.67%, `CLEARN_COMPILE_ITERS=5`). The saving is the deleted per-variable mini-stage: every fragment cloned its dependencies' parse memos into a `ModelStage0` literal to answer `get_dimensions`, which the shape map it already built answers. Sweep of 509 models plain and `--ltm`: 399 / 398 identical, 110 refused identically, 0 one-sided, the one `--ltm` mover the GH #859 flipper `arrays_cname` (the same two digests on both binaries in both modes, 6x each); the `test/` diagnostics corpus identical before and after (plain 471 rows over 366 `(model, variable, code)` keys, `--ltm` 856 over 391, the same per-code distribution, 0 row differences); one mechanism-level divergence -- a helper lowers under its parent's shapes -- pinned by a 48-row value table (helper kind x reducer x rank) and a 5-row refusal table ("Phase 8.1 semantic divergences"); engine suite (lib 5693, integration 783), libsimlin, CLI, mcp-core, clippy, `cargo fmt --all -- --check` and the default-feature check green, every golden unregenerated |
| 8.2+8.3 | `engine: one lowered memo per variable` | 7.2435 G (median of 3; range 7.2367-7.2437), +0.23% against `75ee055a` re-measured in the same session (7.2268 G, median of 3, range 7.2260-7.2295; interleaved pairs +0.14 / +0.25 / +0.19%), inside the channel's floor and not investigated | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | Artifacts byte-identical on C-LEARN, plain (every count, 1053 initial programs, 371 names, 7 modules, the full opcode histogram) and under `CLEARN_LTM=1` (29398 slots, 855713 / 1477 / 24669 opcodes, 14078 literals, 2166 views); LTM compile channel 80.2377 G against 82.8212 G, **-3.12%** (pairs -3.13 / -3.12 / -3.16%, `CLEARN_COMPILE_ITERS=5`); memory (counting allocator, C-LEARN, bytes the database and sync state retain above the parsed datamodel; peak = compile phase): plain compile-only 22.94 -> 32.16 MiB (peak 30.2 -> 39.3), plain with diagnostics 36.58 -> 33.54 (peak 43.9 -> 40.8), LTM 228.26 -> 225.98 (peak 270.0 -> 267.5), LTM with diagnostics 242.34 -> 227.02 (peak 284.1 -> 268.9); allocations plain 1,562,107 / 199.2 MiB -> 1,541,101 / 193.7 MiB, LTM 26.06 M / 2859.0 MiB -> 24.90 M / 2583.6 MiB; sweep of 509 models plain and `--ltm` 398 / 398 identical, 110 refused identically, 0 one-sided, the movers GH #859 flippers (`arrays_varname`, `arrays_cname`, `test_subscript_transposition`: the same two digests on both binaries in both modes, 6x each), 8 `--ltm` stderr line-order permutations (GH #1036); `test/` diagnostics corpus identical (plain 471 rows over 366 keys, `--ltm` 856 over 391). The +9.2 MiB is the lowered trees a compile-only caller retains for a unit pass or describer it never runs (pysimlin `Model.simulate()` used alone, a C/Go embedder holding a project without `get_errors`, serve's transient `simulate_sync` as peak only), while every path that collects diagnostics, the CLI's `simulate` included, retains less than the base. A per-element helper with a module head is pinned only under the module-cycle gate (`units_tests::a_module_cycle_reached_through_a_per_element_helper_still_unit_checks`: without it the unit pass on a cyclic project is salsa's `compute_layout` cycle panic), and the rename patch is syntactic over the equation text ("Phase 8.2 semantic divergences"). Engine suite (lib 5697, integration 783), libsimlin, CLI, mcp-core, clippy, `cargo fmt --all -- --check` and the default-feature check green, every golden unregenerated |
| 8.3b | `engine: box the Expr2 node's array bounds` | 7.1283 G (median of 3; range 7.1281-7.1303), **-1.01%** against `a17e8027` re-measured in the same session (7.2008 G, median of 3, range 7.2004-7.2008; interleaved pairs -0.98 / -1.01 / -1.01%) | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | Artifacts byte-identical on C-LEARN, plain (every count, 1053 initial programs, 371 names, 7 modules, the full opcode histogram) and under `CLEARN_LTM=1` (29398 slots, 855713 / 1477 / 24669 opcodes, 14078 literals, 2166 views; LTM compile channel 78.9676 G against 79.8251 G, **-1.07%**, pairs -1.10 / -1.07 / -1.12%, `CLEARN_COMPILE_ITERS=5`); `size_of::<Expr2>()` 128 -> 64 (the bounds slot 72 -> 8; `Expr3` 128 -> 64, `IndexExpr2` 264 -> 136, `variable::Variable` 680 -> 552); memory (counting allocator, C-LEARN, bytes the database and sync state retain above the parsed datamodel; peak = compile phase): plain compile-only 32.16 -> 29.49 MiB (peak 39.3 -> 36.7), plain with diagnostics 33.54 -> 30.73 (peak 40.8 -> 38.0), LTM 225.97 -> 223.12 (peak 267.4 -> 264.7), LTM with diagnostics 227.02 -> 224.20 (peak 268.9 -> 266.3); allocations plain 1,540,711 / 193.7 MiB -> 1,542,030 / 171.3 MiB, LTM 24,898,752 / 2583.6 MiB -> 24,905,400 / 2277.1 MiB; sweep of 509 models plain and `--ltm` 399 / 397 identical, 110 refused identically, 0 one-sided, the two `--ltm` movers GH #859 flippers (`arrays_varname`, `test_subscript_transposition`: the same two digests on both binaries in both modes, 6x each), 7 `--ltm` stderr line-order permutations (GH #1036); `test/` diagnostics corpus identical (plain 471 rows over 366 keys, `--ltm` 856 over 391). A representation change with the observable held fixed: the -2.7 MiB on every row is the retained `Expr2` trees at half a node, under a third of the +9.2 MiB the memos cost a compile-only caller (the `Variable` beside each tree, the heads and the handle map are the rest), so that row stays above the pre-memo base (22.94 MiB). The instruction saving is the node copies (every construction, clone and move of a node moves half the bytes), and the +0.1% / +0.03% allocations are one box per bound produced. Engine suite (lib 5699, integration 783), libsimlin, CLI, mcp-core, clippy, `cargo fmt --all -- --check` and the default-feature check green, every golden unregenerated |
| 8.5 | `engine: one dependency representation, classified once` | 6.9918 G (median of 3; range 6.9895-6.9932), **-1.84%** against `9e6253cd` re-measured in the same session (7.1229 G, median of 3, range 7.1224-7.1313; interleaved pairs -1.96 / -1.87 / -1.81%) | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | Artifacts byte-identical on C-LEARN, plain (every count, 1053 initial programs, 371 names, 7 modules, the full opcode histogram) and under `CLEARN_LTM=1` (29398 slots, 855713 / 1477 / 24669 opcodes, 14078 literals, 2166 views; LTM compile channel 77.7223 G against 79.0077 G, **-1.63%**, pairs -1.59 / -1.69 / -1.63%, `CLEARN_COMPILE_ITERS=5`); memory (counting allocator, C-LEARN, bytes the database and sync state retain above the parsed datamodel; peak = compile phase): plain compile-only 29.49 -> 28.97 MiB (peak 36.7 -> 36.2), plain with diagnostics 30.73 -> 30.18 (peak 38.0 -> 37.5), LTM 223.17 -> 222.52 (peak 264.9 -> 264.0), LTM with diagnostics 224.28 -> 223.55 (peak 266.1 -> 265.8); allocations plain 1,542,125 / 171.3 MiB -> 1,466,767 / 163.5 MiB, LTM 24,905,822 / 2277.5 MiB -> 24,226,276 / 2241.2 MiB; sweep of 509 models plain and `--ltm` 397 / 398 identical, 110 refused identically, 0 one-sided, the movers GH #859 flippers (the same two digests on both binaries in both modes, 6x each), 8 `--ltm` stderr line-order permutations (GH #1036); `test/` diagnostics corpus plain 471 -> 472 rows over 366 -> 367 keys, `--ltm` 856 -> 857 over 391 -> 392, the one added row divergence 4. The saving is deleted work: one classification per variable and helper over its `Expr1`, where the base lowered every one to `Expr2` a second time under an empty scope for its dependencies, and no `·` re-splitting at the consumers. Seven divergences pinned under "Phase 8.5 semantic divergences", none with a corpus model of its shape but divergence 4 (`sir_social_distancing_mixnot.stmx`, refused on both binaries): the nested-stock ordering (5) moves numbers on a model the base ran, from the #591-c1 stale-input class to the one-hop rule's, and the output-port scan (6) adds LTM series. Engine suite (lib 5686, integration 783), libsimlin (244), CLI, mcp-core, clippy, `cargo fmt --all -- --check` and the default-feature check green, every golden unregenerated |
| V9a | `engine: LTM reads typed values; one module-instance owner` | 6.9798 G (median of 3; range 6.9797-6.9883), -0.44% against `bee455c4` re-measured in the same session (7.0107 G, median of 3, range 7.0061-7.0132; interleaved pairs -0.44 / -0.25 / -0.48%); LTM compile channel (`CLEARN_LTM=1 CLEARN_COMPILE_ITERS=2`) 49.1146 G against 48.6483 G, +0.96% (pairs +0.84 / +0.76 / +0.99%), recorded | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | C-LEARN artifacts identical plain and under `CLEARN_LTM=1` (29398 slots, 855713 / 1477 / 24669 opcodes, 14078 literals, 2166 views); one natural LTM shape moves, pinned (V9a-1, the frozen-whole reducer over a bare arrayed argument), and a base VM abort compiles (V9a-2). Sweeps plain 398 identical / 110 refused / 0 one-sided / 1 mover and `--ltm` 397 / 110 / 0 / 2, every mover a GH #859 flipper, every stderr difference a GH #1036 order-only permutation; corpus-wide LTM variable sets and detected loops identical. The LTM channel's +1% is the `Expr2` tier's array bounds on ~7k generated equations (+2.9 points, measured with a bounds-free control) against the parses the text boundary no longer pays (-2.1 points); typing each generated equation once (`lower_variable_from_typed`) is what keeps it there rather than at +4.6%. Rust non-test -175 lines. |
| 9 | `engine: one diagnostic payload from site to collection` | 7.0152 G (median of 3; range 7.0119-7.0152), +0.38% against `baeac250` re-measured in the same session (6.9890 G, median of 3, range 6.9825-6.9902; interleaved pairs +0.33 / +0.47 / +0.36%), inside the channel's floor and not investigated | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | Artifacts byte-identical on C-LEARN, plain (every count, 1053 initial programs, 371 names, 7 modules, the full opcode histogram) and under `CLEARN_LTM=1` (29398 slots, 855713 / 1477 / 24669 opcodes, 14078 literals, 2166 views); LTM compile channel 78.6850 G against 77.7404 G, +1.22% (pairs +1.18 / +1.32 / +1.13%, `CLEARN_COMPILE_ITERS=5`), with compile-phase allocations plain 1,466,823 / 163.9 MiB -> 1,466,145 / 163.5 MiB and LTM 24,226,667 / 2242.7 MiB -> 24,224,380 / 2241.5 MiB and a symbol-level profile whose whole delta sits in untouched lowering and lexing functions (the code this phase touches is under 0.01% of samples), so it is recorded as build-layout perturbation; sweep of 509 models plain and `--ltm` 396 / 398 identical, 110 refused identically, 0 one-sided, the movers the GH #859 flippers `arrays_cname`, `arrays_varname` and `subscript_transposition` (each the same two digests on both binaries in both modes, 6x each), one plain stderr line-order permutation (`RealBeer4-Sterman13.mdl`, its cycle row after the variable rows, divergence 6) and 8 `--ltm` (GH #1036, divergence 7); `test/` diagnostics corpus plain 472 rows over 367 keys and `--ltm` 857 over 392 on both binaries, the same per-code distribution but the 33 umbrella rows (divergence 1), which move from the `Model` arm's rendering to the inference arm's. A diagnostic is one typed payload from its raising site to `collect_all_diagnostics`, context attached once by type, recursive queries returning facts and the per-model owner emitting them exactly once (`db::diagnostic_payload_tests`: the producer x category x severity matrix, the once-across-revisions matrix over every warning family, one-variable invalidation under `ProbedDb`). Engine suite (lib 5693, integration 783), libsimlin (245), CLI, mcp-core, clippy, `cargo fmt --all -- --check` and the default-feature check green, every golden unregenerated, `simlin.h` byte-identical under cbindgen |
| V9b | `engine: ltm describes the executed read` | 6.9521 G (median of 3; range 6.9501-6.9593), -0.41% against `615526fe` re-measured in the same session (6.9808 G, median of 3, range 6.9805-6.9854; interleaved pairs -0.44 / -0.48 / -0.30%), on a channel this chunk does not touch (no plain-compile code changes; recorded as build-layout perturbation) | 5189 | 28505 / 1477 / 24669 | 1368 / 162 / 28 / 627 | Artifacts byte-identical on C-LEARN, plain (every count, the full opcode histogram) and under `CLEARN_LTM=1` (29398 slots, 855713 / 1477 / 24669 opcodes, 14078 literals, 2166 views, the 6193-variable LTM set); LTM compile channel 78.5986 G against 79.5227 G, -1.16% (pairs -1.18 / -1.17 / -1.16%, `CLEARN_COMPILE_ITERS=5`); sweep of 509 models plain 397 identical / 110 refused identically / 0 one-sided, the movers GH #859 flippers (`arrays_cname` and `arrays_varname` `.mdl` this run, `subscript_transposition` in the previous; base-vs-base differs run to run), no stderr change; `--ltm` 396 identical / 110 refused / 3 moved (the same three flippers), nine stderr differences of which seven are line-order permutations (sorted-identical: `sir_social_distancing_mixnot`, `critical-slowing`, `PinkNoise2010`, `modules_hares_and_foxes`, `hares_and_lynxes_modules` .stmx/.xmile, C-LEARN; the warning order is not deterministic run to run on either binary), one FREE6 (V9b-3: six pair-level and per-element declines gone, nothing added) and one `covid19_severity.stmx` (refused on both binaries, `conveyor_driven_flow_read`; the arrayed outflow of its scalar stock is a shape the compiler refuses to integrate, and the flow-to-stock fragment for it fails to compile on both -- its two helpers on the base, the fragment as a whole on the tree -- each warned); corpus-wide LTM dump in both modes 501 of 509 identical this run (499 in the previous; the difference is the two flippers `arrays_varname.mdl` and `subscript_switching.mdl`), the movers named in the V9b report (FREE6 +12, the subrange fixtures +2/+3 -- V9b-3 shapes -- `subscript_switching.xmile` +5, `arrays_cname.mdl`, on which the importer assigns `inputAB` to `DimA` or to the same-named `DimX` run to run, and `covid19_severity.stmx` -1, whose base discovery-mode direct score beside a hoisted `SUM(total_infected)` was a fragment that failed to compile). Engine suite (lib 5702, integration 793), libsimlin (245), CLI, mcp-core, clippy, `cargo fmt --check`, the default-feature check and the debug-build IR dump over 97 probe models green; every golden unregenerated |
