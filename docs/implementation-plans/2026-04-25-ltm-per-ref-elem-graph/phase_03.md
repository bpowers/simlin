# Phase 3 — Per-Shape Partial Equations and Link Scores

**Goal:** Per AST reference site, build a partial equation that leaves matching-shape references live and wraps everything else in `PREVIOUS()`. Emit one `LtmSyntheticVar` per `(from, to, RefShape)` tuple. Phase 1's red tests for AC2.1, AC2.2, AC2.3 turn green.

**Architecture:** Replace `wrap_deps_in_previous`'s flat `HashSet<Ident>`-based identification with a shape-aware AST transform that walks the equation's `Expr0` tree and decides per-node whether to wrap. Thread `RefShape` through `link_score_equation_text` and the link-score generators. Add a fourth, shape-aware variant to the `(Bare | FixedIndex | Wildcard | DynamicIndex)` link-score emission in `model_ltm_variables`.

**Tech Stack:** Rust, salsa-tracked `link_score_equation_text`, `Expr0` AST transformation, `LtmSyntheticVar` emission.

**Codebase verified:** 2026-04-25 (Phase 3 codebase-investigator confirmed: `build_partial_equation` is private and called from `generate_auxiliary_to_auxiliary_equation:322` and `generate_stock_to_flow_equation:426`; flow-to-stock uses fixed structural formula with no AST; `parse_link_offsets:274-332` already handles bracketed from/to as scalar element-level entries via `contains('[')` check; `Link` struct has no shape field today; `generate_loop_score_equation:454-475` constructs link names from `from`/`to` strings only).

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### ltm-per-ref-elem-graph.AC2: Per-shape partial equations are correct
- **ltm-per-ref-elem-graph.AC2.1 Bare-shape partial holds wildcard at PREVIOUS:** For `share[R] = population / SUM(population[*])` and the link score keyed by `(population, share, Bare)`, the partial must leave the bare `population` reference live and wrap the `population[*]` inside the SUM in `PREVIOUS()`.
- **ltm-per-ref-elem-graph.AC2.2 Wildcard-shape partial holds bare at PREVIOUS:** For the same equation, the link score keyed by `(population, share, Wildcard)` must wrap the bare `population` in `PREVIOUS()` and leave the wildcard reducer live.
- **ltm-per-ref-elem-graph.AC2.3 FixedIndex per-element partials:** For `migration_pressure[NYC] = (pop[NYC] - pop[Boston]) * 0.01`, partial for `(pop, mp, FixedIndex(NYC))` is `(pop[NYC] - PREVIOUS(pop[Boston])) * 0.01`; partial for `(pop, mp, FixedIndex(Boston))` is `(PREVIOUS(pop[NYC]) - pop[Boston]) * 0.01`.
- **ltm-per-ref-elem-graph.AC2.4 Other-source refs still wrapped:** Every reference to a variable other than the link's `from` is wrapped in `PREVIOUS()` regardless of `RefShape`.

### ltm-per-ref-elem-graph.AC3: Link score variables track shapes
- **ltm-per-ref-elem-graph.AC3.1 Per-shape link score emission:** When a target equation references a source under multiple distinct `RefShape`s, `model_ltm_variables` emits one `LtmSyntheticVar` per `(from, to, RefShape)` tuple.
- **ltm-per-ref-elem-graph.AC3.2 FixedIndex naming convention:** FixedIndex link scores use the existing `$⁚ltm⁚link_score⁚{from}[{elem}]→{to}` naming.
- **ltm-per-ref-elem-graph.AC3.3 Bare and Wildcard share existing names:** When only one shape is present, the canonical un-suffixed name is used. When both Bare and Wildcard appear for the same `(from, to)` pair, a deterministic disambiguation rule emits two distinct names. (See Task 5 for the decision.)

---

## Implementation Tasks

The phase begins with the partial-equation builder upgrade (Tasks 1–3), pivots `link_score_equation_text` and emission (Tasks 4–6), and ends with discovery-parser compatibility verification (Task 7).

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Shape-aware AST transform — `wrap_non_matching_in_previous`

