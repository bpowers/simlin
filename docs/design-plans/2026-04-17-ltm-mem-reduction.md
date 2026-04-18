# LTM Circuit-Enumeration Memory Reduction

Status: in progress on branch `reduce-ltm-mem`.
Baseline captured 2026-04-17.

## Problem

`CausalGraph::find_circuit_node_lists` enumerates elementary circuits via
lexicographic-small-start DFS over the causal graph. On dense SD models this
blows up in both time and memory:

* test/metasd/WRLD3-03/wrld3-03.mdl has **1,863,803** deduplicated elementary
  circuits (mean length 47 nodes, max 80).
* Full enumeration consumes **8.45 GiB peak RSS** and **27 s wall-clock** on
  reduce-ltm-mem@d1870d2e (baseline commit for this work). WASM's 4 GiB linear
  memory limit forces the existing `MAX_LTM_CIRCUITS = 100_000` budget, which
  itself already consumes ~322 MiB.
* Per-circuit cost: ~4.5 KiB (Vec<Ident<Canonical>> with ~47 heap-allocated
  String clones per circuit, plus the dedup HashSet<String> key which is
  string-joined node names).

## Graph Characterization (wrld3-03)

* 302 nodes with outgoing edges, 313 nodes overall
* 483 total out-edges
* 1 weakly-connected component (size 313)
* **148 strongly-connected components. Largest = 166 nodes.**
* **Exactly 1 non-trivial SCC (size > 1).** 147 trivial SCCs carry zero cycles.
* 15 stocks, all in the single non-trivial SCC.

The `166`-node SCC is where every loop lives. Enumeration across all 302
start-nodes spends the majority of its work inside this SCC; the 147 nodes
outside it never close a cycle but are still explored when their successor
edges lead into the SCC.

## System-Dynamics Domain Notes

* Directed edges: flow→stock (structural), stock→flow / aux→aux / stock→aux
  / aux→flow (from equation dependencies). Stock init-value dependencies are
  intentionally excluded from the cycle graph.
* A well-formed SD cycle contains at least one stock: stocks provide the
  DT-delay that makes the feedback solvable. A pure auxiliary cycle would
  be an unsolvable algebraic loop and is rejected elsewhere.
* The DT delay is a semantic property of how the simulator integrates the
  stock, not a structural property of the graph. For the purposes of cycle
  enumeration the directed graph is the complete ground truth; Johnson's
  algorithm on it is both necessary and sufficient.
* SD models tend to have long cycles (many auxes between stocks). wrld3's
  mean cycle length is 47 nodes -- the per-circuit memory cost is dominated
  by path length.

## Hypotheses (in priority order)

### H1 -- SCC-restricted enumeration

Compute strongly-connected components of the full graph once. For each
non-trivial SCC, enumerate circuits using only intra-SCC edges, starting
only from nodes in that SCC. Cross-SCC edges are never traversed (they
cannot close any cycle). Trivial SCCs are skipped outright.

Expected savings: modest on wrld3 (most of the work already happens inside
the big SCC because of the small-start invariant) but significant for time
on graphs with many trivial feeders. Mainly a correctness-preserving
structural improvement that composes well with H2/H3.

### H3 -- integer node IDs throughout the DFS

Replace `Vec<Ident<Canonical>>` paths and visited sets with `Vec<NodeId>`
(NodeId = u32) plus a bitset. Each Ident clone is a heap String clone; each
NodeId is 4 bytes no-alloc.

Expected savings:
* Per-circuit memory: ~14x reduction (~120 B for a 30-node circuit vs. ~1660 B)
* Dedup key: ~8x reduction (sorted Vec<u32> vs. joined string)
* Projected wrld3 peak: 8.45 GiB -> ~500 MiB.

### H5 -- indexed dedup key

Follows directly from H3. Dedup via `HashSet<SmallVec<[u32; 48]>>` of sorted
node indices rather than `HashSet<String>` of joined names.

### H2 -- true Johnson's with blocking (deferred)

If time at `usize::MAX` budget remains unacceptable after H1+H3+H5, adopt
Johnson's full algorithm using block-map unblocking. Johnson's is O((V+E)(C+1))
worst case; the current small-start DFS can degrade badly on dense SCCs.

### H11 -- eliminate defensive dedup (deferred / investigative)

The small-start invariant already guarantees each circuit is found exactly
once (from its lex-smallest node). The current `HashSet` dedup is defensive.
Confirm empirically on a few models that the dedup is a no-op, then either
remove it or keep a cheap indexed version from H5.

## Success Criteria

* Full wrld3 enumeration (budget = usize::MAX) fits in **<= 500 MiB** peak RSS.
* Stretch: **<= 200 MiB** peak RSS.
* No regression on simulate_ltm integration tests.
* New unit tests cover SCC decomposition and indexed-graph invariants.
* `wrld3_ltm_compilation_finishes_in_time` continues to pass.

## Measurement Harness

`src/simlin-engine/examples/ltm_mem_bench.rs` reports, given an `.mdl` path
and a circuit budget:

* graph shape (nodes / edges / stocks / SCC sizes)
* elapsed wall-clock for `find_circuit_node_lists_with_limit`
* VmPeak / VmHWM / VmRSS from /proc/self/status at each phase

Invocation:
```
cargo run --release --example ltm_mem_bench -- test/metasd/WRLD3-03/wrld3-03.mdl <budget>
```
