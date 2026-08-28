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
  and `rg "make_temp_arg" src/simlin-engine/src` are empty;
  `substitute_dimension_refs` is an AST-to-AST pre-hoist rewrite with no
  print/parse boundary; `rg ModuleIdentContext src/` is empty.

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
pub struct DepTarget {
    pub module_path: Vec<Ident<Canonical>>,
    pub variable: Ident<Canonical>,
}
pub struct DepRef {
    pub target: DepTarget,
    pub phase: DepPhase, // Dt | Init
    pub lag: DepLag,     // Current | Previous | Initial
}
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
is that it is carried as an AST subtree with logical callsite inputs
`(parent, id, argument position, suffix)`, never as equation text. Those inputs
derive the synthetic name; current storage and `ImplicitVarMeta::find_in`
remain name-keyed until the context-dependent parse is removed. The parse
becomes keyed on `(variable, project)`
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

Both parts are done. `dimensions::match_axes` owns axis precedence and
one-to-one occurrence allocation; the Subscript arm is five named steps and
uses one lowering entry point with an explicit dimension-reference mode.
`compiler::array_operand::materialize_arrays`, after subscript resolution, is
the one owner of computed array operands and array-producing results. It
derives every operand decision from `BuiltinSig::ALL`, with the
`ALLOCATE AVAILABLE` priority-profile identity arm explicit because codegen
alone knows how to expand that direct variable view.

Plain apply-to-all and arrayed equations prepare each source expression once,
then resolve that prepared tree for each active element. Each final assignment
opens a `TempAllocator::element_scopes` scope, so its materialized temps use one
dense id range. A phase-local definition cache keeps the first dominating
write at a recycled physical slot when the resolved expression and view are
identical and recursively pure. Time-dependent, lagged, snapshot,
nested-temp, module-evaluation and assignment-target definitions stay local to
their assignment. No source-tree metadata or pointer identity determines a
temp id or reuse verdict; id remapping, probe lowering, and replay paths do not
participate. Resolved SCC assembly refuses any temp definition or read spanning
per-element segments, because segment order otherwise cannot prove that its
write dominates every read. Repeated active-axis occurrences are paired positionally
(`square[D,D]` reads `square[d_i,d_j]`), while a genuinely mapped reference
retains mapped intent and translates the selected element through the canonical
`DimensionsContext::executed_read_correspondence`. Same-rung mapping ties take
target declaration order; an incomplete relation refuses rather than addressing
a nonexistent ordinal. LTM's semantic edge classification follows that same
occurrence-sensitive projection; the name-keyed score layout cannot represent
a target that repeats a dimension and declines it with an attributed warning.
The Phase 6b semantic notes below state the artifact and refusal boundaries.
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

**8a -- source scheduling relation.** `VariableDeps` and `ImplicitVarDeps`
carry one `BTreeSet<DepRef>`, where each row has a complete module-INSTANCE
path, terminal canonical variable, `Dt | Init` phase and
`Current | Previous | Initial` lag. `classify_dependencies` emits the
occurrence identity and lag from production Expr2; the salsa query attaches the
phase and resolves every candidate module hop through the owning source model,
the same parse's implicit modules, or the per-name registry for a source
implicit module referenced by a generated LTM equation. Generated LTM equations
come from the post-module-expansion source AST and do not own a second module
namespace. A `·` split is only a candidate tokenizer inside that
metadata-backed resolver: dimension-element qualification shares the spelling
and never becomes a module path without a proven instance at every hop. The
resolver reads `model_variable_by_name` and `project_model_by_name` at every
segment, so unrelated owner, intermediate, or project map edits backdate at the
selected handle without executing an unchanged dependency query. Scheduling,
`build_var_info`, sparse nested initial outputs, causal/LTM edges, module-output
ports, layout and all four fragment-input constructors consume the structured
fields. Lookup-table references remain a separate textual layout side channel;
causal/element graph output maps remain presentation names rather than source
dependency sets.

**8b -- compiler-expression facts.** Delete `assemble.rs::collect_expr_refs`
and its `LoweredVarFragment::dep_names: BTreeSet<String>`, then make
`db/invariance.rs` consume the authoritative compiler Expr reference walk and
structured source facts without reconstructing phase or lag. Migrate the
remaining LTM `identifier_set`/`classify_dependencies().all_names()` callsites
in `ltm_augment.rs`, `db/ltm/compile.rs`, `db/ltm/mod.rs`, and
`db/ltm/link_scores.rs` only where they schedule a dependency; the per-slot
equation-transformation memberships remain canonical `Ident` sets. The only
production `dep_head` callsites consume `referenced_tables`, the intentionally
textual lookup-table layout channel. `analysis.rs` edge maps and runlist/result
sets are presentation names, not source dependency relations. A resolved
compiler `VarRef` or physical offset cannot replace scheduling identity:
resolution collapses the first instance to a layout base plus terminal element
offset and therefore loses nested instance names, the terminal source ident,
and the pre-assembly `(model, input-set)` key required by sparse initials and
diagnostics. Compiler IR can supply expression/invariance facts to `DepRef`; it
cannot own the source scheduling key.

**8c -- whole-model stages and final docs.** Move unit-checking and the
database-free stage oracle to per-variable queries, delete `model_stage0` and
`model_stage1`, and finish the crate/architecture/performance current-state
documentation. `BTreeSet<String>` values that remain after 8b must be audited
as table/layout references, graph/result presentation names, dimensions, or
test-only fixtures rather than dependency sets.

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
- Every helper the parse synthesizes carries parsed data: a `PREVIOUS`/`INIT`
  capture and a hoisted module-call argument are `Expr0` subtrees with logical
  callsite inputs `(parent, id, argument position, suffix)`, and a stdlib or
  macro module instance is its target model plus its input wiring. No
  compile-stage consumer lexes a
  helper back from generated text, so the printer and lexer do not have to
  agree on every spelling for a model to compile (GH #913's class). The
  source-format `Variable::eqn` projection remains for diagnostics and LTM's
  generated-equation fallback; it does not define the helper's AST or identity.
  A helper's derived name is still its physical lookup key at the name-keyed
  sites: `ImplicitVarMeta::name` selects it, `index_hint` only accelerates that
  name check, and runlists, layout, offsets, and symbolic references are
  name-keyed too. `capture::synthetic_ident` is the ONE derivation; no store
  physically addresses the logical callsite tuple. See "Phase 7.2 captures"
  and "Phase 7.3a implicit modules" below.
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
  initial edge (load-bearing: `LoadInitial` reads `curr` during initials), and
  loses the dt edge. INIT capture storage enters flows only when another
  current-value expression genuinely depends on it. Identical positional
  storage shared by INIT and PREVIOUS unions the two phase demands. Stable
  runlists and layout additionally need the capture's runlist ident to sort
  exactly as today's `$⁚{parent}⁚{n}⁚arg0[⁚{suffix}]` does, with `n` assigned
  in the same argument-first walk order and the same shared-`n`-per-module
  rule.
- LTM's snapshot helpers are captures too and keep append-by-presence placement,
  with fragment presence derived directly from the capture's phase demand; they
  do not enter `model_dependency_graph` in this plan.

Phase 7 is therefore executed as: 7.1 the execution-count probe and the single
D1/D3 predicate (a plain function, not a projection: the parse still decides
and the dep stage still reads the parsed helper list, so no projection removes
a whole-model read); 7.2 captures for PREVIOUS/INIT with today's
capture set, names, and walk order held fixed (the goldens are the defect
detector); 7.3a `ImplicitModule` with per-element expansion and shared `n`,
which covers macro CALLS because `expand_module_function` is one expansion for
both; 7.3b macro passthrough and GH #554, the paths that deliberately do not
reach it; 7.4 deletion of `ModuleIdentContext`, the empty-context twin call
sites and the AC3.1 flip. Removing the context necessarily also removes the D1
captures for `PREVIOUS(module-call aux)` and qualified scalar/array-element
module ports (their lowered references already address concrete slots), and
adds a loud refusal for a bare explicit module rather than allowing it to read
flattened slot zero. A bound module input likewise remains uncaptured and
refuses loudly because its lowered `Expr::ModuleInput` has no snapshot storage;
macro formal parameters retain captures because their typed descriptor is
project-global parse input. 7.5 keeps the remaining shape changes, one commit
and ledger row each: 7.5a generalises the D3 bare-element rule from the LTM path
to user equations, together with the QUALIFIED-element form
`PREVIOUS(vals[Dim.elem])`, whose qualified dimension supplies a 1-based
position that is applied to the referenced variable's own axis (measured in
chunk 7.1, "Phase 7.1 predicate"); 7.5b takes INIT-only captures out of flows
while unioning shared snapshot storage's demands; 7.5c keeps `ApplyToAll`
instead of rewriting to `Arrayed` for an apply-to-all body that merely contains
`PREVIOUS`/`INIT` (D14); 7.5d normalizes `DELAYN`/`SMTHN`'s omitted
initial-value argument to sparse module wiring, leaving the canonical stdlib
model's input fallback authoritative and evaluating the input only once (D10).

**Phase 7.2 captures.** A `capture::Capture` is a `PREVIOUS`/`INIT` argument
hoisted into its own unit of evaluation: `(id, kind, arg, suffix, dims)`, where
`arg` is the argument's `Expr0` subtree exactly as the parse's walk left it,
`id` is the walk counter the visitor was at, and `dims` is the canonical
storage shape. A structural apply-to-all snapshot has all enclosing axes and no
active-element `suffix`; an explicitly per-element or module-expanded body can
carry a suffix for element-specific storage. `capture::ImplicitVar` is the
ordered list a parse produces: a `Capture`, a `HoistedArg`, or an
`ImplicitModule`. Each arm carries parsed data directly.