**Verifies:** infrastructure for AC2.1, AC2.2, AC2.3, AC2.4

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs`

**Implementation:**
Add a new function alongside `wrap_deps_in_previous` (which stays for legacy callers but is removed at the end of Phase 3):

```rust
/// Walk an Expr0 tree and wrap variable references in PREVIOUS() except
/// those whose access shape matches the live shape for the given source.
///
/// `live_source` identifies the source variable whose live shape is held
/// out from PREVIOUS wrapping. `live_shape` declares which AST occurrences
/// of that source remain live; all other occurrences (and all references
/// to other sources in the same expression) are wrapped.
///
/// `other_deps` is the set of canonical idents for non-`live_source`
/// dependencies that must be wrapped (used to identify which `Var`/`Subscript`
/// nodes to wrap; nodes referencing names not in this set are left alone,
/// e.g., function names like `MAX` or constants).
fn wrap_non_matching_in_previous(
    expr: Expr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    other_deps: &HashSet<Ident<Canonical>>,
) -> Expr0
```

Recursion rules per Expr0 variant:

- `Const(..)`: return unchanged.
- `Var(ident, loc)`: canonicalize ident.
  - If ident == live_source: if live_shape is `Bare`, leave live (return unchanged). Otherwise wrap in PREVIOUS (this Var occurrence does not match the live shape — e.g., live shape is FixedIndex, but here we have a bare Var).
  - If ident is in other_deps: wrap in PREVIOUS.
  - Otherwise (a function name or unknown identifier): leave unchanged.
- `Subscript(ident, indices, loc)`: canonicalize ident.
  - First, recursively transform each `IndexExpr0::Expr` and `IndexExpr0::Range` inner expression (subscripts can contain dependencies in their indices, e.g., `arr[other_var]`).
  - If ident == live_source: classify the subscript's shape (Wildcard / FixedIndex(elems) / DynamicIndex). If the classified shape matches `live_shape`, leave live. Otherwise wrap.
  - If ident is in other_deps: wrap in PREVIOUS.
  - Otherwise: leave unchanged.
- `App(UntypedBuiltinFn(name, args), loc)`: recursively transform each arg. Function name itself is never matched as a variable.
- `Op1`, `Op2`, `If`: recurse into all children.

**Subscript shape classification helper (mirrors Phase 2's logic for Expr2 but at Expr0 level):**

```rust
fn classify_expr0_subscript_shape(
    indices: &[IndexExpr0],
    source_dim_elements: &[Vec<String>],
) -> RefShape {
    if indices.iter().any(|idx| matches!(idx, IndexExpr0::Wildcard(_))) {
        return RefShape::Wildcard;
    }
    let mut elems = Vec::with_capacity(indices.len());
    for (i, idx) in indices.iter().enumerate() {
        match idx {
            IndexExpr0::Expr(Expr0::Var(name, _)) => {
                let canon = canonicalize(name).into_owned();
                if i < source_dim_elements.len()
                    && source_dim_elements[i].iter().any(|e| e == &canon)
                {
                    elems.push(canon);
                } else {
                    return RefShape::DynamicIndex;
                }
            }
            IndexExpr0::Expr(Expr0::Const(s, _, _)) => {
                // Possibly an integer literal index; map to Indexed-dimension element name.
                if let Ok(n) = s.parse::<u32>() {
                    elems.push(n.to_string());
                } else {
                    return RefShape::DynamicIndex;
                }
            }
            _ => return RefShape::DynamicIndex,
        }
    }
    RefShape::FixedIndex(elems)
}
```

The function takes `source_dim_elements` so it can validate literal index names against the source's actual elements. If a "literal" index doesn't match any element name AND isn't a parseable integer, classify defensively as `DynamicIndex`.

**Visibility:** Make the new function `pub(crate)` so the partial-equation tests in Phase 1 can call it directly. Keep `wrap_deps_in_previous` private; it's removed at the end of Phase 3.

**Testing:**
None directly in this task — the partial-equation tests added in Phase 1 (Tasks 4–6) and re-activated below in Task 3 cover the contract.

**Verification:**
Run: `cargo build -p simlin-engine`. Expected: clean build.

**Commit:** `engine: shape-aware Expr0 transform wrap_non_matching_in_previous`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Shape-aware partial-equation builder — `build_partial_equation_shaped`

**Verifies:** infrastructure for AC2.1, AC2.2, AC2.3

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs`

**Implementation:**
Add a new entry point alongside `build_partial_equation` (which stays for one-shape callers; refactored or deleted at end of Phase 3):

