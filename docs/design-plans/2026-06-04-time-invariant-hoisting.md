# Time-invariant variable hoisting (GH #712)

Status: Stage B1 (classification + runlist partition + split metadata) — this
document. Stage B2 (VM execution: run the invariant phase once per `run_to`) is a
separate change; sketched at the end.

## Motivation

On constant-heavy models a large fraction of the per-step flow program recomputes
values that are identical at every timestep. Measured (prototype classifier):

- C-LEARN: 45.4% of the 31,797 root flow opcodes target dynamically-constant
  variables; 678 / 1368 root flow vars are provably run-invariant.
- WORLD3-03: 26.9% of root flow opcodes; 71 / 242 vars.

A variable whose dt-phase lowered expression transitively depends only on
constants — with no dependency on `TIME`, stocks, `PREVIOUS`, time-dependent
builtins (`PULSE`/`RAMP`/`STEP`), or module evaluations — produces the same value
every step. Such variables can be evaluated **once** per `run_to` rather than per
step (B2). B1 is the behavior-neutral prerequisite: classify them, reorder the
flow runlist so they form a contiguous prefix, and record the prefix boundary.

## Definition (run-invariant, root-model flow phase)

A root-model flow-phase variable is *run-invariant* iff its dt-phase lowered
expressions (`compiler::Var.ast`, the `Vec<Expr>` produced by `Var::new`)
transitively reference only:

- literals (`Expr::Const`), `Expr::Dt`, and the builtins
  `TimeStep`/`StartTime`/`FinalTime`/`Pi`/`Inf`/`IsModuleInput`;
- `BuiltinFn::Init(x)` of ANY variable (the `initial_values` buffer is frozen
  after the initials phase, so an INIT read is constant across the run);
- graphical-function lookups (`Lookup`/`LookupForward`/`LookupBackward`) whose
  index/element-offset subexpressions are invariant (the tables themselves are
  static; the table argument is a `Var(off)` reference to a static lookup-only
  holder, which is treated as an always-invariant source);
- pure scalar builtins (`Abs`/`Arccos`/`Arcsin`/`Arctan`/`Cos`/`Exp`/`Int`/`Ln`/
  `Log10`/`Sign`/`Sin`/`Sqrt`/`Tan`/`Max`/`Min`/`Mean`/`SafeDiv`/`Quantum`/
  `Sshape`) and array reducers (`Sum`/`Size`/`Stddev`) of invariant arguments;
  the array-producing builtins (`Rank`/`VectorSelect`/`VectorElmMap`/
  `VectorSortOrder`/`AllocateAvailable`/`AllocateByPriority`) of invariant
  arguments are pure too and are included via an exhaustive argument walk;
- other run-invariant variables, reached by offset:
  `Expr::Var(off)`, `Expr::Subscript(off, idxs, ..)` with invariant index
  exprs, `Expr::StaticSubscript(off, view, ..)`; plus `TempArray`/
  `TempArrayElement`/`AssignTemp` self-references within the variable's own
  statement list (temps are intra-statement scratch, classified by the
  expression assigned to them).

**Variant (hard exclusions, default-variant):** `BuiltinFn::Time`,
`Pulse`/`Ramp`/`Step` (time-dependent even with constant args), `Previous`
(reads `prev_values`), `Expr::ModuleInput`, `Expr::EvalModule` (module instances
are conservatively variant), and a reference to any offset that resolves to a
stock, a module-instance slot range, or a time-global other than DT/INITIAL/
FINAL. Anything not explicitly recognized is variant; the walker is exhaustive
over every `Expr` and `BuiltinFn` variant with explicit arms, so a future new
variant is a compile error rather than a silent misclassification.

### The two prototype false-positive classes (both handled)

1. **Module-OUTPUT reads via plain LoadVar offsets.** A parent variable that
   reads `submodule·output` reads a slot inside the module instance's slot
   range; that slot changes every step. This is prevented by variant-by-omission
   plus the bare module-instance kind check: in the salsa per-fragment path, any
   dependency whose `VarInfo.is_module` flag is set is classified variant; the
   pre-computed `dep_names` set fed to the topological check does not include
   module-kind deps in the invariant set, so a variable reading a module output
   is automatically variant.

2. **Arrayed whole-array reads via views of a variant array.** A
   `StaticSubscript`/view read whose base offset belongs to a variant array is
   variant. Handled the same way: the base offset (or dependency name) is
   resolved to its owner and classified; an invariant verdict requires the owner
   to be invariant.