`capture::synthetic_ident(parent, n, part, suffix)` is the single statement of
how EVERY synthesized helper is named -- captures, module instances, and their
hoisted arguments alike -- so `rg "arg0" src/simlin-engine/src` finds exactly
one production derivation. The constructor's `(parent, id, part, suffix)` is
the logical callsite source of that name, while the name remains the physical
lookup key: `ImplicitVarMeta::{name,index_hint}` resolve it by name, every
runlist is a lexicographic sort, and the layout's implicit section and results
offset map are name-sorted. No downstream store physically addresses a capture
by `(parent, id)`.

`Capture::variable_stage0` is the one constructor of a capture's parse-stage
`Variable`, replacing the `parse_var`-over-printed-text call at every consumer
(`db::implicit_deps`, `db::fragment_compile::lower_implicit_var`,
`db::stages::model_stage0`, `db::analysis::reconstruct_implicit_variable`, both
`db::ltm::compile` sites, and the `ModelStage0::new_in_project` oracle). Two
things it deliberately keeps rather than simplifies, because dropping either
would change the compiled artifact rather than the representation. It fills the
`Variable::eqn` field by printing the subtree, because that field is source
text by definition and LTM's link-score generator has two readers of it:
`ltm_augment::target_equation_dims`, which takes an arrayed target's
datamodel-cased dimension names off it (a target reporting no dimensions gets a
scalar link score), and `ltm_augment::scalar_or_a2a_target_expr`, which falls
back to `scalar_eqn_text_or_zero` and RE-PARSES that text whenever the target
has no lowered AST -- reachable for a capture, because
`db::analysis::reconstruct_implicit_variable` lowers every capture through the
total `model::lower_variable`, which discards the AST on a lowering error. It
runs the body through `instantiate_implicit_modules`, where real module calls
instantiate per element but snapshot-only storage retains `Ast::ApplyToAll`.
LTM's ordinary link-score generation still prints the target's lowered body
(`patch::expr2_to_expr0` + `print_eqn`) and re-parses it in
`db::ltm::equation::LtmArm::new`, the GH #965 generated-text boundary, which
applies to every variable and to captures alike.

One representation difference survives, and it is not observable. A capture
keeps the SOURCE spelling of an identifier where a re-parse kept the lexer's:
`PREVIOUS(vals[d.e2], 0)` captures `RawIdent("d.e2")` where re-parsing
`print_eqn`'s output produced `RawIdent("d·e2")`. `Expr0` -> `Expr1` lowering
canonicalizes every identifier and `common::canonicalize` maps an unquoted `.`
to `·`, so the two are one identifier from that point on;
`db::capture_tests::a_captures_fragment_is_its_argument_compiled` is the
measurement rather than the argument, requiring that row's capture and an
ordinary aux holding the same expression compile to identical bytecode.

Two identical helpers are one helper: `Capture::merge_same_definition` and
`Expr0::eq_ignoring_loc` answer the value-dedup question without consulting
source positions, which is what lets the apply-to-all expansion collapse the N
copies of one cloned body, and what stops a whitespace-only difference between
an element's equation and its initial equation from becoming two helpers
claiming one name. The merge also unions PREVIOUS/INIT consumers when the dt
and initial parses mint the same positional storage. `PartialEq` keeps
positions, because salsa uses it to decide whether a re-parse changed anything
and a moved span changes the diagnostics.

**Phase 7.3a implicit modules.** A stdlib or macro module-function call
expands into two typed values on the same ordered `ImplicitVar` list, one entry
per synthesized variable so list order, name-keyed deduplication, and salsa
equality preserve the source walk's logical callsite identity:

- `capture::ImplicitModule` is the instance: its logical callsite `(parent,
  id, call_name, suffix)`, target `model_name`, and source-to-port inputs. Its
  constructor derives both the `synthetic_ident` external key and every
  `ModuleReference::dst`, so neither can disagree with the typed callsite
  inputs.
- `capture::HoistedArg` is one value per argument that is not a bare
  identifier, carrying that argument's exact `Expr0` subtree, including source
  locations. A bare identifier
  argument wires straight to its port by name, so the wiring and hoisted-
  argument list are not one-to-one; `arg{i}` in a helper name is the argument's
  position in the call, not its position in an auxiliary list.

`ImplicitModule::variable_stage0` and `HoistedArg::variable_stage0` join
`Capture::variable_stage0` as the constructors every compile-stage consumer
uses. `ImplicitVar::Synthesized` and the module-call helper's generated-text
parse boundary do not exist. Because `expand_module_function` is the single
expansion for stdlib calls and macro calls, macro calls use these values too;
7.3b is limited to macro passthrough and GH #554, paths that do not reach that
function.

The typed callsite fields are constructor sources, not a tuple-addressed
storage API. `ImplicitVarMeta::find_in` currently resolves an implicit helper
by `name`, using `index_hint` only as a checked fast path, and later compiler
stages also file helpers by their derived name.

`BuiltinVisitor::insert_implicit_var` is the only mutation of the visitor's
helper map. An exact same-definition repeat is idempotent; different helpers
claiming one derived name preserve the first definition and return a
`DuplicateVariable` error before `IndexMap::insert` can overwrite it. This is
source-reachable: a macro named `ARG1` invoked as `ARG1(k, k * 2)` makes the
module and its computed second-argument helper both derive
`$⁚out⁚0⁚arg1`. Cross-element aggregation uses a `Vec` followed by
`dedup_vars_by_ident`, and the dt/initial merge has the same checked
first-definition contract, so no helper merge path silently overwrites before
checking definition equality.

Two details remain because they encode source semantics rather than the old
text representation:

- `substitute_dimension_refs` is an `Expr0` to `Expr0` rewrite. It runs before
  hoisting because the argument is about to leave an apply-to-all body and
  become a scalar helper with no active element against which lowering could
  resolve a bare dimension name. The apply-to-all rows in
  `db::implicit_module_tests` pin the substituted `vals[d·e1]` bodies.
- `ImplicitVar::is_stock()` answers `false` from the typed arm list. The parser
  synthesizes captures, hoisted arguments, and module instances, none of which
  is a stock; the predicate remains because dependency metadata carries an
  `is_stock` field for every variable. Retiring that field belongs to Phase 8's
  dependency-shape work.

**Phase 7.3b macro fall-throughs.** `MacroRegistry::resolve_call` is the one
exhaustive routing decision for a parsed call: `Expand`, `Passthrough`,
`RenamedBuiltinSelfCall`, or `Unresolved`. The expansion visitor and the macro
recursion graph both read it. The precedence is explicit: inside a macro, its
same-canonical-name importer-renamed builtin is a
`RenamedBuiltinSelfCall` even when the macro's descriptor is also classified
as a passthrough; at an external callsite that descriptor is `Passthrough`.
Both preserve the already-walked function and arguments and continue through
ordinary builtin handling, but only `Passthrough` retains the registered
descriptor: an external call must first match the macro's exact declared
arity. `RenamedBuiltinSelfCall` is the builtin spelled inside the macro body,
so it follows builtin arity instead. A non-passthrough macro validates the
same declared arity and still expands before every builtin, alias, or stdlib
decision; genuine recursion still fails the registry build before expansion.

The successful production rows go through MDL import, salsa sync, the model
module context, and the production variable parse. An external `INITIAL(k *
2)` call in the presence of `:MACRO: INIT(x) = INITIAL(x)` yields exactly one
typed `Capture`; the macro body's own renamed `INIT(x)` also yields a typed
capture because a macro input is not a static snapshot slot, never a recursive
module instance. An ordinary macro with two computed arguments yields two
`HoistedArg`s followed by its `ImplicitModule`. A `DELAYN` macro body wrapping
`DELAY N(..., 1)` yields the distinct typed `stdlib⁚delay1` module and its
three port references. These rows preserve callsite id, argument position,
helper order, source-subtree identity, and module wiring.

The collision-arity rows exercise the production source importers and salsa
compile path, derive the one-parameter contract from each imported descriptor,
and require a valid unary control beside the invalid binary call. `DELAY1` is
an MDL row. `PREVIOUS` is an XMILE row because unary `PREVIOUS(x)` is valid
engine/XMILE syntax but is not a native Vensim builtin: the MDL converter
treats that symbol-kind one-argument spelling as a graphical-function
invocation and imports `LOOKUP(previous, x)`. Both descriptors must classify
as genuine passthroughs; `DELAY1(input, 2)` and `PREVIOUS(input, 0)` are valid
builtin arities but invalid macro arities, and fail as `BadBuiltinArgs`
attributed to the caller rather than compiling through builtin fall-through.

`ImplicitVar::variable_stage0` is the one exhaustive conversion used by every
compile-stage consumer, including both LTM sites, dependency extraction,
fragment compilation, analysis reconstruction, whole-model unit staging, and
the database-free oracle. The text-boundary audit has four intentional
survivors and no implicit-module helper reconstruction:

- `capture.rs` prints a capture or hoisted argument only into the
  `Variable::eqn` source projection used by diagnostics and LTM's documented
  failed-lowering fallback; the carried `Expr0` remains authoritative.
- `MacroRegistry::build` parses user-authored macro body formulas for
  passthrough classification and recursion validation. Those are project
  source equations, not synthesized helper text.