```rust
pub(crate) fn build_partial_equation_shaped(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
) -> String {
    let other_deps: HashSet<Ident<Canonical>> = deps
        .iter()
        .filter(|d| {
            *d != live_source && normalize_module_ref(d) != *live_source
        })
        .cloned()
        .collect();

    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return equation_text.to_lowercase();
    };

    let transformed = wrap_non_matching_in_previous(
        ast,
        live_source,
        live_shape,
        &other_deps,
    );
    print_eqn(&transformed)
}
```

Note: even when `live_shape` is `Bare` and there are no other deps, the function still parses and re-prints the equation to canonical form. The legacy `build_partial_equation` returned `equation_text.to_lowercase()` as a fast path when no wrapping was needed; the new function does not, because the result must always be canonicalized for downstream parsing. (The performance impact is negligible — these equations are short.)

`source_dim_elements` is `&[Vec<String>]`: one vec per source dimension, each containing the dimension's element names in source order. The caller (the link-score generator) builds this from `variable_dimensions(db, source_var, project)` via `dimension_element_names`.

**Testing:**
Phase 1 added `test_partial_equation_share_bare_shape`, `test_partial_equation_share_wildcard_shape`, `test_partial_equation_migration_pressure_fixed_nyc`, and `test_partial_equation_migration_pressure_fixed_boston` with `#[ignore]`. Remove the `#[ignore]` from each. The tests now compile (Task 1 introduced the API) and should pass.

To make the tests work, they need access to `source_dim_elements`. Add a small helper in the test module:

```rust
#[cfg(test)]
fn region_dim_elements() -> Vec<Vec<String>> {
    vec![vec!["nyc".to_string(), "boston".to_string()]]
}
```

(For the `share = pop / SUM(pop[*])` tests, only one dim with two elements is needed.)

The expected partial-equation strings depend on `print_eqn`'s exact output. To finalize:
1. Initially write each test with a placeholder expected string (e.g., `"PLACEHOLDER"`).
2. Run with `--nocapture` and a `dbg!(&partial)` to see the printed form.
3. Lock in the exact string.

Expected strings (subject to `print_eqn` confirmation):

- `test_partial_equation_share_bare_shape`: `population / PREVIOUS(SUM(population[*]))` (the Bare ref live; Wildcard ref wrapped).
- `test_partial_equation_share_wildcard_shape`: `PREVIOUS(population) / SUM(population[*])` (Wildcard live; Bare wrapped).
- `test_partial_equation_migration_pressure_fixed_nyc`: `(population[nyc] - PREVIOUS(population[boston])) * 0.01` (after canonicalization).
- `test_partial_equation_migration_pressure_fixed_boston`: `(PREVIOUS(population[nyc]) - population[boston]) * 0.01`.

**Verification:**
Run: `cargo test -p simlin-engine --lib partial_equation`. Expected: all four un-ignored tests pass.

**Commit:** `engine: build_partial_equation_shaped with per-shape PREVIOUS wrapping`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Direct unit tests for shape-aware partial equation

**Verifies:** AC2.1, AC2.2, AC2.3, AC2.4

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` (`#[cfg(test)] mod tests` block)

**Implementation:**
In addition to the four tests carried over from Phase 1, add two more direct tests that lock in AC2.4:

```rust
#[test]
fn partial_equation_other_source_always_wrapped() {
    // Equation has a reference to `helper` (other dep) plus the live source `pop`.
    // The `helper` reference must be wrapped regardless of shape.
    let deps = deps_set(&["pop", "helper"]);
    let live = Ident::new("pop");
    let shape = RefShape::Bare;
    let dims = vec![vec!["nyc".to_string(), "boston".to_string()]];

    let partial = build_partial_equation_shaped(
        "pop * helper",
        &deps,
        &live,
        &shape,
        &dims,
    );
    assert!(partial.contains("PREVIOUS(helper)"), "partial: {}", partial);
    assert!(!partial.contains("PREVIOUS(pop)"), "partial: {}", partial);
}

#[test]
fn partial_equation_unknown_ident_unchanged() {
    // A reference to a variable not in deps (e.g., a typo or external) is
    // left alone -- it's not a known dep and shouldn't be wrapped.
    let deps = deps_set(&["pop"]);
    let live = Ident::new("pop");
    let shape = RefShape::Bare;
    let dims = vec![vec!["nyc".to_string(), "boston".to_string()]];

    let partial = build_partial_equation_shaped(
        "pop + unknown",
        &deps,
        &live,
        &shape,
        &dims,
    );
    assert!(partial.contains("unknown"), "partial: {}", partial);
    assert!(!partial.contains("PREVIOUS(unknown)"), "partial: {}", partial);
}
```