The soundness oracle test (below) would catch either regression: it asserts
every classified-invariant variable has a bit-constant series across all saved
steps on real models.

## Time-globals and offsets

DT/TIME/INITIAL_TIME/FINAL_TIME are 0-arity builtins reified at the `Expr0`
level (`reify_0_arity_builtins`), so a bare `time`/`dt`/`initial_time`/
`final_time` reference lowers to `Expr::App(BuiltinFn::*)` / `Expr::Dt`, NOT to
`Expr::Var(0..3)`. Therefore an `Expr::Var(off)` at the root never aliases the
implicit globals in practice. The offset classifier still rejects offsets
0..`IMPLICIT_VAR_COUNT` defensively (TIME at slot 0 is the only one of the four
that would be variant, but a `Var` reference to any of them is unexpected and is
treated conservatively).

In the salsa per-variable fragment path the lowered `Expr` offsets are NOT
model-global — they are *mini-layout* offsets local to the fragment (the
variable itself at 0, its dependencies at sequential slots), invertible to a
`SymVarRef { name, element_offset }` via the fragment's `ReverseOffsetMap`. So
the offset-classification callback in that path resolves an offset to a
**dependency name** and looks that name up in a per-model verdict map. In the
monolithic (test-only) path the offsets ARE model-global and resolve via the
full metadata/offsets map. The classifier itself is identical; only the callback
differs.

## Fixpoint

Per-variable "locally invariant given the verdicts of its dependencies" is
computed by the shared classifier. Variance then propagates along the dependency
graph. The flow runlist is already a topological order (`dep_graph
.runlist_flows`): every dt-phase dependency precedes its reader (stocks break the
dt chain and are sinks, so they never appear as an un-ordered forward dep). A
single ordered pass therefore reaches a fixpoint — when we classify variable `v`,
every non-stock/non-module dependency of `v` has already been classified, so its
verdict is final. We record the verdict in a `BTreeSet<String>`
(`flows_invariant`) of canonical names. No iteration to convergence is needed for
the flow phase; this is justified by the topological-order property of the
runlist.

## Partition

The ROOT module's flow runlist is reordered to `[invariant vars (original
relative order preserved), then dynamic vars (original relative order
preserved)]`. This is a valid topological order: the invariant subgraph is closed
under its own dependencies (an invariant var cannot depend on a dynamic var, by
construction — if it did, it would be variant), so moving all invariant vars
ahead of all dynamic vars never places a reader before a dependency. Relative
order within each group is preserved, so the partition is a stable rearrangement
of the existing valid order.

The split is recorded as `CompiledModule.flows_invariant_opcode_len: usize` — the
opcode length of the invariant prefix of `compiled_flows.code` (0 when no var is
invariant, e.g. every submodule and any model with no run-invariant flow var).

### Why a prefix opcode length is a sound representation (both paths)

**Salsa path (production), `db/assemble.rs::assemble_module`.** The flow runlist
is assembled by concatenating per-variable symbolic `PerVarBytecodes` fragments
in runlist order (`concatenate_fragments_with_gf`). The concatenation is
**opcode-count-preserving**: `renumber_fragment_code` strips each fragment's
trailing `Ret` and copies every remaining opcode 1:1 (only resource IDs are
renumbered), and the merger appends exactly one terminal `Ret`. So the merged
symbolic flow code length is `sum_f (f.symbolic.code.len() - 1) + 1`. The
symbolic→concrete `resolve_module` is also 1:1 (`resolve_bytecode` maps each
`SymbolicOpcode` to one `Opcode`; `resolve_module` does NOT fuse). Therefore the
invariant-prefix opcode length in the final `compiled_flows.code` equals
`sum over the invariant prefix fragments of (f.symbolic.code.len() - 1)`. We
compute it by reordering `flow_frags` (invariant first), then summing the
Ret-stripped lengths of the invariant prefix. This is computed at the same place
the concat runs, so both the prefix and the concatenation see the identical
fragment order.

**Monolithic path (test-only), `compiler::Module`.** `Module::new` is
`#[cfg(test)]`; the production whole-model `Module` is never built. `Module::new`
flattens `flow_vars: Vec<Var>` into `runlist_flows: Vec<Expr>` before
`Compiler::compile` walks it. We reorder `flow_vars` (invariant groups first),
record the invariant var-name set on `Module`, and have `Compiler::compile`
record the opcode count after walking the invariant prefix. Because both paths
funnel every variable through the same `Var::new` lowering and the same classifier
over the resulting `Vec<Expr>`, the two paths classify identically; an
equivalence test asserts the split index agrees.

