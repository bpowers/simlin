# Phase 2 Task 1 spike: init combined-fragment representability

**Status: HARD GATE PASSED — a representable init mechanism exists.**

**Verdict (one sentence):** A combined init-phase recurrence SCC is emitted as
**one** `SymbolicCompiledInitial` carrying a synthetic ident, whose recompiled
bytecode contains every member's `member_base + elem` `AssignCurr` writes in the
SCC's `element_order` — exactly analogous to the flows combined fragment, with
no change to `resolve_module`, the VM init runner, or the variable layout.

**Codebase verified:** 2026-05-18, branch `clearn-hero-model`. Line numbers
below are the actual current numbers (Phase 1's `db.rs` edits shifted the phase
file's cited numbers by ~+155).

---

## Actual current line numbers for the key init structures

| Structure | Phase file cited | Actual current |
|---|---|---|
| `SymbolicCompiledInitial` definition | "compiler/symbolic.rs" | `src/simlin-engine/src/compiler/symbolic.rs:278-283` |
| `SymbolicCompiledModule` definition (`compiled_initials` field) | — | `src/simlin-engine/src/compiler/symbolic.rs:285-304` (field at `:290`) |
| init `SymbolicCompiledInitial` renumber loop | `db.rs:4583-4635` | `src/simlin-engine/src/db.rs:4739-4795` (comment `:4739`, loop `:4749-4795`) |
| per-ident keying (`ident: Ident::new(name)`) | `db.rs:4617-4619` | `src/simlin-engine/src/db.rs:4777-4778` |
| `resolve_module` call site | `db.rs:4719` | `src/simlin-engine/src/db.rs:4879` |
| `resolve_module` definition | — | `src/simlin-engine/src/compiler/symbolic.rs:1086-1139` (initials at `:1090-1103`) |
| flows `concatenate_fragments` (contrast) | `db.rs:4556`/`4567` | `src/simlin-engine/src/db.rs:4716` (`concatenate_fragments` def `symbolic.rs:1218`) |
| flow-fragment collection loop (Task 6 dt injection) | `db.rs:4450-4486` | `src/simlin-engine/src/db.rs:4625-4633` |
| init-fragment collection loop (Task 6 init injection) | — | `src/simlin-engine/src/db.rs:4615-4623` |
| `CompiledInitial` definition | — | `src/simlin-engine/src/bytecode.rs:3071-3084` |
| production init runner `eval_initials` | — | `src/simlin-engine/src/vm.rs:1219-1243` |
| `CompiledModuleInitials` construction | — | `src/simlin-engine/src/vm.rs:548-562` (initials `:558`) |
| `compile_phase` closure (Task 5 reuse) | `db.rs:3433-3520` | `src/simlin-engine/src/db.rs:3593-3680` |

---

## The four questions, answered with evidence

### 1. Is `compiled_initials` consumed strictly per-variable-ident downstream of `resolve_module`?

**No. It is consumed positionally (in vector order); the `ident` is
diagnostic-only metadata and is never used to route a write to a slot.**

Evidence chain, from `resolve_module` outward:

- `resolve_module` (`symbolic.rs:1090-1103`) maps `sym.compiled_initials` 1:1
  to `CompiledInitial`s. For each it calls `resolve_bytecode` then
  `extract_assign_curr_offsets(&bytecode)` (`symbolic.rs:1142-1155`) — the
  offsets are **re-derived from the bytecode's `AssignCurr`/`AssignConstCurr`/
  `BinOpAssignCurr` operands**, not from the ident. The `ident` is copied
  through verbatim (`sci.ident.clone()`, `:1098`) and never inspected.
- `CompiledInitial` (`bytecode.rs:3071-3084`) documents `ident` and `offsets`
  as "Used for diagnostics … and tests"; **both fields carry
  `#[allow(dead_code)]`** (`:3077`, `:3081`). Only `bytecode` is live.
- `CompiledModuleInitials` is built by a plain `m.compiled_initials.clone()`
  (`vm.rs:558`) — **order preserved exactly** from the `db.rs` `Vec`.
- The production init runner `eval_initials` (`vm.rs:1219-1243`) does:
  `for compiled_initial in module_initials.initials.iter() { eval_bytecode(.., &compiled_initial.bytecode, StepPart::Initials, ..) }`.
  It runs each entry's bytecode **in vector order** and reads neither `.ident`
  nor `.offsets`.
- Slot identity is purely the `AssignCurr` operand: the VM does
  `curr[module_off + off] = stack.pop()` (`vm.rs:1474`; `AssignConstCurr`
  `:1483`; `BinOpAssignCurr` `:1489`). The variable→slot mapping is the
  `VariableLayout`/`offsets` map, entirely independent of how
  `compiled_initials` is partitioned into entries.
- The only other readers of `.ident`/`.offsets` are all `#[cfg(test)]`:
  `debug_print_bytecode` (`vm.rs:2712-2720`) and the offset/ident assertions
  at `vm.rs:3363-3457`. No production path keys off them.
- Constant-override resolution does **not** use the ident either: `set_value`
  (`vm.rs:1040-1061`) resolves the variable through `self.offsets`
  (ident→slot), then `apply_override`→`constant_info[&off]` (slot→
  `BytecodeLocation`). `BytecodeLocation::Initial { initial_index }` is built
  at `vm.rs:418-434` by scanning each initial's bytecode `AssignConstCurr`
  ops and matching **absolute offsets**, then indexed positionally
  (`initials_module.initials[*initial_index]`, `vm.rs:950`/`990`). Still
  offset/position based, never ident based.

**Therefore a single `SymbolicCompiledInitial` carrying a synthetic ident can
write any number of members' init slots**, provided its bytecode contains the
corresponding `AssignCurr` writes. This is not even novel: an arrayed variable
*already* does it — `arr[Dim]` (3 elements) compiles to **one**
`CompiledInitial { ident: "arr", … }` whose bytecode writes 3 slots
(`arr[a]`, `arr[b]`, `arr[c]`); the test at `vm.rs:3436-3457` finds that single
entry by ident and asserts `offsets.len() == 3`. A 1→N ident↔slot relationship
is already routine; a synthetic-ident combined fragment is the same shape with
N members instead of one variable's N elements.

### 2. Can the combined init fragment be emitted as one `SymbolicCompiledInitial` with a synthetic ident whose bytecode writes each member's `member_base + elem` init slot (same offsets, reordered), analogous to the flows combined fragment?

**Yes.** The init phase already produces the *same* `PerVarBytecodes` type as
flows. In `compile_var_fragment`, `initial_bytecodes`, `flow_bytecodes`, and
`stock_bytecodes` are each `compile_phase(&var_result.ast)` returning
`Option<PerVarBytecodes>` (`db.rs:3703-3737`). `compile_phase`
(`db.rs:3593-3680`) wraps the lowered `&[Expr]` into a minimal
`crate::compiler::Module` with `runlist_flows: exprs.to_vec()` ("we put
everything in flows", `:3641`) and emits one `PerVarBytecodes` ending in the
normal trailing `Ret`. **The init lowered `Vec<Expr>` is structurally
indistinguishable from the flows lowered `Vec<Expr>` for the purpose of the
Task 4 interleave + Task 5 recompile.** So the Task 4/5 pipeline produces one
combined-init `PerVarBytecodes` exactly as it does for dt.

`resolve_module` and init codegen do **not** assume a 1:1 ident↔init-slot
mapping (Question 1 evidence). The *only* `compiled_initials.len()` assertion
in the codebase is `symbolic.rs:1945-1948`, which checks that `resolve_module`
preserves the **count** of entries on the symbolic→resolved roundtrip (a
length parity + pairwise compare). It maps 1:1 over whatever vector it is
given and makes no cardinality claim about idents vs slots; a combined
fragment is just a shorter vector and roundtrips identically.

Each member's `AssignCurr` keeps its original absolute slot operand
(`member_base + elem`); only the *ordering* of the writes changes. Variable
layout (`compute_layout`) and the results offset map are untouched, so
per-variable result series stay individually addressable (AC2.3).

### 3. If representable: the exact mechanism

**It is representable. Mechanism (the init half of Task 6):**

The init renumber loop is `db.rs:4749-4795`, fed by `initial_frags`
(`Vec<(String, &PerVarBytecodes)>`) collected at `db.rs:4615-4623`. The
mechanism mirrors the dt path but terminates at the per-fragment renumber loop
instead of `concatenate_fragments`:

1. **Build the combined init `PerVarBytecodes` (Tasks 4 + 5) for each
   `ResolvedScc` whose `phase == SccPhase::Initial`.** Source each member's
   *production-init* lowered `Vec<Expr>` via
   `var_phase_lowered_exprs_prod(.., SccPhase::Initial)` (Phase 1 Task 3
   accessor; selects `per_phase_lowered.initial`). Task 4 splits each member's
   init `Vec<Expr>` into per-element slices keyed by the `AssignCurr` offset
   (`member_base_M + e`) and concatenates the slices in
   `ResolvedScc.element_order`. Task 5 recompiles that combined `Vec<Expr>`
   through the extracted `compile_phase` into one `PerVarBytecodes` ending in
   a single `Ret`. **No structural difference from the dt combined fragment** —
   the init `Vec<Expr>` is the same shape (a flat per-element sequence of
   `[pre-exprs…, AssignCurr(member_base+elem, rhs)]`).

2. **Skip the SCC members in the init-fragment collection loop and inject the
   combined fragment at the first member's slot.** The loop to modify is
   `db.rs:4615-4623`:

   ```rust
   for var_name in &dep_graph.runlist_initials {
       if let Some(result) = all_fragments.get(var_name)
           && let Some(ref bc) = result.fragment.initial_bytecodes
       {
           initial_frags.push((var_name.clone(), bc));
       } else if !is_module_input(var_name) {
           missing_vars.push(var_name.clone());
       }
   }
   ```

   For each init-phase `ResolvedScc`, when iterating `runlist_initials`:
   - skip every SCC member's per-ident `initial_frags.push((member, bc))`;
   - at the position of the **first** SCC member encountered (in
     `runlist_initials` order — same anchor rule as the dt path's "first SCC
     member encountered"), push **one** entry
     `initial_frags.push((synthetic_ident, &combined_init_bc))` instead.

   The combined `PerVarBytecodes` must be **owned** in a local that outlives
   the `db.rs:4749-4795` renumber loop (the loop borrows `&PerVarBytecodes`).
   Allocate it alongside the dt combined fragment's owner (Task 6 already
   needs such a local for the flows combined `PerVarBytecodes`); both can live
   to the end of `assemble_module`.

3. **The existing renumber loop (`db.rs:4749-4795`) then processes the combined
   entry unchanged.** It maps `renumber_opcode` over the entry's code with the
   running `init_*_off` resource bases and pushes
   `SymbolicCompiledInitial { ident: Ident::new(synthetic_ident), bytecode: … }`
   (`db.rs:4777-4783`). Critically, `renumber_opcode` (`symbolic.rs:1327-1380`)
   renumbers **only resource IDs** (literals, GFs, modules, views, temps,
   dim-lists) — it **never touches `AssignCurr` slot operands**. The
   per-fragment `init_*_off` accumulation (`db.rs:4784-4794`) exists solely to
   keep each renumbered fragment's resource namespaces non-colliding; it is
   orthogonal to slot offsets. So the combined entry's `member_base + elem`
   writes survive verbatim into the renumbered bytecode, and the running
   resource bases stay correct because the combined fragment's resource
   side-channels (`graphical_functions`, `module_decls`, `static_views`,
   `temp_sizes`, `dim_lists`) are self-consistent (Task 5's obligation, the
   same one the dt combined fragment must satisfy for `concatenate_fragments`
   / `ContextResourceCounts::from_fragments`).

4. **`resolve_module` and the VM consume it with zero changes.** Per Question 1:
   `resolve_module` re-derives offsets from the bytecode and copies the ident
   through; `eval_initials` runs each entry's bytecode in vector order. The
   combined entry sits at the first-member slot, so its writes execute at the
   point in init order where the first member would have — and because the
   SCC is element-acyclic by Phase 1's verdict, the interleaved
   `element_order` satisfies every cross-member element dependency within that
   single bytecode.

**Synthetic ident:** use a reserved, collision-free name analogous to the
existing synthetic-node convention (LTM uses the `$⁚…` U+205A prefix; aggs use
`$⁚ltm⁚agg⁚{n}`). Recommended: `$⁚scc⁚init⁚{n}` where `{n}` is the
`ResolvedScc`'s deterministic index (Phase 1's sorted Tarjan order ⇒ stable).
Constraints the ident must satisfy:
   - It must **not** collide with any real variable name in `offsets`/the
     layout (a `$⁚`-prefixed name cannot — those characters are not produced
     by canonicalization of user identifiers, same guarantee LTM relies on).
   - It does **not** need a layout slot of its own: the combined bytecode
     writes the *members'* absolute slots; the synthetic ident is never looked
     up in `offsets` for the init phase (init has no per-ident slot lookup —
     Question 1). It is purely the diagnostic label on the `CompiledInitial`.
   - Determinism: the ident is a pure function of the SCC index, and
     `element_order` is already byte-stable (Phase 1 sorted tie-break), so the
     combined fragment and its `SymbolicCompiledInitial` are byte-stable
     (AC2.3 init analogue).

**This is the chosen mechanism. Task 6's init half implements exactly steps
1–3 above; step 4 requires no code.**

### 4. If NOT cleanly representable: the alternative

Not needed — the primary mechanism in §3 is clean. Recorded for completeness
because the task asked it be investigated and because it is a valid,
**equivalent fallback** if §3 ever hits an unforeseen obstacle:

**Per-member ordered `SymbolicCompiledInitial`s.** Init slots are **written,
not accumulated** — `AssignCurr`/`AssignConstCurr`/`BinOpAssignCurr` all do
`curr[…] = …` (`vm.rs:1474`/`1483`/`1489`), never `+=`. And `eval_initials`
runs entries strictly in `compiled_initials` vector order (`vm.rs:1230`). So
an alternative is: keep one `SymbolicCompiledInitial` **per member** (each with
its real ident and its own correct per-member bytecode), but **emit them into
`initial_frags` in an order consistent with `element_order`** so that, across
the sequentially-executed entries, every cross-member element read sees an
already-written slot.

This fallback is **strictly weaker** than §3 and should not be preferred:
within a single member's `SymbolicCompiledInitial` the element writes are still
in that member's *declared* element order (one contiguous `AssignCurr` block
per variable — the very limitation Phase 2 exists to remove). It therefore
only resolves SCCs whose required interleaving happens to be expressible as a
*whole-member* topological order (no genuine per-element interleaving *within*
the cycle). It would handle `interleaved.mdl`-style cases only if the element
dependencies do not force splitting a member's block. **It does not subsume
§3** and must not be silently substituted. It is documented solely so a future
implementor knows the write-not-accumulate property holds (verified) and a
degraded path exists in principle.