**Verification:**
Run: `cargo test -p simlin-engine --lib partial_equation`. Expected: all 6 tests pass (the 4 from Phase 1 plus these 2).

**Commit:** `engine: tests for AC2.4 (other-source refs wrapped, unknown idents passthrough)`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->

<!-- START_TASK_4 -->
### Task 4: Per-shape link score naming convention — design and document

**Verifies:** AC3.2, AC3.3 (the naming decision is locked in here; emission lands in Task 5)

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` (add naming helper)
- Document the decision in `docs/design/ltm--loops-that-matter.md` Phase 6 (deferred — only the helper is added in this phase)

**Implementation:**
Add a helper function in `ltm_augment.rs` that, given `(from, to, RefShape)`, produces the link score variable name. The decision rule:

- **Bare**: canonical name, no suffix. `$⁚ltm⁚link_score⁚{from}→{to}`. This is today's name; if the target's only reference to `from` is Bare, no behavior change.
- **FixedIndex(elems)**: per-element prefixed-from naming. `$⁚ltm⁚link_score⁚{from}[{elem_joined}]→{to}` where `elem_joined` is comma-separated. Already-existing convention.
- **Wildcard**: when this shape coexists with `Bare` in the same `(from, to)`, suffix: `$⁚ltm⁚link_score⁚{from}→{to}⁚wildcard`. When `Wildcard` is the only shape, use the canonical name.
- **DynamicIndex**: same pattern as `Wildcard`. Suffix `⁚dynamic` when coexisting with `Bare`; canonical name otherwise.

Helper:
```rust
pub(crate) fn link_score_var_name(
    from: &str,
    to: &str,
    shape: &RefShape,
    has_collision: bool,
) -> String {
    let from_part = match shape {
        RefShape::FixedIndex(elems) => format!("{}[{}]", from, elems.join(",")),
        _ => from.to_string(),
    };
    let to_part = match (shape, has_collision) {
        (RefShape::Wildcard, true) => format!("{}\u{205A}wildcard", to),
        (RefShape::DynamicIndex, true) => format!("{}\u{205A}dynamic", to),
        _ => to.to_string(),
    };
    format!("$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}", from_part, to_part)
}
```

The `has_collision` parameter is computed by the caller in Task 5: it's true iff the same `(from, to)` pair has more than one distinct shape AND `Bare` is among them. The Bare variant always claims the canonical name; the others get suffixed.

**Testing:**
Add a few unit tests in `mod tests`:
```rust
#[test]
fn link_score_name_bare_canonical() {
    assert_eq!(
        link_score_var_name("pop", "births", &RefShape::Bare, false),
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births"
    );
}

#[test]
fn link_score_name_fixed_index() {
    let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
    assert_eq!(
        link_score_var_name("pop", "rel_pop", &shape, false),
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop"
    );
}

#[test]
fn link_score_name_wildcard_with_collision() {
    assert_eq!(
        link_score_var_name("pop", "share", &RefShape::Wildcard, true),
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\u{205A}wildcard"
    );
}

