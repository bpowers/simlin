# simlin-base

What every simlin crate is built on: the data model and the patches to it, identifiers, errors, dimensions, the equation lexer, parser and AST, the builtin functions, the standard library of module functions, and the results of a run.
It uses no other simlin crate.
The root `CLAUDE.md` holds the development rules; this file is a map of where functionality lives (paths are relative to this crate's `src/`) and of the standing invariants code in this crate keeps.
The compilation pipeline these pieces are the first stages of is in [the engine's map](/src/simlin-engine/CLAUDE.md).

## Data model and identifiers

- `datamodel.rs` -- the canonical serializable model: `Project`, `Model` (with optional `MacroSpec`), `Variable`, `Equation` (`Scalar`, `ApplyToAll`, `Arrayed` with an EXCEPT default), `Dimension` with `mappings`, views.
  `Project::default_model` is the one statement of the model a host runs when it names none: the model whose name is `main` (`canonical_model_name`, so `Main` and an unnamed model are it), else the first that is neither a macro nor a stdlib model.
  `Model.variables` and `StockFlow.elements` are `SharedVec`s, each element behind its own `Arc`, so a clone of a project (a host's undo copy) shares every variable and view element until one is edited.
  A `SharedVec` has no `iter_mut`, `IndexMut` or `&mut` iteration, so every mutation says what it touches and nothing unshares by accident: `get_mut`, `find_mut` and `replace` for one element, `edit_where` for the ones a predicate picks, `update` for a pass that can't tell beforehand what it changes (it reads each element and returns a replacement for the ones it changes, so it cannot change one and leave it out), and `rewrite` for a pass over a plain `Vec` that changes them all (imports, layout generation).
  No mutation's result depends on whether a copy shares the elements; `patch_sharing_tests.rs` holds every patch operation to that, and holds every patch operation and the conveyor and queue expansions to leaving each variable and view element they do not change in the allocation it was in (two models and three special-stock fixtures, one a driven flow already holding the placeholder `0`, in the default suite; the corpus for the patch operations in the gates).
  - `==` on the datamodel is "the same model", bit for bit: every float-bearing type compares its floats by their bits (`bitwise_eq!`, which destructures without `..` and refuses an unmarked float; `LinkShape` and `Dt` by hand), so every datamodel type is `Eq`, a value holding a NaN equals itself, and `0.0` is not `-0.0`.
    `SharedVec::share_identical` relies on it to put an old allocation back for an equal element of a view built afresh.
  - `Variable::expression_texts` (with `expression_texts_mut` and `map_expression_texts`, all three from one walk) is the one statement of where a variable holds text the engine parses as an expression: the equation's texts, an element's initial, `ACTIVE INITIAL`, and the conveyor and leak options (`ExpressionRole::Option`, one `StockOption` each, `StockOption::ALL`; `Compat::expression_texts` is the options' share).
    A pass over what a variable references (a rename, the special-stock scans, `model_reads`) takes the texts from it.
  - `Model::map_variable_names` (over `Variable::names` and `ViewElement::names`, each from one walk) is the one statement of where a model holds a variable's name outside an expression, each under its `NameRole`: a variable's own name, a stock's flow lists, a module reference's ends, a spread inflow's distribution (a variable's name or a list of numbers, never an expression), a group's members, a macro's parameters and outputs, and a diagram element's label; a loop names its variables by uid.
    Both walks destructure without `..`, so a field added to a model, a variable or a view element fails to compile until it is listed as an expression, a name or neither.
  - A stock's `inflows`/`outflows` hold what the producer wrote, a repeat included; `distinct_stock_flows` is the one statement of how they are read (each flow once, at its first occurrence -- the engine's rule, unverified against Stella and Vensim).
    Every reader that decides what the model does takes the set through it before reading: the salsa sync (which records the repeats for the `RepeatedStockFlow` advisory), `queue_compile::build_compiled` before either expansion, the layout metadata, the MDL writer's `INTEG`, and the patch ops, which store the set.
    The order of a list is the modeler's and every patch operation keeps it: XMILE 1.0 section 4.2 makes it the inflows' priority and a queue's outflow priority, and the queue and conveyor passes read it (`patch_flow_order_tests.rs`).
- `common.rs` -- `ErrorCode`, `Error`, `EquationError`, `Result`; the identifier types.
  An `EquationError` is `{start, end, code, details}`: the code names the CLASS of failure, the span points at the offending text (which `errors.rs` renders as a source snippet), and `details` carries the reason when it is not visible in the span -- the name that did not resolve, the arity a call missed, the reference whose dimensions did not fit.
  A parse error therefore writes no `details`: the snippet is its reason, and every Rust surface renders that snippet.
  A unit-definition error, whose units string has no equation to point into, adds that string to its reason, or to its code's description when it has none (`EquationError::in_context`), as in "the equation ends unexpectedly (in units 'kg/')".
  An `Error`'s `details` is likewise a complete phrase ("there is no model named 'x'", never a bare "x"), and an `Error` displays as `Error::reason`: its `details`, else `ErrorCode::description`, the plain-words meaning every code has.
  `ErrorCode`'s own `Display` is its snake_case name, for the terminal summaries.
  - Nothing between a raising site and `collect_all_diagnostics` drops the reason or puts something else in its place: `From<Error> for EquationError` carries `Error::details` across, the compiler's lowering entry points hand a rejection's `details` to the `Error` they build -- never its span, which is a byte offset into a string the user never sees -- and `UnitError::DefinitionError` wraps an `EquationError` rather than keeping a second reason field beside it.
  - `canonicalize` lowercases and normalizes a raw name; `Ident<Canonical>`, `CanonicalDimensionName`, and `CanonicalElementName` are interned, so clone is a refcount bump and equality is pointer equality.
    They hash as their string does, which is what lets a map keyed by one be probed with a `&str`; never hash one by its pointer or an id (the rustdoc on `CanonicalStorage` says what that breaks, and [engine performance](/docs/design/engine-performance.md) what it would buy).
    A map or set keyed by one, by a reference to one, or by a key made of them (`VarRef`, `ModuleKey`, a tuple) is `common::IdentMap` / `IdentSet`, never `HashMap`, `HashSet` or the `Fx` aliases; `scripts/lint-project.sh` refuses the other spellings.
    Sub-model variables are addressed as `module·var` (U+00B7).
  - `CanonicalElementName::from_subscript` / `from_parts` is the one owner of what a per-element subscript names (split on commas outside quotes, each part canonicalized, joined with `,`): every side of an element-arm match -- the arm keys, the declared-combination keys, the per-element gf tables, the conveyor init lists, the two arrayed-arm advisories, the readers -- goes through it.
    Never key one side by canonicalizing the whole subscript.
    `variable::elements_without_an_arm` is likewise the one owner of which declared elements have no arm (parsed arms plus per-element tables plus a live EXCEPT default).
- `results.rs` -- `Results` (offsets plus step-major series; `step_count` is the rows a run saved, each a step it evaluated) and `Specs`, the one statement of the step grid (pipeline stage 6): which steps a run takes, the time of each, and which of them it saves.
  `Results::print_tsv` prints one column per key of the results-offset map, in slot order; a slot the map has no key for is not printed.

## Arrays

- Subscripts are 1-based in source and 0-based internally.
  Dimensions are `dimensions::Dimension` (`Indexed` or `Named`); `DimensionsContext` holds subdimension relations, `maps_to` chains, and element-level mappings, and is the single statement of how a mapped read resolves (Vensim's rule, that a mapping is consulted for a subscript on the right of an equation and not on its left: `resolve_mapped_read`, for a reference naming the iterated dimension or none, reads each element by name where the source axis declares every iterated element, and otherwise through `read_through_mapping` -- the declared mapping, then the element's name on the source axis, then a mapped parent -- which is also how a subscript naming any other dimension reads, so `x[Younger]` under an `Older` iteration reads `Younger`'s mapped element on `x`'s axis and a chain between two overlapping subranges reads the element before, as Vensim's aging chain does; `resolve_dimension_subscript` chooses between the two by the dimension the subscript names and adds the one ordinal last resort, for a subscript that NAMES its active dimension over an axis declaring no correspondence to it, and both subscript paths -- static `build_view_from_ops`, dynamic `lower_index_expr3` -- resolve a dimension-named index through it, so a sibling index decides a reference's route and never its rule, GH #1044).
  `resolve_axis_index_name` is the single element-vs-dimension-name precedence for a bare identifier inside brackets (element first); the compiler and the LTM describers both read it.
- `dimensions::match_axes` is the single statement of how two axis lists pair: exact name, then declared mapping, then subdimension, then size, staged flat (each rule runs across every source axis before the next runs for any) and one-to-one, so a stronger match wins a target axis whatever the declaration order and no two source axes read the same one.
  - Every compile-time caller asks it -- the implicit subscripts a bare arrayed reference gets, the subscripts a bare reference under a reducer gets, the reordering that lines two operands up, `Expr2`'s bounds unification, the per-element resolution inside an apply-to-all body, and the view join -- and supplies an `AxisRelations` projection saying which declared relations it can act on: `DimensionsContext` answers the mappings; `DirectMappingsOnly` withholds the mapping-onto-a-parent rung from a caller that resolves the paired element by ORDINAL; `SubdimensionRelations` adds the subdimension rung for the one caller that never resolves an element through the answer; `NoAxisRelations` is what a caller comparing two `ArrayView`s can supply (`axis_match_tests` rows every arm).
  - `match_dimensions_two_pass` stays separate: it is the RUNTIME broadcast matcher, pairs `dim_id`s where no `DimensionsContext` exists, and states a deliberately weaker rule (id, then size for indexed dimensions).

## Invariants

- **Floats.** `float::NA` is Vensim's `:NA:`, the finite value `-2^109`, so `IF x = :NA:` works and `:NA:` arithmetic stays finite.
  A NaN is an absorbing failure signal, produced for out-of-range vector reads and empty reducers other than `SUM`.
  The module docs of `float.rs` are the landing page for this.

## Generated files

Do not edit: `src/stdlib.gen.rs` is generated from the repo-root `stdlib/*.stmx` (`pnpm rebuild-stdlib`) and carries a hash that a test checks.

The `test-support` feature compiles in the test helpers other crates' tests use; it is for `[dev-dependencies]` edges alone.