### Why the prefix boundary is fusion-proof

Two fusion passes touch flow bytecode:

- `peephole_optimize` runs **per fragment** inside `ByteCodeBuilder::finish()`
  (each per-var fragment is compiled and finished separately in the salsa path),
  so it never spans a fragment boundary at all.
- `fuse_three_address` runs **whole-program** at `Vm::new` on the execution copy
  of `compiled_flows`. Its fusion windows always START with a `Load*` opcode
  (`LoadVar`/`LoadConstant`/`LoadGlobalVar`) and end with an `Op2`/`BinOpAssign`
  combiner; no window starts with, or uses as a combiner, an `Assign*`-family
  opcode. The last invariant-prefix fragment ends in an `Assign*` write (the
  opcode immediately before the stripped `Ret`). Invariant vars are never module
  calls; module-call fragments end in `EvalModule`, but such vars are classified
  variant by the `is_module` kind check and never appear in the invariant
  prefix. The opcode at the prefix boundary `k` (the last opcode of the
  invariant prefix) is therefore an `Assign*`. A fusion window crossing the
  boundary would have to start at `code[k-1]` or `code[k]`: `code[k]` is
  `Assign*` (no window starts there), and a window starting at `code[k-1]`
  would need `code[k]` as its second-position `Load*` (3-window) or combiner
  (2-window) — but `code[k]` is `Assign*`, neither a `Load*` nor a valid
  combiner. So no fusion window crosses the boundary.

Consequence for B1: `flows_invariant_opcode_len` is recorded on the **pre-fusion**
resolved `compiled_flows` (the salsa artifact). B2 either makes
`fuse_three_address` boundary-aware (it already has the old→new PC remap to
translate the boundary index) or runs the two regions as separate programs.
Either way the boundary stays a clean opcode index because fusion never merges
across it.

### `collect_constant_info` is unaffected

`collect_constant_info` (vm.rs) scans the entire `compiled_flows.code` for
`AssignConstCurr`, order-independent. Reordering the flow runlist does not change
which offsets it reports, so `set_value`/`clear_values` constant-override
semantics are preserved.

### Scope

- **Submodules:** split is always 0 (the whole program is treated as dynamic).
  Hoisting only the root module keeps B2 simple (one invariant phase, one
  snapshot) and captures essentially all of the measured win (the measurements
  above are root-flow).
- **LTM synthetic variables:** classified like any other variable. Their
  `PREVIOUS` usage makes them variant naturally; no special-casing.

## B1 behavior-neutrality

B1 only reorders the root flow runlist and records a count. The reordered single
flow program computes the exact same values in a valid topological order, so:

- the VM (which in B1 still runs the whole flow program every step) produces
  byte-identical results — verified by `simulates_clearn` (VDF byte-exactness)
  and the corpus oracle;
- wasmgen consumes the same reordered single `compiled_flows` and is likewise
  unchanged (it ignores the split field in B1 and B2).

## B2 execution sketch (NOT in B1)

- At `Vm::new`, split the root `compiled_flows` at `flows_invariant_opcode_len`
  into an *invariant* program (prefix) and a *dynamic* program (suffix), each a
  standalone `ByteCode` ending in `Ret`. Fuse each independently (the boundary is
  fusion-proof, so this is equivalent to fusing the whole and slicing).
- `run_to(target)` runs the invariant program **once** at entry (writing the
  invariant slots into `curr`), then runs only the dynamic program per step.
- Per saved chunk, the invariant slots must still be present. Either (a)
  copy-forward the invariant slots from a snapshot buffer into each saved chunk,
  or (b) seed each fresh `curr` from the snapshot before the dynamic phase. The
  snapshot is taken right after the one-shot invariant run.
- `set_value` overrides are absorbed by **re-running the invariant phase** at the
  next `run_to` entry (an override of a constant, or of something feeding a
  constant-derived aux, propagates because the whole invariant program re-runs).
  This matches the existing "re-run the phase after `set_value`" requirement.
- wasmgen intentionally ignores the split in B2; it keeps running the single
  reordered program, so VM/wasm parity is preserved (the reordered program is
  identical for it).

Measure empirically before trusting the win: the perf doc records two negative
results where an opcode-count reduction did not translate to wall-clock because
the codegen of the giant inlined `eval_bytecode` was perturbed. B2 changes the
per-step memory-access order; an interleaved A/B on freshly built binaries is
mandatory.