#[test]
fn link_score_name_wildcard_without_collision_canonical() {
    assert_eq!(
        link_score_var_name("pop", "total", &RefShape::Wildcard, false),
        "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}total"
    );
}
```

**Verification:**
Run: `cargo test -p simlin-engine --lib link_score_var_name`. Expected: all 4 tests pass.

**Commit:** `engine: link_score_var_name helper with shape-driven naming`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Pivot `link_score_equation_text` and `model_ltm_variables` to per-shape emission

**Verifies:** AC3.1, AC3.2, AC3.3

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (`link_score_equation_text` at lines 2023–2137)
- Modify: `src/simlin-engine/src/ltm_augment.rs` (`generate_link_score_equation_for_link`, `generate_auxiliary_to_auxiliary_equation`, `generate_stock_to_flow_equation`)
- Modify: `src/simlin-engine/src/db_ltm.rs` (`model_ltm_variables` link-emission loops)

**Implementation:**

**5a. Salsa input for shape:** `LtmLinkId` (in `db.rs`, search for the salsa-tracked struct definition) currently holds `(link_from: String, link_to: String)`. To make `link_score_equation_text` differentiate per shape, either:
- (preferred) Add `shape: RefShape` to `LtmLinkId` so each `(from, to, shape)` is a distinct salsa key.
- (alternative) Add a new salsa-tracked function `link_score_equation_text_shaped(db, link_id, shape, ...)` that takes shape as an explicit argument.

The preferred approach makes salsa caching automatic. `RefShape` already derives `Clone`, `PartialEq`, `Eq`, `Hash` (Phase 2 requirement); add `salsa::Update` if salsa requires it.

**5b. Update `generate_link_score_equation_for_link`** to accept `shape: &RefShape` and `source_dim_elements: &[Vec<String>]`:

```rust
pub(crate) fn generate_link_score_equation_for_link(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String { ... }
```

The internal dispatch to `generate_auxiliary_to_auxiliary_equation` / `generate_stock_to_flow_equation` passes `shape` and `source_dim_elements` along; both call `build_partial_equation_shaped` instead of `build_partial_equation`. `generate_flow_to_stock_equation` ignores `shape` (it uses the structural formula, no AST parsing).

For Bare on a scalar source, `source_dim_elements` is empty (`vec![]`), and `wrap_non_matching_in_previous` falls back to identity behavior on Bare with no shape ambiguity.

**5c. Update `link_score_equation_text` in `db.rs`** to read shape from the new `LtmLinkId` field, look up the source's dimensions via `variable_dimensions(db, source_var, project)`, build `source_dim_elements`, then call the updated generator. Continue to return `Option<LtmSyntheticVar>` — but the `LtmSyntheticVar.name` is now built via `link_score_var_name(from, to, shape, has_collision)`. The `has_collision` decision happens in `model_ltm_variables` (Task 5d) because it's the only place that sees the full set of shapes for a given `(from, to)`.

To keep the salsa cache stable: pass `has_collision: bool` as a separate function arg if salsa-tracked, or compute the name in `model_ltm_variables` using the result's equation text from `link_score_equation_text`. The simplest is: `link_score_equation_text` returns `(equation, default_name_without_suffix)`; `model_ltm_variables` rewrites the name with `link_score_var_name(from, to, shape, has_collision)` after enumerating all shapes for the `(from, to)` pair.

**5d. Update `model_ltm_variables` link-emission loops** in both branches (discovery/sub-model at line 2422 and exhaustive at line 2441):

For each `(from, to)` pair the existing logic processes:
1. Try `try_cross_dimensional_link_scores` first (handles arrayed-source-to-scalar-target reducers — unchanged).
2. Otherwise, **collect all `RefShape`s** for which the target's AST references `from`. This requires reusing `collect_reference_sites` from Phase 2 (or introducing a sibling that returns just unique shapes).
3. For each unique shape:
   - Skip `Bare` if the source is scalar and target is also scalar (no edge expansion needed; falls into legacy logic).
   - Compute `has_collision = shapes.contains(&RefShape::Bare) && shapes.iter().any(|s| !matches!(s, RefShape::Bare | RefShape::FixedIndex(_)))`.
   - Call `link_score_equation_text` with the shape-aware `LtmLinkId`. Wrap the resulting `LtmSyntheticVar`:
     - `lsv.name = link_score_var_name(from, to, shape, has_collision)`
     - `lsv.dimensions = link_score_dimensions(...)` for Bare and Wildcard; for FixedIndex, use the target's dimensions if target is arrayed (the link score is A2A over the target dim) or empty if target is scalar (each FixedIndex(elem) emits a single scalar `LtmSyntheticVar`).

For the exhaustive path's loop iteration: `loop_item.links` doesn't carry shape today. Phase 3 must add `shape: Option<RefShape>` to `Link` or pass shape through a sidecar map. The simplest is to add `shape: Option<RefShape>` to `Link`; existing code that constructs `Link` without shape (`Link { from, to, polarity }`) becomes `Link { from, to, polarity, shape: None }`. Phase 4 fills in Some(shape) at loop-construction time. For Phase 3, the discovery/sub-model path (which iterates `edges_result.edges`) computes shapes per `(from, to)` directly and the loop-iteration path uses `link.shape.unwrap_or(RefShape::Bare)` as a fallback (Phase 4 fills in real values).

**Testing:**
Add an integration test in `db_ltm_unified_tests.rs` (or a new `db_ltm_per_shape_tests.rs`) that asserts:

```rust
#[test]
fn per_shape_link_scores_for_share_with_sum() {
    // share[R] = pop / SUM(pop[*]) emits two link score variables.
    let project = TestProject::new("share_sum")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .array_aux("share[Region]", "pop / SUM(pop[*])");
    // ... build LTM-augmented project ...
    let lsv_names: Vec<String> = ... // collected from model_ltm_variables result
    assert!(lsv_names.iter().any(|n| n == "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share"));
    assert!(lsv_names.iter().any(|n| n == "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\u{205A}wildcard"));
}
```

Also test the FixedIndex case:
```rust
#[test]
fn fixed_index_link_score_emits_per_element_name() {
    let project = TestProject::new("rel_pop")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .array_aux("rel_pop[Region]", "pop / pop[NYC]");
    let lsv_names: Vec<String> = ...;
    // Bare-shape link score (canonical name; A2A over Region):
    assert!(lsv_names.iter().any(|n| n == "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}rel_pop"));
    // FixedIndex(NYC)-shape link score (subscripted-from name; A2A over Region):
    assert!(lsv_names.iter().any(|n| n == "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop"));
    // Total: 2 distinct link scores for the (pop, rel_pop) pair.
    let pop_to_rel: usize = lsv_names.iter().filter(|n| n.contains("pop") && n.contains("rel_pop")).count();
    assert_eq!(pop_to_rel, 2);
}
```

**Verification:**
Run: `cargo test -p simlin-engine`. Expected: all tests pass; existing `simulate_ltm` tests pass (with documented updates per Phase 2 Task 5 if any).

**Commit:** `engine: per-shape link score emission in model_ltm_variables`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Loop-score generation resolves the right link score per shape

**Verifies:** AC3.1 (loop scores correctly multiply per-shape link scores)

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` (`generate_loop_score_equation` at lines 454–475)
- Modify: `src/simlin-engine/src/ltm.rs` (`Link` struct at lines 66–72)

**Implementation:**
Add `shape: Option<RefShape>` to `Link`. All existing constructors that build `Link` get default `shape: None`. Phase 4 will populate the shape at loop construction.

Update `generate_loop_score_equation` to use `link.shape` (defaulting to `RefShape::Bare` when None) and call `link_score_var_name(link.from.as_str(), link.to.as_str(), &shape, has_collision_for_link)`. The `has_collision_for_link` is true iff there's another `Link` in the same loop with the same `(from, to)` but a different non-`FixedIndex` shape — generally false in practice (a loop edge has a single shape), but the function must not crash on edge cases.

Practical rule: `has_collision_for_link` defaults to `false` for loop-score name resolution. The collision flag is only relevant when emitting two distinct `LtmSyntheticVar`s for the same `(from, to)`; from the loop's perspective, each edge corresponds to ONE shape, so there's no ambiguity in name lookup.

**Testing:**
Add a test in `ltm.rs::tests` or `ltm_augment.rs::tests` asserting that loop-score equations reference the correct link score names:
```rust
#[test]
fn loop_score_equation_references_fixed_index_link_score() {
    let loop_item = Loop {
        id: "r1".to_string(),
        links: vec![
            Link {
                from: Ident::new("pop"),
                to: Ident::new("rel_pop"),
                polarity: LinkPolarity::Positive,
                shape: Some(RefShape::FixedIndex(vec!["nyc".to_string()])),
            },
            // ... other links ...
        ],
        stocks: vec![],
        polarity: LoopPolarity::Reinforcing,
        dimensions: vec![],
    };
    let eq = generate_loop_score_equation(&loop_item);
    assert!(eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop\""));
}
```

**Verification:**
Run: `cargo test -p simlin-engine --lib generate_loop_score_equation`. Expected: pass.

**Commit:** `engine: loop_score equation references shape-resolved link score names`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_7 -->
### Task 7: Discovery parser compatibility with subscripted A2A link scores

**Verifies:** AC3.2 (discovery parser handles new names correctly)

**Files:**
- Modify: `src/simlin-engine/src/ltm_finding.rs` (`parse_link_offsets` at lines 274–332)

**Implementation:**
Today's `parse_link_offsets` treats any name with `[` in `from_str` or `to_str` as a single scalar element-level entry. After Phase 3, FixedIndex link scores can be A2A over the *target* dimension. For example, `$⁚ltm⁚link_score⁚pop[nyc]→rel_pop` is one variable with N slots (one per element of the target's `Region` dimension); each slot represents the link score for `(pop[nyc], rel_pop[d])` at element d.

