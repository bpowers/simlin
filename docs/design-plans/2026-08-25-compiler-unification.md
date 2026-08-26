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
    pub module_idents: Option<&'a HashSet<Ident<Canonical>>>,
    pub model_var_names: Option<&'a HashSet<Ident<Canonical>>>,
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

Part (a) is done: `dimensions::match_axes` is the one precedence, the
Subscript arm is five named steps, `lower`/`lower_preserving_dimensions` are
one function under `DimensionRefs`, and GH #1027 is fixed. Part (b) -- the
single materialization pass and the deletion of Pass 1 hoisting -- is what
remains, and inherits one lowering entry point with an explicit mode.

One shape is waiting for it. Resolving a `Dimension(d)` subscript picks the
first active axis named `d` for EVERY occurrence, so with `square[D,D]` holding
`10i+j` both `o[D,D] = square` and `o[D,D] = square[D,D]` read the diagonal
`square[d_i,d_i]` -- measured identical on the pre-change and the post-change
CLI, so it is pre-existing and outside 6(a), but 6(a)'s corrected
`square[*,*]` (divergence 7) now disagrees with both. The single
materialization pass has to resolve each `Dimension(d)` occurrence against its
own active axis, which is the same positional property `match_axes_partial`
already states.
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
<!-- END_PHASE_8 -->

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
- D1 and D3 move to lowering, decided by ONE predicate -- "does the argument
  lower to a `static_slot`/`snapshot_static_view`" -- computed once from the
  capture's lowered form and exposed as a per-variable projection that BOTH
  the dependency stage and `lower_fragment` consume, so the graph and the
  bytecode cannot drift (the GH #568 class).
- Four module-ident contexts are live today (empty, the model's own, the
  per-instance widened one, the stdlib-extended one) plus LTM's own sets; the
  analysis, LTM, layout, libsimlin and CLI paths parse every variable a second
  time under the empty context and can see different dependency sets than the
  compiler schedules. They collapse to one memo per variable; the ~20 call
  sites that construct a context are rewritten in chunk 7.4, which is why
  `rg ModuleIdentContext src/` is empty only after that chunk.
- A synthesized helper is printed to text at two sites and re-parsed at seven;
  its name is its identity at twenty-one. Captures carry the argument as an
  AST subtree with positional identity `(parent, id)`.
- AC3.1 needs more than the parse key: the fragment compilers cloned a
  whole-model module map whose value changed whenever an implicit module
  instance was added. Phase 3 deleted that map (no fragment reads a whole-model
  module map; module shapes come from `model_shape` per sub-model), so chunk
  7.4 starts with a salsa execution-count probe of the pinning test to find
  what still re-executes, whose stated cause (1) --
  `project.models` changing on a stdlib splice -- is inconsistent with
  `db/sync.rs` splicing stdlib models on every sync.
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
D1/D3 predicate as a projection; 7.2 captures for PREVIOUS/INIT with today's
capture set, names, and walk order held fixed (the goldens are the defect
detector); 7.3a stdlib `ImplicitModule` with per-element expansion and shared
`n`; 7.3b macros, passthrough, and GH #554; 7.4 deletion of `ModuleIdentContext`
and the empty-context twin call sites and the AC3.1 flip; 7.5 the shape changes, one commit and ledger row each: dropping
the captures D1 synthesizes for `PREVIOUS(module-call aux)` and
`PREVIOUS(m·scalar_port)` (redundant: codegen's `static_slot` already accepts
`m·port`, and a stdlib-call aux is a one-slot scalar) and refusing a bare
`PREVIOUS(sub)` of an explicit module loudly; generalising the D3 bare-element
rule from the LTM path to user equations; taking INIT captures out of the flows
runlist; keeping `ApplyToAll` instead of rewriting to `Arrayed` for an
apply-to-all body that merely contains `PREVIOUS`/`INIT` (D14); and hoisting
`DELAYN`'s duplicated input argument once.

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
   with the values derived from the rules (the helper and the instance's input
   are 60 on every step; the smooth rises 0, 30, 45). The 0 start is the
   pre-existing cross-model initials boundary -- a stockless sub-model
   evaluates nothing in the initials phase, so a parent stock (an explicit one
   too) initialized from its output reads 0 -- pinned as a disclosed residual
   by `stock_initialized_from_a_stockless_modules_output_is_a_pre_existing_residual`,
   tracked as GH #1028, and independent of this phase.

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
   `DimensionsContext::positional_correspondence`) -- while the parent one runs
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
