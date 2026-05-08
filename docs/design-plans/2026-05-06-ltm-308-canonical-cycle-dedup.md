# LTM canonical-edge-sequence cycle dedup

Date: 2026-05-06
Branch: `ltm/308-canonical-cycle-dedup`
Issue: #308 (epic: #488)

## Summary

LTM currently deduplicates feedback loops by **sorted node-set**, collapsing
distinct directed cycles like `A -> B -> C -> A` and `A -> C -> B -> A` into a
single `Loop`. This contradicts the elementary-circuit identity used in the
LTM literature: cycle identity is determined by the directed edge sequence,
not just the set of participating variables. Two cycles with the same node
set can have different polarity products and different scores.

This change replaces the sorted-node-set dedup key with a **canonical
edge-sequence rotation**: rotate each circuit so it starts at the
lex-smallest start position, then compare the ordered node sequence (which
on an elementary cycle determines the edge sequence).

## Identity rule

A circuit `[v0, v1, ..., v_{n-1}]` represents the directed elementary cycle
`v0 -> v1 -> ... -> v_{n-1} -> v0`. Two circuits represent the same cycle
iff one is a cyclic rotation of the other.

The **canonical rotation** is defined by:

1. Score each rotation by the pair `(node_index, next_node_index)` of its
   starting position, using lex order over the entire rotation's node-pair
   sequence as a tiebreaker.
2. The rotation whose starting `(start_node, second_node)` pair is smallest,
   with subsequent pairs as tiebreakers, is canonical.

Concretely: pick the index `i` in `[0, n)` such that the rotation
`[circuit[i], circuit[i+1], ..., circuit[i-1]]` has the lex-smallest
sequence of `(node_index, next_node_index)` pairs. Comparison proceeds
element-wise; the first differing pair decides.

In the elementary-cycle setting all nodes within a circuit are distinct
(Johnson's algorithm and the discovery DFS both guarantee this via blocked /
visiting sets), so the lex-smallest **node** alone uniquely identifies the
starting position. The `(node, next)` tiebreaker matters only for theoretical
robustness: if two equally-small starting nodes ever appeared (e.g., in a
non-elementary closed walk), the second-position node would break the tie
naturally because directed neighbors of a single node along the cycle are
themselves distinct.

### Worked example: arms-race 3-party

Three stocks `A`, `B`, `C` with cross-target equations
(`a_target = B + 0.9*C`, `b_target = A + 1.1*C`, `c_target = 1.1*A + 0.9*B`)
generate a multidigraph SCC with both directions of three-way feedback:

- Cycle `R1`: `A -> b_target -> b_changing -> B -> a_target -> a_changing -> A`
  (via stocks `A -> B -> A`-loop, but with the three stocks chained:
  `A_arms -> b_target -> b_changing -> B_arms -> c_target -> c_changing -> C_arms -> a_target -> a_changing -> A_arms`).
- Cycle `R2`: same node set, opposite traversal:
  `A_arms -> c_target -> c_changing -> C_arms -> b_target -> b_changing -> B_arms -> a_target -> a_changing -> A_arms`.

Both rotate to start at `A_arms` (lex-smallest). Their canonical sequences
differ at the second position: `R1` continues to `b_target`, `R2` continues
to `c_target`. Distinct canonical forms, two distinct loops.

### Polarity composition example

Two cycles can share a node set but compose different polarities even when
every individual link's polarity is the same -- because the **link
identities** themselves differ. Consider a 3-cycle `A,B,C` where:

- Forward direction: `A -> B` is positive, `B -> C` is negative, `C -> A` is
  positive. Negative-link count = 1 (odd) -> Balancing.
- Reverse direction: `A -> C` is negative, `C -> B` is positive, `B -> A` is
  positive. Negative-link count = 1 (odd) -> Balancing.

The polarities happen to align here, but in general they need not. For
example, change `B -> C` to positive and `C -> B` to negative (asymmetric
polarities): forward becomes Reinforcing (0 negatives), reverse becomes
Balancing (1 negative). Currently the buggy dedup keeps only one of these
two and silently drops the other's distinct polarity composition.

## Application points

Three call sites currently dedup by sorted node-set; all three must switch
to canonical edge sequence:

1. **`IndexedGraph::johnson_circuit`** in `src/simlin-engine/src/ltm.rs`.
   Inline dedup inside the per-SCC enumerator. Currently keys a 64-bit
   rapidhash fingerprint over the **sorted** index path; will switch to
   the **canonical-rotated** index path.
   Key type stays `Vec<u32>` (numeric, indexed by `IndexedGraph`).

2. **`SearchGraph::add_loop_if_unique`** in
   `src/simlin-engine/src/ltm_finding.rs`. Discovery-mode DFS dedup; keys
   the sorted `Vec<String>` of node names. Will switch to a canonical
   rotation of `Vec<String>` over the directed sequence.
   Key type stays `Vec<String>` (Ident-like, since `SearchGraph` is
   String-keyed).

3. **`discover_loops_with_graph`** in `src/simlin-engine/src/ltm_finding.rs`.
   Cross-timestep path dedup; same fix as (2), keyed on `Vec<String>`.