The current parser emits a single `LinkOffset` for this name with `from = "pop[nyc]"` and `to = "rel_pop"`, ignoring the dimensions. Phase 3 must extend it to detect: if `from_str` contains `[` (FixedIndex marker) AND the `LtmSyntheticVar.dimensions` is non-empty (meaning A2A), emit one `LinkOffset` per element of the target dim, with `to = "{to_str}[{elem}]"`.

This piggybacks on the existing A2A expansion code that handles bare-source A2A link scores. Look at `parse_link_offsets` for the `dim_elements` expansion logic.

Implementation sketch:
```rust
// After existing if-branch on `from_str.contains('[') || to_str.contains('[')`:
if from_str.contains('[') && !ltm_var.dimensions.is_empty() {
    // Subscripted-from A2A link score: expand `to` into N element entries.
    // The `from` already carries its element subscript (e.g., "pop[nyc]").
    let dim_elements = expand_dimensions_to_elements(&ltm_var.dimensions, dims);
    for (slot_idx, elem) in dim_elements.iter().enumerate() {
        let from = Ident::new(from_str);
        let to = Ident::new(format!("{}[{}]", to_str, elem));
        link_offsets.push(((from, to), offset + slot_idx as u32));
    }
    continue;
}
```

(Adapt to existing helper names. `expand_dimensions_to_elements` may already exist or may be inlinable from existing logic.)