---

## Consequence for C-LEARN

C-LEARN reports **both** a dt cycle and an init cycle. With the §3 mechanism,
the init-phase combined fragment is representable, so Phase 6 (which depends on
Phase 2) is **not** blocked on init. The loud-safe init fallback (keep
`CircularDependency` for init-phase SCCs) is **not** taken and is **not** an
autonomous path in Task 6. AC2.4 (a fixture where a stock breaks the dt chain
but not the init chain) is achievable because both dt and init recurrence SCCs
resolve through the same combined-fragment pipeline, differing only in (a)
which lowered exprs are sourced (`SccPhase::Initial` vs `Dt`) and (b) the
injection site (the `db.rs:4749-4795` renumber loop via `initial_frags` vs
`concatenate_fragments` via `flow_frags`).

**No `track-issue` filing is warranted: the hard gate passed.**

---

## Task 6 implementor checklist (init half)

1. After computing `dep_graph.resolved_sccs`, for each `ResolvedScc` with
   `phase == SccPhase::Initial`, build the combined init `PerVarBytecodes`
   via Tasks 4 (`var_phase_lowered_exprs_prod(.., SccPhase::Initial)` →
   interleaved `Vec<Expr>`) + 5 (recompile via the extracted `compile_phase`).
   Own each in a local that lives to the end of `assemble_module`.
2. In the `runlist_initials` collection loop (`db.rs:4615-4623`): for members
   of an init `ResolvedScc`, skip the per-ident `initial_frags.push`; at the
   first member encountered (runlist order), push
   `(synthetic_ident /* "$⁚scc⁚init⁚{n}" */, &combined_init_bc)` once.
3. Do **not** modify the renumber loop (`db.rs:4749-4795`), `resolve_module`
   (`symbolic.rs:1086`), `compute_layout`, or `eval_initials` — they are
   already agnostic to the entry partitioning (verified above).
4. Mirror the dt half exactly; the only deltas are the expr source phase and
   the injection site (`initial_frags` renumber path vs `flow_frags`
   `concatenate_fragments` path).
5. AC2.3 init analogue: assert the assembled module's results offset map for
   each init SCC member is identical to the non-SCC layout, and that the
   combined `SymbolicCompiledInitial` is byte-stable across two fresh-DB
   compiles (Task 10).