The numeric vs string distinction is intentional: `IndexedGraph` operates
on dense integer node IDs (allocated by `IndexedGraph::from_edges`), while
discovery operates on already-allocated `Ident<Canonical>` values. Lifting
the helper to a generic over `Ord + Hash + Clone` keeps both paths sharing
one implementation.

### Helper signature

```rust
// In src/simlin-engine/src/ltm.rs, available to ltm_finding.rs.
//
// Returns a Vec representing the lex-smallest cyclic rotation of `circuit`
// based on the (node, next_node) pair-sequence ordering described above.
// `circuit` is treated as a closed cycle: position i edges to position
// (i + 1) % len. Empty input returns empty.
pub(crate) fn canonical_rotation<T: Ord + Clone>(circuit: &[T]) -> Vec<T>;
```

`canonical_rotation` is the only new public-ish helper. The two String-keyed
sites build their key by mapping the rotated slice to whatever string
representation matches their existing dedup-set type.

## Stability

Canonical rotation is **deterministic from the input**: given a fixed circuit
`[v0, ..., v_{n-1}]`, the canonical rotation is uniquely defined. So:

- Loop IDs (`r1`, `b2`, ..., assigned by `assign_loop_ids` after sorting by
  variable-name key) remain stable across runs on identical input. The
  sorting key in `assign_loop_ids` is the underlying variable set, not
  the circuit shape, so adding a previously-collapsed second cycle to the
  output simply inserts a new ID without renaming the existing one.
- Salsa cache keys (`LoopCircuitsResult` returned from
  `db_analysis::model_loop_circuits`) remain stable: the rapidhash
  fingerprint is computed over a deterministic byte sequence (canonical
  rotation followed by little-endian u32 encoding), and the seed
  (`CIRCUIT_HASH_SEED`) is unchanged. Rotation order is fully determined
  by node-index ordering, which itself is determined by lex sort of node
  names in `IndexedGraph::from_edges`.
- Test models that previously had `N` loops will now have `N + k` loops,
  where `k` is the number of distinct directed cycles whose node set was
  shared with another. New loops get new IDs assigned by
  `assign_loop_ids`'s deterministic counter; existing loops keep theirs.

## Fixture impact

Only models with multidigraph SCCs (multiple distinct directed cycles
sharing a node set) are affected. Empirically this is rare in real SD
models -- the node-set dedup was harmless on every fixture except those
explicitly designed to exercise it.

| Fixture | Location | Before | After | Notes |
|---|---|---|---|---|
| `discovery_arms_race_3party` | `tests/simulate_ltm.rs:385` | 7 loops | 8 loops | 3 self-balancing, 3 pairwise reinforcing, 2 three-way (forward + reverse) |
| `johnson_complete_k3_all_directed_cycles` | `src/ltm.rs:5371` | 4 circuits | 5 circuits | K3: 3 two-cycles + 2 three-cycles (forward + reverse) |
| `johnson_multidigraph_dedups_to_single_node_set` | `src/ltm.rs:5416` | 1 three-cycle | 2 three-cycles | The test's premise inverts; rename + rewrite |
| `johnson_budget_charges_duplicate_raw_cycles` | `src/ltm.rs:5444` | 11 circuits | 20 circuits | K4: C(4,2) + C(4,3)*2 + 6 = 6 + 8 + 6 = 20. Budget that fits 20 must succeed; budget of 19 must trip |

K4's four-cycle count: there are 3!/2 = 3 undirected Hamilton cycles on 4
nodes; each maps to 2 directed cycles (forward + reverse), so 6 directed
4-cycles. The K4 test's budget value also needs adjusting since the test
proves the budget bounds raw enumeration cost.

The `assert_johnson_matches_tiernan` test oracle uses
`canonicalize_circuits` to compare Johnson's vs Tiernan's outputs; this
helper currently sorts each circuit's nodes, so under the new identity
rule it would mask divergences. It must switch to canonical-rotation
comparison so equivalence still proves Johnson's = Tiernan's.

WRLD3's element-level enumeration count
(`wrld3_element_level_enumeration_is_uncapped`, currently `#[ignore]`d) was
1,863,803 under sorted-node-set dedup. Under canonical-edge-sequence dedup
the count may grow if WRLD3 contains symmetric directed cycles. Since this
test is `#[ignore]`d and only run on demand, we will adjust its expected
value when the change lands, with the new count documented in the test
comment for traceability.

The `loops_have_unique_node_sets` and `IndexedGraph::has_no_duplicate_node_sets`
debug-only invariant checkers must be updated to assert canonical
edge-sequence uniqueness instead of node-set uniqueness, otherwise they
fail loudly on the first multidigraph input.

## Out of scope

- The known flow-to-stock time-alignment decision (separate concern).
- The `MAX_LTM_SCC_NODES` gate stays at 50; this change does not relax it.
  If the new dedup substantially increases per-SCC circuit counts, the
  gate continues to short-circuit before enumeration runs.
- The discovery heuristic's prune (`best_score`) is unchanged. The new
  identity rule only changes which discovered paths survive dedup.