- `LtmArm::new` parses generator output once at the GH #965 generation
  boundary and stores that AST beside diagnostic text. LTM parsing and both
  fragment consumers carry the AST; they do not parse the text again.
- `db/ltm/compile.rs` has one dims-only zero-bodied `datamodel::Aux` parsed for
  an arrayed sibling LTM synthetic variable. It supplies only dimensions to a
  lowering scope and is neither a module-function helper nor derived from one.

`print_eqn` and `make_temp_arg` remain absent from `builtins_visitor.rs`, and
`ImplicitVar::Synthesized` remains absent. `ModuleIdentContext` and the
context-keyed parse entry points intentionally remain for 7.4; this chunk does
not claim AC3.1 or AC2.4.

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

The `SMTH1` column is three different things added together, and the distinction
is what 7.4 needs. Five of the eight `compile_var_fragment` runs and ten of the
thirteen parses are the `stdlib⁚smth1` template compiling for the first time
(its five variables, parsed under the two contexts assembly demands for it: the
model's own and the per-instance one widened by the instance's module-input
names), along with `model_shape` and the second `compute_layout` /
`assemble_module`. That is a new sub-model, not saturation. The added
`smoothed` itself is new work. `k` also legitimately changes: the new implicit
module seeds initials, its initial-dependency closure pulls in parent-side
source `k`, and `k` therefore changes from flows-only membership to
initials-plus-flows and gains an initial fragment. Only `probe` is residual
over-invalidation: its phase membership and fragment input are unchanged.

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
`compile_var_fragment` and `model_implicit_var_info` for `probe`, all four
through the single `ModuleIdentContext` edge. Fixed by a projection -- nothing;
the projections AC3.1 needs (`var_runlist_membership`,
`model_implicit_var_by_name`, `model_variable_by_name`) are already in place and
already backdate, which is why `var_runlist_membership` runs eight times and
recompiles nothing. Inherent -- the new sub-model's first compile, and the
per-model queries of the edited model itself. **So 7.4 needs exactly one thing
for AC3.1 to flip: delete `ModuleIdentContext`, from the parse key and from
every caller that derives it (the eleven `model_module_ident_context` call
sites and the empty-context twins, which is chunk 7.4's list). No new
projection is warranted. The first module instance still recompiles `k` for
its required phase-membership change; AC3.1 forbids recompiling `probe`.**

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
production. D1's module-call/qualified-port rows agree after 7.4, and D3's bare
and qualified element rows agree after 7.5a. The one recorded divergence runs
the other way: `PREVIOUS(arr)` for an arrayed `arr` in a scalar position, where
the source AST has not yet attached arity and codegen refuses the array-valued
read. That costs no storage -- the equation is ill-typed with or without the
`PREVIOUS` (`x = arr` refuses too, with a different message) -- and the refusal
is loud. Every INIT twin is in the same table, so either intrinsic changing one
verdict without the other fails the exhaustive matrix.

**Phase 7.4 context-free parse.** `parse_source_variable(var, project)` is the
only source parse query. Its project dependency is limited to source-language
facts -- relevant dimensions, units, macros and the enclosing typed macro
descriptor -- plus 7.5a's stable owner/base/index projections when a snapshot
argument contains an element subscript; it never reads an owning model's whole
variable-name set or an instance's input wiring. `ModuleIdentContext`, both
model-context constructors, every contextual/empty twin parse and D2's equation
pre-scan are absent. Generated LTM equations deliberately retain
`ltm_model_var_names`, because their synthesized variable surface is a whole-set
boundary and needs helper-aware element-versus-variable shadowing.

The unavoidable D1 artifact slice lands here rather than pretending 7.4 can be
artifact-neutral. A scalar stdlib-call auxiliary and qualified scalar or
array-element output port no longer allocate a redundant capture; lowering
reads their resolved slot. A bound module input no longer acquires a helper
from instance-specific parse context and refuses loudly because
`Expr::ModuleInput` has no snapshot slot. A bare explicit module also refuses
loudly in direct and LTM lowering, closing the silent flattened-slot-zero read.
Macro formal parameters remain captures because the project-keyed typed macro
registry names them without model-local context. Capture allocation for
non-storage resolved shapes remains in the later 7.5 slices.

Qualified same-step initial dependencies propagate into the referenced child
module's sparse initials program. A project-level fixed point starts from each
production module key's actual model-local initials runlist, removes
PREVIOUS-only paths through the shared dependency classification, follows
explicit and generated module instances structurally, and preserves nested
suffixes such as `m·n·out`. Requirements are unioned by the runtime compilation
identity `(model, bound-port set)`: instances sharing a key share the required
initial bytecode but retain private slots, while distinct `isModuleInput`
branch keys remain separate. The per-key salsa projection extends only the
initial runlist; the existing membership projections therefore compile the
new initial fragments without changing VM/wasm `EvalModule` phase semantics.
Production VM rows pin scalar, array element, nested, stock, active-initial,
implicit-module, same-key-union, distinct-input-set and resolved-SCC values;
the PREVIOUS-only and unrelated-output controls remain absent. The wasm row
pins both parity and the absolute qualified-INIT value. A seed edit recompiles
only the caller plus the two child variables whose initial membership changes.

The fixed point reads accumulator-free dependency facts. It must inspect every
project root because two roots can demand different outputs from the same
runtime `(model, bound-port set)` key; attaching cycle emission to that shared
walk leaks an unrelated draft model's diagnostic into each caller. The local
facts therefore carry at most one unresolved-cycle attribution as ordinary
memoized data, and `emit_model_dependency_cycle_diagnostic` is the sole
per-model accumulator trigger. Assembly rejects through the same pure
`has_cycle` fact. A production regression pins zero draft-cycle rows when
collecting a valid `main`, exactly one attributed row in whole-project and CLI
diagnostics, successful main simulation, and the same results after an
unrelated revision. Its salsa execution probe observes one execution per graph
key on the initial walk and no graph/SCC or cycle-emitter execution after the
revision; only the eleven intentionally untracked `model_all_diagnostics`
bodies rerun and replay the cached per-model emitter's accumulator row.
The transitive-closure DFS sorts its cloned root identifiers before recursion;
successors were already lexical `BTreeSet`s. This makes the first attribution
canonical even with several disconnected cycles, pinned across every rotation
of a seven-variable production declaration alphabet, 28 fresh databases and an
unrelated revision in each. The artifact effect is diagnostic-only: a
compilable graph's transitive maps and separately sorted runlists are unchanged,
and an unresolved-cycle graph emits no bytecode; only the selected
`Diagnostic.variable` is stabilized.

AC3.1 is pinned through salsa's production execution events. Adding the first
`SMTH1(k, 2)` to `k = 3; probe = k * 2` executes explicit fragments exactly
`[delay_time, flow, initial_value, input, k, output, smoothed]`, its two new
implicit helpers, and six distinct parses (`smoothed` plus the five first-use
stdlib sources), while `probe` stays cached. The `k` execution is required by
its new initial-phase membership. Adding a second instance executes and parses
only `smoothed2` and compiles only its two new helpers; the first instance,
`k`, `probe`, and the stdlib template remain cached.

**Phase 7.5a static element snapshots.** Source PREVIOUS and INIT arguments
whose subscripts pin one declared element read that slot directly for both the
bare (`vals[e1]`) and qualified (`vals[Dim.e1]`) spellings. The source parser's
`SnapshotIndexResolver` owns this D3 decision because the parse result owns the
capture list consumed by both dependency construction and lowering; there is
no second downstream classification to drift. The salsa and datamodel paths
derive the referenced axis and qualified 1-based position from their own real
inputs; `dimensions::resolve_snapshot_axis_index` makes their shared final
decision through the canonical `resolve_axis_index_name` and
`resolve_axis_index_position` helpers. Bare source elements follow XMILE
footnote 9: an owning-axis element wins over a same-named model variable.
Qualified positions may come from an unrelated dimension and are applied to
the referenced axis. `source_variable_owner_model` finds the stable model
handle, `variable_dimensions` supplies the declared axis, and the tracked
per-qualified-name position projection backdates an unchanged result before an
unrelated dimension edit can re-run the parse. Generated LTM equations retain
their local full-name-set resolver and conservative helper-aware shadowing:
their variable surface is synthesized as a whole, so per-name source
incrementality does not apply there.

The production matrix crosses every PREVIOUS/INIT intrinsic with same-name
collision, unrelated-axis qualification, missing qualified and bare names,
globally ambiguous bare names, mapped axes and a proper named subdimension.
It compares the salsa and datamodel Stage0 derivations, then pins capture
discovery, direct opcodes and VM values or a loud refusal. The numeric,
dimension-spanning, dynamic-expression and module arms remain enumerated in the
general parse/codegen table, phase-runlist/layout fixture, and module refusal
tables. A separate LTM discovery fixture pins the topology consequence: bare
and qualified reads of `a1` coalesce into one dimensioned score variable, the
same-named `b2` element produces another dimensioned score, and only dynamic
reads retain a storage capture and its dimensioned score; both user and score
series are asserted. INIT is outside that fixture's dt-score boundary and
remains covered by the shared PREVIOUS/INIT matrix. C-LEARN carries the same
shape independently: 35 scalar/per-element score names coalesce, their declared
extent grows by 278 slots, and removing 26 source captures yields a net
+252-slot LTM layout.

**Phase 7.5b capture phase membership.** `CaptureKind` states the storage's
phase demand: PREVIOUS refreshes in flows for the next committed snapshot and
INIT populates initials before `initial_values` is frozen. The dt and
active-initial parsers restart the positional counter; when they mint the same
ident, dimensions and argument body for different snapshot consumers,
definition dedup retains the first helper and unions the demands. Different
bodies or helper forms remain an attributed `DuplicateVariable` refusal.

Ordinary implicit dependency extraction carries `flow_required` and
`initial_seed_required` from the capture kind into `VarInfo`, and preserves raw
dt `init_referenced` facts from both explicit and implicit definitions. Those
facts are first-class initialization roots: a later flow program needs the
frozen referent even when its owning definition does not itself run in
initials. Qualified facts enter the project-level propagation directly, so a
live mixed expression or a nested PREVIOUS capture seeds exactly the demanded
child output and its local initial closure without widening unrelated outputs.

The flow runlist excludes an INIT-only capture
unless the transitive production dt relation rooted at a per-step definition
contains a current-value read of its name; INIT/PREVIOUS-only edges are removed
before that test, so a snapshot consumer cannot promote itself, and a current
edge reachable only from another INIT-only capture stays initial-only. The slot
remains allocated because INIT needs it while initials run. Symmetrically, a
PREVIOUS capture whose body has only initial dependencies does not become its
own initials root; a live initial consumer can still pull it through the normal
dependency closure. After the snapshot is frozen an INIT-only capture's
unwritten `curr` value is dead and backend-private; a quoted source reference
to the synthesized name from a live definition creates the current dependency
and restores the flow fragment. LTM implicit helpers bypass the model
dependency graph, so their compiler selects required phases directly and the
LTM diagnostic predicate requires exactly those phases. Today's generated
scores synthesize PREVIOUS captures only, while the typed LTM parser and
compiler accept both source capture kinds.

The production-derived coverage crosses PREVIOUS and INIT with computed
scalars, dynamic elements, array-valued storage and an initial-backed body;
explicit, bound-module, macro caller/body and typed/generated-LTM sources;
both parser orders of same-position demand union; live current and initial
consumer promotion; an INIT-only dependency chain; and local, nested qualified
and mixed live-flow INIT referents, including sparse exclusion of an unrelated
child output. It pins runlist membership, fragment presence, retained layout
slots, assembled assignment targets, VM initialization/reset/rerun values and
user-variable VM/WASM parity. The fragment characterizations state the
intended shape: INIT capture helpers are initial-only, and generated LTM
PREVIOUS helpers are flow-only.

**Phase 7.5c structural snapshot storage.** `PerElementRequirements` is the
single recursive classification of calls that need per-element parse work. It
uses the same `resolve_call` / `MacroCallResolution` router as expansion, then
applies the opcode, stdlib and project-macro fallthroughs. A real stdlib or
project macro needs a distinct module instance for each active element and
converts its enclosing `Ast::ApplyToAll` to explicit `Ast::Arrayed` bodies;
PREVIOUS/INIT need snapshot storage but retain a structural apply-to-all body.
The structural visitor has the declared axes but no active element, so it
creates one capture carrying those axes, the untouched argument and no suffix.
Ordinary apply-to-all lowering remains the one place that resolves active axes,
mappings, repeated axes and proper subdimensions. `Capture::variable_stage0`
constructs the same `Ast::ApplyToAll`, so parse metadata, layout and fragment
lowering consume one stored shape. Both endpoints of `IndexExpr0::Range`
participate in classification, dimension substitution and capture walking.

An explicit `Ast::Arrayed` preserves the element context of each body. A
snapshot-only default is transformed once in structural context and its capture
read is inserted only into missing storage slots. A module-bearing default is
materialized independently for each missing slot. An inactive default, or an
active default with no holes, is never visited and cannot synthesize storage or
module instances. Thus downstream dependency, LTM and aggregate walkers see the
same missing-slot restriction as compilation rather than an unrestricted
default expression.

**Phase 7.5d sparse default module input.** [XMILE 1.0 section
3.5.3](../reference/xmile-v1.0.html#_Toc439926074) specifies that an omitted
delay or smooth initial value uses the input's initial value. Alias
normalization supplies only `[input, delay_time]` for omitted-initial `DELAYN`
and `SMTHN` calls. The canonical stdlib models are authoritative: their
`isModuleInput(initial_value)` guards use the bound port when present and fall
back to `input` otherwise. The input is therefore evaluated or hoisted once,
and there is no second source-to-port binding for LTM or the implicit dependency
graph to reconcile. An explicit fourth argument supplies the third port and is
always an independent expression, even when its spelling or location-free AST
equals the input. Different callsites likewise retain callsite-derived instance
and helper identity. `DELAY` only renames to `DELAY1`, and ordinary macros keep
their supplied arguments. XMILE permits arbitrary order N; the engine currently
has canonical first- and third-order module targets, so other literal orders and
dynamic orders refuse loudly rather than inventing semantics.

The context-free source parse intentionally sees only the owning variable's
relevant axes. A per-element `HoistedArg` retains canonical `(axis, element)`
coordinates only, so memoized active metadata is O(rank) rather than cloning a
named dimension's complete element and mapping tables into every element's
helper. `substitute_active_dimension_refs` performs the locally visible
substitution during expansion. `HoistedArg::variable_stage0` resolves those
lightweight axes through the full project `DimensionsContext` and applies the
same idempotent walker, so foreign mappings and proper subdimensions use
`DimensionsContext::resolve_mapped_read` at the first layer that owns their
metadata. This does not widen a salsa parse key or restore model-context
parsing. The production-derived matrix covers all four `MacroCallResolution`
routes; valid and invalid DELAYN arities and engine-supported orders; omitted
and explicit initial forms; scalar, shaped and dynamic arguments; mapped,
proper-subdimension and repeated-axis reads; qualified modules and sparse
initial outputs; nested INIT and PREVIOUS captures; reset/rerun; SCCs; exact LTM
topology and series; and both backends.

`db::ltm::endpoint_dimensions` is the one projection of an ordinary causal
endpoint's declared shape. It reads an explicit non-module variable or the real
`model_implicit_var_by_name` entry used by fragment compilation; `Some([])` is a
scalar and `None` is a module or unknown name. The per-name query is an
incrementality firewall: adding an unrelated source helper may revalidate the
projection but cannot execute an unchanged endpoint's score query.
Element-graph, link-score and loop routing all consume that projection. The LTM
fragment compiler's broader `ltm_dependency_shape` additionally covers LTM
implicit and synthetic namespaces, so absence from `SourceModel::variables`
never implies a scalar dependency. Compatible capture edges inherit their
dimensions; disjoint fixed-element sources route across every target slot;
incompatible axes emit one deterministic warning and no scalar score fallback.

`model_flow_member_names` and `dt_causal_dependencies` are the shared phase
projection for LTM causal edges, aggregate registration, reference sites and
module-output ports. INIT-only source and parent edges stay outside scoring;
an ordinary current read promotes a capture and its source relation, while the
parent INIT edge remains excluded. PREVIOUS-only state stays in the causal graph
and in both stateless and pinned-cycle lag classification. Aggregate width
rejection applies only to causal sites, so dead INIT syntax neither allocates
LTM storage nor disables otherwise scoreable analysis. The per-edge link-score
query reads per-name occurrences, per-module output ports and the module's own
reconstructed variable; unrelated helpers or module ports do not invalidate it.
Whole-model loop builders cache endpoint dimensions locally instead of entering
the per-name salsa projection for every circuit edge.

`EdgeShapesResult::target_restricted_edges` records a bare edge whose observed
`target_element` sites cover only a strict subset of the target storage. Such an
edge takes the element graph's slow path even when endpoint dimensions match,
so a materialized missing-only snapshot or module-call default cannot
manufacture loop or pin slots for overridden elements. Pinned-cycle
classification uses the same endpoint dimensions and restriction decision as
discovery. Compatible, fixed-disjoint,
scalar-broadcast and incompatible shaped endpoints therefore have one topology
source of truth.

The production-derived matrix pins both source intrinsics; scalar, dynamic,
fixed and array-valued reads; mapped, repeated and proper-subdimension axes;
explicit/default/implicit/module definitions; stock, initial and flow phases;
LTM scores, loops, pins and SCC assembly; every call-router and index-expression
arm; initialization reset; and exact VM/WASM values. Focused endpoint fixtures
derive captures through the production implicit-metadata path, assert every
capture and score slot is written exactly once, and require compatible,
fixed-disjoint, scalar-broadcast and loud incompatible topology. Phase fixtures
cover direct INIT, computed INIT capture, current-read promotion, PREVIOUS-only
state, module ports, aggregates and width rejection. Execution-count fixtures
pin the per-name and per-module firewalls. C-LEARN's all-slot digest and exact
warning surface complement those fast tests with every emitted LTM slot and all
production warnings. Dynamic range bounds have exact VM values; wasm's existing
`ViewRangeDynamic` limitation is an explicit unsupported skip.

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
   are 60 on every step and the smooth starts and remains at 60). The child
   output is part of the cross-model initial requirement fixed point described
   under Phase 7.4, so the same t0 value is available to an explicit parent
   stock initialized from that output.

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
  These are content choices at stable diagnostic sites, independent of array
  materialization ownership; changing them requires a diagnostic-text contract
  rather than another lowering mechanism.

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

**Phase 6b semantic divergences.** The materialization owner change preserves
the genuine-output corpus but deliberately changes two observable compiler
contracts.

1. A dimension reference is occurrence-sensitive. With `square[D,D]` holding
   `10i+j`, `out[D,D] = square[D,D]` reads `square[d_i,d_j]`; the old
   independent lookups selected active axis zero twice and read
   `square[d_i,d_i]`. The same allocation covers an implicit whole-variable
   read, wildcard/range forms, exact name, mapping, subdimension/element-map,
   size-only fallback, and no-match refusal. `active_dimension_refs` retains
   only the positional-vs-mapped intent; allocation is
   `dimensions::match_axes_partial`. A computed operand aligns that match with
   `DimensionsContext::executed_read_correspondence`, including both mapping
   declaration directions and explicit element maps. Same-rung ties take
   target declaration order, and an incomplete correspondence refuses. Source
   reference element translation still uses
   `DimensionsContext::resolve_mapped_read`. Production parse and subscript
   resolution feed the exhaustive matrices in
   `mapped_reference_semantics_tests` and
   `array_operand_materialization_tests`; the shared repeated-axis production
   fixture asserts absolute VM values, absolute wasm values, and backend
   parity.
2. Materialized work is owned after subscript resolution, rather than by a
   speculative pre-subscript hoist. A phase-local cache reuses a recycled temp
   slot's first write only when both the complete final expression and output
   view are identical and the expression is recursively reusable. Eligibility
   reads `BuiltinSig::invariance`: all pure builtin rows qualify (including
   lookup, whose compiled graphical-function tables are immutable); the
   time-dependent, lagged and snapshot rows, nested temps, module evaluation
   and assignment-target reads do not. The cache is discarded at the phase
   boundary, and reuse is checked only at the physical slot the allocator
   assigns to the current definition. Resolved SCC assembly refuses a temp touched by more than one exact
   per-element segment; no combined SCC can reorder a read ahead of its write.
   Thus `VECTOR SORT ORDER(vals[D], 1)` over 300 elements emits one dominating
   write, while `VECTOR SORT ORDER(vals[D], dir[D])` emits 300 element-specific
   writes. A repeated target-axis fixture has three row definitions, not nine
   cell definitions. The fixtures assert both bytecode counts and VM results.

Every builtin row is accounted for from `BuiltinSig::ALL`: five array-result
builtins are materialized, every `ArgKind::Array` position is materialized
except `ALLOCATE AVAILABLE`'s profile identity arm, and scalar, table, and
identifier positions are explicit non-materialization rows. Lookup tables need
one further execution distinction: a scalar `@N` table subscript resolves to
that element, while an arrayed lookup keeps a wildcard until the materializer
can build the graphical-function array result. The GF matrix pins fixed,
wildcard, mapped, per-element, scalar-result, and loud unmatched-projection
forms.

LTM records a repeated projected source occurrence as `PerElement`, so its
causal edges follow the values simulation reads. A score target that itself
repeats a dimension remains unrepresentable by the name-keyed score layout and
is refused with one attributed warning per edge; the integration fixture
separates that warning from the existing square-source refusal and asserts no
silent score or warning cascade in exhaustive and discovery modes.

**Risks.** Array materialization deliberately remains compile-time
per-element expansion. A later loop-form lowering must preserve the same
post-subscript shape and refusal contracts. Phase 7 changes salsa keying; the
determinism suites and the execution-count tests are the guard.

### GH #1025 current-state handoff

The issue update must **replace**, not append to, its stale implementation and
eligibility sections: those sections name expansion helpers that no longer
exist and exclude array-producing builtins, which would leave the measured
C-LEARN residual untouched. The replacement text to post after this branch is
authorized is:

> Phase 6b gives array materialization one owner:
> `compiler::array_operand`, after subscript resolution and constant folding.
> Apply-to-all equations still compile to one final scalar assignment per
> element. The current loop-form design excludes array-producing builtins from
> its batched eligibility, so it does not recover the execution shape Phase 6b
> exposes on C-LEARN.
>
> The concrete residual is four equations: `sorted_target_active`,
> `sorted_target_type`, `sorted_target_value`, and `sorted_target_year`. Each
> has seven COP-coordinate-specific final source slices. Against exact base
> `c666c4a6`, plain C-LEARN therefore has 24 extra `VectorElmMap`, 48 extra
> `PushStaticView`, and 48 extra `PopView` in each of flow and initial bytecode
> (`+120/+120` opcodes and `+96` static views overall). LTM initial bytecode has
> the same additions; its flow bytecode independently removes 32 maps and 64
> of each view opcode, so LTM's net artifact is `-160` flow, `+120` initial and
> `-16` views.
>
> Five alternating exact-base/current plain `RUN_ITERS=20` measurements put the
> run channel at `+6.865%`. Five alternating LTM rounds with `RUN_ITERS=20` and a matching
> zero control for every binary/round put one added LTM run at 19.4937 G base
> versus 19.6967 G current retired instructions: `+1.0413%`, or +203.0 M per
> run (paired range `+1.0095%..+1.0715%`). Phase 6b intentionally does not
> recover batched whole-array execution. This is accepted temporarily because
> loop-form/batched lowering is explicitly sequenced after compiler cleanup,
> not because either regression is small.
>
> Extend #1025 so batched array-producing builtins are in scope. Start with a
> production-derived `VectorElmMap` row whose source and offset views vary by
> the outer apply-to-all coordinate but form one runtime-iterable definition.
> TDD must pin VM and wasm values, the exact map/view opcode classes above,
> plain and LTM artifact counts, and five-round control-subtracted run cost.
> The existing Phase 6b matrices remain fallback gates: EXCEPT arms,
> mappings/subranges, repeated axes, snapshots, tables/dimension names,
> per-element graphical functions, reducers, assignment-target/stateful reads,
> and resolved SCCs must either preserve their current semantics or decline
> loudly. Acceptance is removal of the four `sorted_target_*` duplicate groups
> without restoring pre-resolution probing or a second materialization owner.

## Measured

Ledger rows are recorded by each phase's teammate. Every compile-cost number
is retired instructions unless the row says otherwise. A phase row names its
commit by subject line: the row is written before the commit exists, so the
hash is not available to it.

| phase | commit | cold compile Ir | slots | opcodes (flow / stock / init) | literals / GFs / temps / views | notes |
|---|---|---:|---:|---|---|---|
| 8a | `engine: structure source dependencies` | Against exact `af201c7f` and the final release binary pinned to one performance core, five alternating paired rounds with a matching zero-extra-compile control for each binary and round put the per-side median plain compile at 0.99120 G instructions current versus 1.02325 G base (-3.132%); the median paired delta is -3.177% (range -3.497%..-2.010%). LTM is 10.95372 G current versus 11.14736 G base (-1.737%); median paired delta -1.989% (-2.213%..-0.372%). | 5189 plain / 28490 LTM | plain 28637 / 1477 / 24711; LTM 830380 / 1477 / 24711 | plain 1358 / 162 / 28 / 725; LTM 13113 / 162 / 28 / 2334 | `DepClassification` emits canonical occurrence + lag rows from production Expr2; `VariableDeps` and `ImplicitVarDeps` attach dt/init phase and a metadata-proven `DepTarget { module_path, variable }`. Scheduling, `build_var_info`, nested sparse initials, causal/LTM edges and pins, module-output ports, layout and all fragment constructors read the structured fields. The resolver recognizes explicit and same-parse implicit modules plus source-registered implicit modules read by generated LTM equations, proves every nested hop, and leaves dimension-element qualification local; generated equations come from the post-expansion source AST, so the unreachable LTM-owned module lookup is absent. The adversarial production fixture gives identical `d.e` source spelling a dimension-element meaning with dimension metadata and a module-path meaning without it, while distinct `[left, inner]/out` and `[right, inner]/out` targets remain distinct. The exhaustive classifier matrix covers every lag/context/index arm; the production phase matrix crosses both phases with all three lag variants. Previous-only state is `Previous - Current`, so an Initial fallback or sibling snapshot never cancels the lag. Production resolver rows cover leading-parent wiring, missing target/intermediate/non-module/leaf metadata, depth three, same-parse and source-registered implicit modules, and full missing-leaf diagnostic identity. Per-name model and variable projections prove unrelated owner/intermediate/project structural edits do not execute an unchanged dependency query. Existing production suites cover captures, stdlib/project-macro modules, active initials, mappings/subdimensions, SCCs, LTM synthetic/implicit fragments, diagnostics and both backends. Plain and LTM artifacts are byte-identical to the base profile: totals 54825 / 856568, post-fusion flow+stock 17017 / 384463, result offsets 4900 / 13800, and every slot/opcode/histogram/literal/GF/temp/dimension/view/name/module count agrees. Release C-LEARN user series parity, all generated partials, complete warnings, VM/wasm discovery parity and the all-slot digest `(19264, 0, 3106, 19264, 790401758590, 2212, 7484877280482623718)` are green. Validation: engine lib 5731 passed / 2 ignored; integration 786 passed / 16 ignored; VM allocation 4 passed; doctests 2 passed / 1 ignored; both determinism modules pass 12 consecutive invocations; all-target/all-feature engine clippy with warnings denied, rustfmt and `git diff --check` are green. 8b owns compiler-Expr invariance (`collect_expr_refs` / `dep_names`) and any remaining LTM per-slot identity projection; lookup-table strings are a separate layout channel. Physical offsets cannot replace source scheduling identity because resolution erases nested instance and terminal source names before sparse initialization and diagnostics consume them. |
| 7.5d | `engine: use sparse delay initial ports` | Against the exact saved 7.5c binary, five paired control-subtracted rounds put one plain compile at 1.01913 G instructions current versus 1.02065 G base, -0.1486% (paired range -0.2390%..-0.0969%), inside the channel's noise floor. One LTM compile is 11.12913 G versus 11.30982 G, -1.5974% (-1.7719%..-1.3896%); cycles / branches / branch misses move -1.3810% / -1.5258% / -0.8632%. C-LEARN contains no DELAY N, SMOOTH N, DELAYN or SMTHN call. This slice also changes active-coordinate residency and replay for per-element module arguments, so the LTM compile signal is confounded; without an isolated A/B, no causal mechanism is assigned to it. | 5189 plain / 28490 LTM | plain 28637 / 1477 / 24711; LTM 830380 / 1477 / 24711 | plain 1358 / 162 / 28 / 725; LTM 13113 / 162 / 28 / 2334 | Omitted-initial DELAYN/SMTHN aliases normalize to `[input, delay_time]`; the canonical stdlib model's unbound `initial_value` guard is the single implementation of XMILE 1.0 section 3.5.3. An explicit fourth argument supplies an independent third port even when its source text equals the input. A per-element `HoistedArg` retains canonical `(axis, element)` coordinates only and resolves the full `Dimension` transiently from `DimensionsContext` during stage0 replay, so persistent metadata is O(rank), not O(rank times dimension cardinality). One shared substitution walker serves source expansion and resolved replay. The production-derived matrix covers both aliases, all four macro-router arms, DELAYN's valid 3/4 and invalid 2/5 arities, engine-supported literal orders 1/3 and loud literal/dynamic unsupported orders, scalar/array/dynamic/mapped/proper-subdimension/repeated-axis sources, element-name precedence, nested INIT/PREVIOUS, qualified modules, sparse child initials, exact phase demand, SCCs, exact LTM topology/equations/all-slot series, reset/rerun and VM/WASM values. Direct DELAY1/3 and SMTH1/3 omitted-port controls match the aliases exactly in VM, WASM, reset and LTM compilation. C-LEARN's complete plain and LTM artifact tables, opcode histograms, fused-binop tables, 4900/13800 result offsets, 371 names and 7 modules are byte-identical to 7.5c; totals remain 54825 and 856568 opcodes and post-fusion flow+stock remains 17017 and 384463. Release user series match plain versus LTM, the WASM/VDF gate matches 2924 variables across 251 steps, every emitted LTM partial parses, the warning surface remains one discovery switch plus 17 incompatible-dimension and five rank-like declines, and the all-slot digest remains `(19264, 0, 3106, 19264, 790401758590, 2212, 7484877280482623718)`. Control-subtracted execution is flat: plain is 1.38437 G current versus 1.38440 G base, -0.0024% (paired range -0.0412%..+0.0244%); LTM is 17.75099 G versus 17.76045 G, -0.0533% (-0.0838%..-0.0304%). Validation: engine lib 5727 passed / 2 ignored; integration 785 passed / 16 ignored, including genuine-output, VM/WASM and LTM corpora; VM allocation 4 passed; doctests 2 passed / 1 ignored; both determinism modules pass 12 consecutive invocations; ignored release C-LEARN LTM parity, WASM/VDF, all-partial and all-slot digest gates are green; all-target/all-feature engine clippy with warnings denied, rustfmt and `git diff --check` are green. XMILE's arbitrary-N DELAYN/SMTHN remains unsupported beyond the engine's canonical order-1/order-3 modules and refuses loudly. C-LEARN cannot demonstrate affected storage-count reduction because it has no N-order aliases; the production 64-element scale fixture proves that every emitted helper retains one coordinate and zero dimension element records. |
| 7.5c | `engine: retain shaped snapshot captures` | Against the exact saved 7.5b binary, five paired control-subtracted rounds put one plain compile at 1.02097 G instructions current versus 1.13250 G base, -9.8485% (paired range -9.9812%..-9.7160%), and one LTM compile at 11.3083 G versus 13.4659 G, -16.0228% (-16.1638%..-15.8714%). Cycles / branches / branch misses move -9.7455% / -9.6711% / -10.8682% plain and -15.3566% / -16.0980% / -16.4733% LTM | 5189 plain / 28490 LTM | plain 28637 / 1477 / 24711; LTM 830380 / 1477 / 24711 | plain 1358 / 162 / 28 / 725; LTM 13113 / 162 / 28 / 2334 | `PerElementRequirements` consumes the authoritative macro router and separates real stdlib/project-macro instances from opcode-backed PREVIOUS/INIT storage. Snapshot-only apply-to-all equations retain one structural `Ast::ApplyToAll`; explicit/default equations preserve active element coverage; module defaults materialize only missing bodies; inactive or fully overridden defaults mint no helper. Both `IndexExpr0::Range` endpoints participate in classification, substitution and walking. `endpoint_dimensions` reads explicit variables and the per-name `model_implicit_var_by_name` firewall and is the shared shape source for causal edges, element graphs, scores, loops and pins; broad loop construction caches those shapes. `model_flow_member_names` plus `dt_causal_dependencies` make causal edges, aggregate/reference-site discovery and module ports phase-aware while retaining PREVIOUS-only state. Target-restricted bare edges take the element slow path, so a materialized missing-only snapshot or module-call default cannot manufacture override-slot loops. Per-edge module scores use per-name occurrences, per-module ports and per-name reconstructed module metadata. Against 7.5b, plain keeps flow/stock artifacts fixed while initials fall 126, literals fall 165 and result-offset identities fall 132. Under LTM, root slots move 30375 -> 28490, emitted variables 7128 -> 5561, result offsets 16012 -> 13800, flow opcodes fall 73910, literals fall 3330, static views fall 112 and post-fusion flow+stock falls 34077 (418540 -> 384463); stocks, GFs, temps, dimensions, names and modules are unchanged. Generated LTM implicit helpers move from 738 scalar variables / 738 slots to 225 shaped variables / 759 slots. The checked-in 7.5b digest's 20892 slots first become 21156 through +278 structural extents and -14 fixed-slice declines. Phase-aware discovery then removes 1864 zero link-score slots and 28 INIT-only freezer slots, leaving 19264. Fourteen freezer slots are zero; the other fourteen are the two `effective_target_year` elements in each of seven COP regions, maximum 4000. All 19264 post-shaping/pre-phase-filter survivor maxima are identical. The exact warning pin covers one established discovery switch, 17 established incompatible-dimension declines and five established rank-like declines; there are no added `last_set_target_year` warnings. Release C-LEARN preserves every user-model series with LTM off and on. Final-binary execution is 1.38436 G current versus 1.40780 G base per plain run, -1.6651% (paired range -1.6791%..-1.6549%), and 17.7597 G versus 19.1676 G per LTM run, -7.3449% (-7.3729%..-7.3175%); cycles / branches / branch misses move -0.7613% / +0.0393% / -1.8155% plain and -6.8262% / -6.8936% / -6.5532% LTM. `RuntimeView::dense_linear_start` remains explicitly inlined because it is reachable from the per-element generic addressing path, preventing an out-of-line call there. The production-derived matrix crosses PREVIOUS and INIT; scalar, dynamic, fixed, array-valued, mapped, repeated and proper-subdimension operands; explicit/default/implicit/module, stock/init/flow, LTM score/loop/pin and SCC routes; initialization reset; exact VM/WASM values; every `MacroCallResolution` and `IndexExpr0` arm; per-name/per-module execution-count firewalls; phase-dead aggregate/port exclusion; and loud incompatible topology. Validation: engine lib 5716 passed / 2 ignored; integration 782 passed / 16 ignored, including genuine-output, VM/WASM and LTM corpora; VM allocation 4 passed; doctests 2 passed / 1 ignored; release C-LEARN parity, complete-warning guard and all-slot digest are green; both determinism modules pass 12 consecutive invocations; all-target/all-feature engine clippy with warnings denied, rustfmt and `git diff --check` are green |
| 7.5b | `engine: compile captures only in required phases` | Against the exact saved 7.5a binary, five paired control-subtracted rounds put one plain compile at 1.13212 G instructions current versus 1.14225 G base, -0.888% (paired range -0.975%..-0.807%), and one LTM compile at 13.4573 G versus 13.4923 G, -0.259% (-0.420%..-0.137%); both are below the plan's 1% investigation threshold | 5189 plain / 30375 LTM | plain 28637 / 1477 / 24837; LTM 904290 / 1477 / 24837 | plain 1523 / 162 / 28 / 725; LTM 16443 / 162 / 28 / 2446 | A capture carries an exhaustive PREVIOUS, INIT or combined phase demand. Definition dedup unions the demand when dt and active-initial parsing mint the same positional helper in either intrinsic order; mismatched bodies or helper forms still refuse loudly. INIT-only helpers retain their initial fragment and layout slot but leave the flow runlist unless a per-step definition's production dt dependency closure contains a current-value read of that helper; PREVIOUS/INIT snapshot edges cannot promote themselves, and a dependency reachable only from another INIT-only helper remains initial-only. Symmetrically, an initial-shaped PREVIOUS body does not seed itself, while an independent live initial consumer pulls it through the ordinary closure. Raw dt INIT referents from explicit and implicit live definitions are first-class project initialization roots, so local and nested qualified captures plus mixed live flows populate exactly the demanded child output while unrelated outputs remain sparse. The post-init current slot is deliberately backend-private and dead: VM internal results may contain zero where wasm retains the initialized scratch value, while user variables agree; naming the hidden helper from a live definition creates a real current dependency, restores its flow fragment and makes both backends agree on the now-observable slot. LTM implicit compilation applies the same demand directly because those helpers bypass the ordinary model graph. C-LEARN slots and result offsets are unchanged (5032 plain / 16012 LTM). Plain removes 2,125 flow opcodes, 217 literals and 14 views; LTM removes the same 2,125 flow opcodes plus 3,856 initial opcodes and 770 initial programs from redundant generated-PREVIOUS initialization, with 584 fewer literals and 404 fewer views. Totals move 57,076 -> 54,951 plain and 936,585 -> 930,604 LTM; post-fusion flow moves 17,037 -> 15,942 plain and 418,560 -> 417,465 LTM. Names, modules, GFs, temps and dimensions are unchanged. Release C-LEARN preserves every user-model series with LTM off and on. Control-subtracted execution is 1.40792 G current versus 1.45267 G base per plain run, -3.080% (paired range -3.103%..-3.068%), and 19.1659 G versus 19.2109 G per LTM run, -0.234% (-0.260%..-0.215%); every pair improves. The production-derived matrix crosses both source capture kinds with computed scalar, dynamic-element, array-valued and initial-backed storage; explicit, module, macro and typed/generated-LTM routes; both intrinsic orders of phase union; live cross-phase promotion and dead INIT-only dependency chains; local and qualified INIT-referent propagation with sparse child outputs; implicit runlists; exact fragments and opcodes; active initialization, reset and rerun; and user-variable VM/wasm parity. Validation: engine lib 5,693 passed / 2 ignored; integration 776 passed / 16 ignored, including genuine-output, VM/wasm and LTM corpora; VM allocation 4 passed; doctests 2 passed / 1 ignored; release C-LEARN LTM parity is green; both determinism modules pass 12 consecutive invocations; all-target/all-feature engine clippy with warnings denied, rustfmt and `git diff --check` are green |
| 7.5a | `engine: read static element snapshots directly` | Against the exact saved Phase 6b binary, five paired control-subtracted rounds put one plain compile at 1.14007 G instructions current versus 1.14676 G base, -0.583% (paired range -0.838%..-0.492%), and one LTM compile at 13.4423 G versus 13.5152 G, -0.539% (-0.962%..-0.296%); both are below the plan's 1% investigation threshold | 5189 plain / 30375 LTM | plain 30762 / 1477 / 24837; LTM 906415 / 1477 / 28693 | plain 1740 / 162 / 28 / 739; LTM 17027 / 162 / 28 / 2850 | Static bare and qualified element indices in source PREVIOUS/INIT calls use one production `SnapshotIndexResolver`. Salsa and the datamodel oracle derive only the referenced axis and qualified 1-based position; `dimensions::resolve_snapshot_axis_index` owns the final decision through the canonical name/position helpers. XMILE footnote 9 makes an owning-axis bare element win over a same-named source variable, while generated LTM equations retain their conservative whole-generated-surface shadowing rule. A qualified position can come from an unrelated dimension. The per-qualified-name salsa projection reexecutes after any dimension-vector edit but backdates an unchanged result before parse; moving or removing the selected element invalidates parse. The exhaustive production matrix crosses both intrinsics with same-name collision, unrelated qualification, missing qualified/bare names, globally ambiguous bare names, mapped axes and a proper subdimension; the missing and mapped-target-only arms capture then refuse loudly. The source-reachable dual `SpansDimension`/`Static` row keeps spans-first addressability, exact source locations and zero captures, then lowers to two direct element-first reads with pinned VM values for both intrinsics. C-LEARN's corrected profile is identical to the initial 7.5a profile, so it contains no affected same-name collision: plain removes 26 captures, 52 flow opcodes and 78 initial opcodes from Phase 6b (57,206 -> 57,076 total; initials 1,190 -> 1,164), with every table count unchanged. Under LTM those captures disappear too, while 35 scalar/per-element link-score names coalesce into dimensioned score variables whose declared extent is +278 slots: net slots +252, literals +278, flow opcodes -1,802 and initial opcodes -78 (938,465 -> 936,585 total; initials 1,960 -> 1,934). The post-fusion flow count is 17,063 -> 17,037 plain and 419,166 -> 418,560 LTM. The integration guard pins 7,128 LTM variables and the 30,375-slot ceiling distance. A production-derived fixture pins the same mechanism without C-LEARN names: one two-slot `rate[a1]→grow` score coalesces bare+qualified reads, same-named `rate[b2]` is a second two-slot direct score, and the one dynamic callsite retains two scalar scores; exact user and score series are preserved. Release C-LEARN user series are identical with LTM off and on. Control-subtracted execution is 1.45260 G current versus 1.49290 G base per plain run, -2.699% (five paired rounds -2.735%..-2.684%), and 31.9781 G versus 32.6132 G per LTM run, -1.947% (all five pairs improve, -2.093%..-1.403%). AC3 additionally adds an unrelated variable and a second SMTH1 through production sync: only each new variable parses, and the existing bare-element probe and fragments remain memoized. Validation: engine lib 5,681 passed / 2 ignored; integration 776 passed / 16 ignored, including genuine-output, VM/wasm and LTM corpora; VM allocation 4 passed; doctests 2 passed / 1 ignored; both determinism modules pass 12 consecutive invocations; all-target/all-feature engine clippy with warnings denied, rustfmt and `git diff --check` are green |
| 6b | `engine: materialize arrays after subscript resolution` | The last five-round interleaved measurement is 8.6836 G instructions on the whole-process channel against the exact `c666c4a6` base at 8.5902 G, +1.087%. Isolating five extra compiles by subtracting otherwise identical `CLEARN_COMPILE_ITERS=0` runs from `=5` measures 1.1475 G per compile against 1.1487 G, -0.103%. The remaining cost is execution: the plain `CLEARN_RUN_ITERS=20` channel measures 32.8113 G against 30.7036 G, +6.865%, or +105.39 M instructions per VM construction/run. For LTM, five alternating exact-base/current rounds each pair `RUN_ITERS=20` with its binary's zero control: one added run averages 19.4937 G base versus 19.6967 G current, +1.0413% or +203.0 M instructions/run; every paired delta is positive, range +1.0095%..+1.0715%. Phase 6b does not recover batched whole-array execution. The regressions are accepted temporarily because the owner sequenced loop-form/batched lowering after compiler cleanup, not because they are small. Compile allocation count falls 1,996,628 -> 1,989,055 and allocated bytes fall 245.0 -> 223.0 MiB. The residual plain definitions are the four `sorted_target_*` equations: each of their seven COP coordinates has a distinct final source slice, so the final-expression cache cannot group them. Removing that work at the final materializer would require recovering one pre-resolution iteration-shaped definition, which is the probe/early-hoist ownership this phase excludes; loop-form lowering is the known general alternative, and the GH #1025 handoff above makes batched array-producing builtins part of its acceptance scope | 5215 plain / 30123 LTM | plain 30814 / 1477 / 24915; LTM 908217 / 1477 / 28771 | plain 1740 / 162 / 28 / 739; LTM 16749 / 162 / 28 / 2850 | Plain C-LEARN has 57,206 total opcodes, +120 flow and +120 initial against `c666c4a6`. Each phase adds exactly 24 `VectorElmMap`, 48 `PushStaticView` and 48 `PopView`: every extra map consumes its source and offset views. LTM has 938,465 total, -160 flow and +120 initial; its flow removes 32 maps and 64 of each view opcode while its initial phase adds the same 24 maps and 48 of each view opcode as plain, for a net -8 `VectorElmMap`, -16 `PushStaticView` and -16 `PopView`. Static-view table counts therefore move +96 plain and -16 LTM. Plain temp slots remain 28; LTM temp slots fall from 441 to 28. Stock opcodes, slots, literals, GFs, dimensions, names, modules, initial counts, every user-model series and VM/wasm parity are unchanged. `compiler::array_operand` is the one post-subscript materializer; its exact-definition cache reuses only the allocator's same physical slot; `TempAllocator` is the one allocator; resolved SCC assembly refuses cross-segment temp liveness. Validation: engine lib 5,676 passed / 2 ignored, integration 776 passed / 16 ignored, VM allocation 4 passed, doctests 2 passed / 1 ignored; the materializer invariant/varying/repeated-axis counts, SCC refusal/local control, mapped VM/wasm, per-element-GF, fragment-characterization and 12-repeat determinism matrices are green. Workspace all-target/all-feature clippy with warnings denied, rustfmt, retired-helper zero-searches and `git diff --check` are green |
| 7.4 | `engine: parse source without model context` | 8.5872 G (median of 5; range 8.5869-8.5901), -1.60% against the saved 7.3b binary re-measured in the same session (8.7266 G, median of 5; range 8.7167-8.7307). An interleaved five-round wall-clock comparison measured the compile loop at -0.6%, inside that channel's noise floor | 5215 | 30694 / 1477 / 24795 | 1740 / 162 / 28 / 643 | C-LEARN adds 137 initial opcodes, 16 initial fragments and 8 literals against 7.3b, both plain (56,966 total opcodes, 1,190 initials) and under `CLEARN_LTM=1` (938,505 total, 1,960 initials, 16,749 literals); the correction seeds qualified child outputs that callers read during initialization. Flow/stock opcodes, slots, names, modules, temps, views, the complete post-fusion flow profile and every user-model series are unchanged. `parse_source_variable(var, project)` is the one source parse query; `ModuleIdentContext`, the model-context constructors, contextual/empty twin parses, D2's pre-parse equation probes and LTM's parallel module-ident set are absent. The AC3.1 first-instance probe executes explicit `[delay_time, flow, initial_value, input, k, output, smoothed]`, its two helpers and six distinct parses; `k` legitimately gains initial bytecode through the new module's initial closure and `probe` stays cached. A second SMTH1 executes/parses only `smoothed2` and its two new helpers. Production-derived D1 rows pin qualified scalar, array-element and nested ports by VM value; same-key unions within and across roots, distinct input sets, active-initial, stock, implicit-module and resolved-SCC rows pin the cross-model initial fixed point; wasm pins parity and an absolute value; and PREVIOUS-only plus unrelated child outputs remain absent. Bound module inputs and direct or LTM bare-module reads refuse both PREVIOUS and INIT loudly with attributed diagnostics instead of reading flattened slot zero. The fixed point consumes pure dependency facts: one per-model trigger emits at most one attributed cycle diagnostic, so a valid main neither inherits nor duplicates an unrelated draft's cycle. The execution probe runs all 11 initial graph keys once each; after an unrelated revision neither graph nor cycle-emitter body reruns, while the eleven `model_all_diagnostics` bodies replay the cached single draft row for whole-project and CLI collection. DFS roots and successors are both canonical, so the first attribution is stable across declaration rotations, fresh databases and revisions; this changes only `Diagnostic.variable` for an unresolved graph, never a compilable artifact. Macro formal parameters retain typed-registry-derived captures. D3's generated-LTM three-way shadowing stays pinned through `ltm_model_var_names`; general source classification remains 7.5. Validation: engine lib 5,699 passed / 2 ignored, integration 776 passed / 16 ignored (genuine-output, VM/wasm and LTM corpora included), VM allocation 4 passed, doctests 2 passed / 1 ignored, release C-LEARN LTM parity green, both determinism modules pass 12 consecutive invocations, and the qualified-INIT VM row is green through `run_initials`, reset, a second `run_initials` and the complete run; legacy-symbol and graph-accumulator zero-searches, all-target/all-feature clippy, rustfmt and `git diff --check` are green. The accumulator-ownership and DFS-root-order corrections do not alter bytecode-producing logic, seed sets or SCC verdicts, so the artifact and performance measurements were not repeated after those review-only fixes |
| 7.3b | `engine: centralize macro call routing` | 8.7284 G (mean of 5, +/-0.02%), +0.04% against the saved 7.3a binary re-measured in the same session (8.7249 G, mean of 5, +/-0.03%), inside the channel's noise floor and not investigated | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical to 7.3a on C-LEARN, plain and under `CLEARN_LTM=1`: 56,829 total opcodes plain and 938,368 with LTM, including the complete top-25 histogram, post-fusion totals and fused-binop tables, 371 names and 7 modules; the release LTM run also preserves every user-model series. `MacroRegistry::resolve_call` is the one exhaustive decision read by expansion and recursion analysis, with explicit `Expand`, `Passthrough`, `RenamedBuiltinSelfCall` and `Unresolved` arms; both registered-macro outcomes retain a descriptor long enough for one visitor-boundary exact-arity check, while renamed self-calls deliberately use builtin arity; `ImplicitVar::variable_stage0` is the one typed conversion at all seven consumers, including both LTM sites. The production MDL/salsa parse row pins the helper order and exact typed shape for ordinary expansion, external INIT passthrough, the INIT macro body's renamed-intrinsic self-call and the DELAYN macro body's renamed-stdlib self-call. Production MDL `DELAY1` and XMILE `PREVIOUS` passthrough rows derive their one-argument contract from the imported descriptor, compile valid unary calls, and attribute builtin-acceptable binary calls to the caller as `BadBuiltinArgs`/`NotSimulatable`. The compile-stage text audit finds no implicit-module helper reparse; the intentional source/diagnostic and LTM-generator boundaries are recorded in the Phase 7.3b note. `ModuleIdentContext` remains wholly owned by 7.4. The arity correction changes only declared-invalid macro calls, so the valid-route artifact, release-LTM, and performance gates were not repeated. Validation: engine lib 5691 passed / 2 ignored, integration 776 passed / 16 ignored, VM allocation 4 passed, doctests 2 passed / 1 ignored; the focused macro, capture, implicit-module, four-constructor fragment-input and 14-test determinism suites are green; the ignored release `clearn_with_ltm_simulates_model_vars_identically` gate is green; clippy with all targets/features and rustfmt are green |
| 7.3a | `engine: implicit modules carry parsed data` | 8.7253 G (median of 5; range 8.7159-8.7290), +0.33% against the base tree (the 7.2 chunk staged on `8b9ef311`) re-measured in the same session (8.6966 G, median of 5; range 8.6857-8.7009; interleaved pairs +0.456 / +0.372 / +0.484 / +0.173 / +0.223%). Each binary's spread is about 0.15-0.17%, so the delta is outside the channel's noise but below the one-percent threshold for investigation. A current-artifact verification measured 8.7213 G (mean of 5, +/-0.02%). `size_of::<ImplicitVar>()` remains 144 bytes because `Capture` is still the largest variant; a later performance pass owns the residual | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | C-LEARN artifacts remain identical, plain and under `CLEARN_LTM=1`: 56,829 total opcodes plain and 938,368 with LTM, including the full histograms, post-fusion tables, 371 names, and 7 modules. A call is a typed `capture::ImplicitModule` plus one `capture::HoistedArg` per non-identifier argument; the old `ImplicitVar::Synthesized` text carrier is absent. `ImplicitModule` derives its ident and port destinations from logical callsite inputs `(parent, id, call_name, suffix)` plus source-to-port wiring, while current resolution remains name-keyed through `ImplicitVarMeta::{name,index_hint}`. Production-derived stdlib and macro tests require exact nonzero-span `Expr0` subtree identity. The complete ordered 3x3 `ImplicitVar` pair matrix pins same-arm dedup and all six loud cross-arm collisions; a production ACTIVE INITIAL fixture pins the reachable hoisted-argument/capture collision through parsing, diagnostic collection, and compile refusal. Within one visitor, `insert_implicit_var` is the only map mutation and refuses conflicting definitions before `IndexMap` can replace the first; the production `ARG1(k, k * 2)` macro regression pins the module/second-argument collision through diagnostic collection and compile refusal. `expand_module_function` serves stdlib and macro calls, so 7.3b retains only macro passthrough and GH #554. `print_eqn` is absent from `builtins_visitor.rs`; `make_temp_arg` is absent from `src/simlin-engine/src`; `substitute_dimension_refs` remains the AST-to-AST pre-hoist source-semantics rewrite documented above. The base/working-tree differential sweep over all 509 models found 396 byte-identical and 110 identically refused; the three nondeterministic models (`arrays_cname`, `arrays_varname`, `subscript_transposition`) produced the same two-output set on both binaries across 12 resamples, so no model moved. Current validation: engine lib 5688 passed / 2 ignored, integration 776 passed / 16 ignored, VM allocation 4 passed, doctests 2 passed / 1 ignored; focused implicit-module, capture, macro, fragment-determinism, and stage suites are green; clippy with all targets/features and rustfmt are green |
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
| 7.2 | `engine: captures carry their argument, not its text` | 8.6920 G (median of 5; range 8.6866-8.6976), -0.02% against the base tree (the 7.1 chunk staged on `68774a16`) re-measured in the same session (8.6937 G, median of 5, range 8.6834-8.6979; interleaved pairs -0.017 / +0.163 / -0.047 / -0.038 / -0.017%), inside the channel's noise floor and not investigated | 5215 | 30694 / 1477 / 24658 | 1732 / 162 / 28 / 643 | artifacts identical on C-LEARN, plain and under `CLEARN_LTM=1`: the whole `bytecode_profile` block is byte-identical in both modes -- every count above, the 371 names and 7 modules, the full opcode histogram, the post-fusion stream counts and the fused-binop table; same channel and flags as the baseline row. A representation change with the observable result held fixed, so no saving is expected and none is found. The exact per-capture delta at each of the six consumers that build a helper's parse-stage form is: a lex-and-parse of the helper's equation text is deleted, and one `print_eqn` (the `Variable::eqn` field is source text by definition) plus one `Expr0` subtree clone replaces it. The `instantiate_implicit_modules` walk is NOT part of the delta -- `parse_var` ran it on the old path too, and `Capture::variable_stage0` runs it for the same reason. On a model with 233 captures among 5215 slots that trade is a wash. `PREVIOUS`/`INIT` arguments are now `capture::Capture` values -- an `Expr0` subtree whose logical callsite inputs are `(parent, id, arg0, suffix)` -- carried on the parse result in a `capture::ImplicitVar` list beside the module instances and hoisted call arguments that are still text; `Capture::variable_stage0` is the one constructor of a capture's parse-stage variable, and `capture::synthetic_ident` the one derivation of every synthesized helper's name (`rg "arg0" src/simlin-engine/src` finds one production site). `ImplicitVar::Synthesized` is boxed, which is what keeps the enum the size of a capture rather than of a `datamodel::Variable` in a list salsa retains per variable and, under LTM, per synthetic variable. Differential sweep of the base-tree and working-tree CLIs over all 509 models under `test/`, each run twice per binary: 396 byte-identical, 110 refused identically, and the only three non-identical models are `arrays_cname`, `arrays_varname` and `subscript_transposition`, each resampled 12x per binary and each producing the SAME two-output set on both binaries (GH #859, the importer nondeterminism). No model moved. The engine suite (lib 5677, integration 776, CLI 4, wasm 2), the 12-repeat determinism suites and every fragment/LTM golden are green with no regeneration |