**Testing:**
Add a test in `ltm_finding.rs::tests` (or wherever `parse_link_offsets` is tested). Construct a synthetic `Vec<LtmSyntheticVar>` with one Bare A2A entry, one FixedIndex A2A entry, and one cross-dim per-element entry. Assert the resulting `LinkOffset` list has the right per-element expansion for each.

**Verification:**
Run: `cargo test -p simlin-engine --lib parse_link_offsets`. Expected: pass.

Run: `cargo test -p simlin-engine --test simulate_ltm test_cross_element_ltm_discovery`. Expected: discovery mode finds the correct loops on the cross_element fixture.

**Commit:** `engine: parse_link_offsets expands subscripted-from A2A link scores`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Phase 3 wrap-up — pre-commit hook end-to-end

**Verifies:** ltm-per-ref-elem-graph.AC5.2 (interim — pre-commit budget honored)

**Files:**
- None modified

**Implementation:**
Trigger the pre-commit hook by amending HEAD (no-op). Confirm:
- All Phase 3 tests pass
- All Phase 1 partial-equation tests have `#[ignore]` removed and pass
- `cargo test -p simlin-engine` runs under 180s
- Clippy clean

If `wrap_deps_in_previous` (the pre-Phase-3 builder) is now unused, delete it and `build_partial_equation` along with it. Address any orphaned imports.

**Verification:**
Run: `git commit --amend --no-edit`. Expected: pre-commit prints "All pre-commit checks passed!".

**Commit:** No new commit (this is a verification gate). If cleanup of legacy builder happens, a separate `engine: remove obsolete build_partial_equation` commit captures it.
<!-- END_TASK_8 -->

---

## Phase Done When

- All 7 implementation tasks (Tasks 1–7) committed; Task 8 verification gate passes.
- `wrap_non_matching_in_previous` and `build_partial_equation_shaped` exist as `pub(crate)` in `ltm_augment.rs`.
- `link_score_var_name` produces the correct names for Bare, FixedIndex, Wildcard (with and without collision), and DynamicIndex shapes.
- `model_ltm_variables` emits one `LtmSyntheticVar` per `(from, to, RefShape)` tuple; collision-affected pairs produce two distinct names.
- `Link` carries `shape: Option<RefShape>`; `generate_loop_score_equation` resolves the right link-score name per edge.
- `parse_link_offsets` correctly expands subscripted-from A2A link scores into per-element entries.
- Phase 1 partial-equation tests are all green (no `#[ignore]`).
- Pre-commit hook passes within 180s.
