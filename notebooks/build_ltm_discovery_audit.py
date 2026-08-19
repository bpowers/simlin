#!/usr/bin/env python3
"""Build an LTM discovery audit notebook: engine output vs an independent
pure-Python enumeration of the same loop universe.

Constructs notebooks/ltm_discovery_audit_<model>.ipynb cell-by-cell via
nbformat, then executes it (populating outputs) with nbclient. The executed
.ipynb is a generated artifact (gitignored); this script is the source of
truth, so the audit can be regenerated against any engine build.

Every number in the notebook is COMPUTED by a cell -- nothing is pasted in --
so a regeneration against a newer engine either reproduces the findings or
visibly contradicts them. The audit's ground truth is a second implementation
of the same algorithm, written from the design
(docs/design-plans/2026-08-17-ltm-discovery-exact.md) rather than translated
from the Rust: the point is to disagree with the engine when the engine is
wrong, which a transliteration cannot do.

Before executing, this script builds and runs `examples/ltm_search_graph_dump`,
which emits the exact element-level edge set discovery consumes (from the
public `ltm_finding::link_score_offsets`), its per-step link-score series, the
engine's cycle partitions, and the engine's own reported loops with their score
series. The notebook re-derives everything downstream of that dump.

Run from the pysimlin venv, synced with the `notebooks` extra. Invoke the venv
interpreter DIRECTLY: `uv run` re-syncs the project as an editable install and
replaces any wheel you installed.

    (cd src/pysimlin && uv sync --extra dev --extra notebooks)
    src/pysimlin/.venv/bin/python notebooks/build_ltm_discovery_audit.py

Both paths are relative to the REPOSITORY ROOT; the subshell keeps the `cd`
from leaking into the second command, where it would resolve the interpreter
as `src/pysimlin/src/pysimlin/.venv/...` and fail before the generator starts.

`--model clearn`, `--model cross_agg` or `--model wrld3` builds one; the default builds all three.
Cargo's target directory is taken from `CARGO_TARGET_DIR` when set, so a
worktree can share a prebuilt target with the main checkout.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import nbformat as nbf

NOTEBOOKS_DIR = Path(__file__).resolve().parent
REPO = NOTEBOOKS_DIR.parent

# The two models that drive the discovery design: World3 is the dense-runtime
# stress case the exact enumeration exists for (a ~150k-cycle universe against
# a 200-loop report), C-LEARN the large-but-sparse case where the universe is
# smaller than the cap. Between them they cover both sides of every threshold
# in the pipeline.
MODELS: dict[str, dict[str, str]] = {
    "wrld3": {
        "rel_path": "test/metasd/WRLD3-03/wrld3-03.mdl",
        "title": "World3 (WRLD3-03)",
        "blurb": (
            "World3 is the model that calibrated the LTM mode gate: its 166-node "
            "variable-level SCC is why `MAX_LTM_SCC_NODES = 50` exists. Its "
            "*runtime* graph is the densest in the repo corpus, so it is where "
            "candidate generation either is exact or is a sample."
        ),
    },
    "cross_agg": {
        "rel_path": "test/cross_agg_ltm/cross_agg.stmx",
        "title": "cross-agg reducer fixture",
        "blurb": (
            "A three-element stock whose growth reads `SUM(pop[*])`: the smallest "
            "model on which the reducer machinery -- a synthetic aggregate node, "
            "trimmed reported identities, and cross-aggregate loops stitched from "
            "petals -- all fire. Its universe is seven loops (three petals, three "
            "pair-stitched, one triple), small enough to check by hand."
        ),
    },
    "clearn": {
        "rel_path": "test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
        "title": "C-LEARN v77",
        "blurb": (
            "C-LEARN v77 is a 911-variable arrayed climate-policy model, "
            "subscripted over three climate-sensitivity scenarios. Its static "
            "graph is far too large to enumerate at compile time, but its "
            "*runtime* graph -- the edges that ever carry a nonzero link score "
            "-- is small, which is the whole reason discovery is worth doing."
        ),
    },
}


def dump_path(target_dir: Path, key: str) -> Path:
    return target_dir / "ltm_audit" / f"{key}_search_graph.json"


def notebook_path(key: str) -> Path:
    return NOTEBOOKS_DIR / f"ltm_discovery_audit_{key}.ipynb"


def cargo_target_dir() -> Path:
    """Where cargo puts its artifacts, honoring an external `CARGO_TARGET_DIR`.

    A worktree normally has its own `target/`, but building the engine twice
    costs minutes; letting the caller point at a shared target is the
    difference between a regeneration being cheap and being avoided.
    """
    env = os.environ.get("CARGO_TARGET_DIR")
    return Path(env).resolve() if env else REPO / "target"


def regenerate_dump(key: str) -> Path:
    """Build and run `ltm_search_graph_dump` for one model.

    The dump is regenerated rather than reused, so a notebook can never be
    executed against a stale engine's edge set while claiming to audit the
    current one.
    """
    target = cargo_target_dir()
    out = dump_path(target, key)
    out.parent.mkdir(parents=True, exist_ok=True)
    model = REPO / MODELS[key]["rel_path"]
    if not model.exists():
        raise SystemExit(f"model not found: {model}")

    print(f"[{key}] cargo build --release --example ltm_search_graph_dump")
    subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--example",
            "ltm_search_graph_dump",
            "--manifest-path",
            str(REPO / "src" / "simlin-engine" / "Cargo.toml"),
        ],
        check=True,
        cwd=REPO,
    )

    exe = target / "release" / "examples" / "ltm_search_graph_dump"
    print(f"[{key}] {exe} -> {out}")
    subprocess.run(
        [str(exe)],
        check=True,
        cwd=REPO,
        env={**os.environ, "LTM_DUMP_MODEL": str(model), "LTM_DUMP_OUT": str(out)},
    )
    return out


def build_notebook(key: str, dump: Path) -> nbf.NotebookNode:
    spec = MODELS[key]
    nb = nbf.v4.new_notebook()
    nb.metadata["kernelspec"] = {
        "display_name": "Python 3",
        "language": "python",
        "name": "python3",
    }
    cells: list = []

    def md(source: str) -> None:
        cells.append(nbf.v4.new_markdown_cell(source.strip()))

    def code(source: str) -> None:
        cells.append(nbf.v4.new_code_cell(source.strip()))

    # -----------------------------------------------------------------
    # 0. Provenance
    # -----------------------------------------------------------------
    md(
        f"""
# Is LTM discovery exact on {spec["title"]}?

{spec["blurb"]}

Discovery's *scoring* of a loop is exact by construction -- the per-step product of
its links' recorded score series -- so "is discovery right" reduces to a question
about the candidate SET: does the engine consider every loop that could ever matter,
and does it report the right ones out of that set?

That is checkable, because the loop universe is computable independently. This
notebook:

1. Runs the engine (`pysimlin`) and records what it reports.
2. Loads `examples/ltm_search_graph_dump`'s JSON: the exact element-level edge set
   discovery consumes (from the public `ltm_finding::link_score_offsets`), each
   edge's per-step link-score series, the engine's cycle partitions, and the
   engine's reported loops with their raw and relative score series.
3. Rebuilds the **union graph** of ever-active edges and enumerates **every**
   elementary cycle that is ever *simultaneously* active -- a second, independent
   implementation of the engine's enumerator, written from the design document
   rather than translated from the Rust.
4. Scores, retains, and ranks that universe by the engine's own published rules,
   and diffs the result against the engine's report.

Regenerate with:

```bash
src/pysimlin/.venv/bin/python notebooks/build_ltm_discovery_audit.py --model {key}
```
"""
    )

    code(
        f"""
import json
import platform
import sys
import time
from collections import Counter, defaultdict
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
from IPython.display import Markdown, display

import simlin

REPO = Path.cwd().parent if Path.cwd().name == "notebooks" else Path.cwd()
MODEL_KEY = {key!r}
MODEL_PATH = REPO / {spec["rel_path"]!r}
DUMP_PATH = Path({str(dump)!r})

# The two thresholds the pipeline is defined by (`ltm_finding.rs`). Restated
# here because every retention/cap number below is measured against them; a
# drift between these and the engine's constants would show up as a
# reported-vs-recomputed disagreement in section 5, not as a silent pass.
MIN_CONTRIBUTION = 0.001
MAX_LOOPS = 200
MAX_ANCHOR_K = 3
ANCHOR_SHARE_OF_CAP = 0.5

print(f"pysimlin  {{simlin.__version__}}")
print(f"python    {{sys.version.split()[0]}} on {{platform.platform()}}")
print(f"model     {{MODEL_PATH.name}}")
print(f"dump      {{DUMP_PATH}}")
assert MODEL_PATH.exists(), MODEL_PATH
assert DUMP_PATH.exists(), f"regenerate with build_ltm_discovery_audit.py: {{DUMP_PATH}}"
"""
    )

    # -----------------------------------------------------------------
    # 1. What the engine reports
    # -----------------------------------------------------------------
    md(
        """
## 1. What the engine reports

`Model.analyze()` compiles with LTM discovery instrumentation, simulates, and runs
post-simulation loop discovery. Everything it returns is the thing under audit.
"""
    )

    code(
        """
t0 = time.perf_counter()
model = simlin.load(str(MODEL_PATH))
load_s = time.perf_counter() - t0

t0 = time.perf_counter()
analysis = model.analyze()
analyze_s = time.perf_counter() - t0

print(f"load {load_s:.2f}s | analyze() {analyze_s:.2f}s")
print(f"loops        {len(analysis.loops)}")
print(f"partitions   {len(analysis.partitions)} "
      f"(loop counts: {sorted((p.loop_count for p in analysis.partitions), reverse=True)[:8]})")
print(f"truncated    {analysis.truncated}")
print(f"agg_recovery_truncated {analysis.agg_recovery_truncated}")
print(f"enumeration_complete   {analysis.enumeration_complete}")
print(f"retained_loops         {analysis.retained_loops}")
print(f"universe_loops         {analysis.universe_loops}")
print("polarity mix:", dict(Counter(str(lp.polarity) for lp in analysis.loops)))
"""
    )

    md(
        """
### `enumeration_complete`, `retained_loops`, `universe_loops`

`DiscoveryResult::enumeration_complete` -- the engine's statement that candidate
generation was the union-graph enumeration *and* it ran to completion, so the
candidate set is the provable universe rather than the fallback's sample -- and its
two companions `retained_loops` (survivors before the report cap) and
`universe_loops` (the enumerated candidate count, `None` on a sampled run) are all
three on `pysimlin.Analysis` directly now (`Model.analyze()`'s return value, printed
above).

The dump's own copies of the same three fields come from a SEPARATE discovery run
over a separate simulation of the same model (the dump calls the engine entry point
directly rather than going through pysimlin), so the cell below cross-checks that
`analyze()` and the dump agree on all three before either is used as evidence --
content-pure discovery on an unbudgeted run should make them agree exactly, and a
disagreement here means the dump binary and the pysimlin extension were built from
different engine revisions.
"""
    )

    code(
        """
def decode_score(v):
    \"\"\"Decode one score value from the dump's non-finite-safe encoding
    (`ltm_search_graph_dump.rs`'s `score_to_json`): a JSON number stays a
    float, and the strings "nan"/"inf"/"-inf" decode to the IEEE value they
    name. Without this, `json.load` turns a bare non-finite JSON `null` (the
    default `serde_json` encoding this dump deliberately avoids) into
    Python `None`, and `np.array([None, ...], dtype=np.float64)` silently
    converts EVERY `None` to `nan` -- collapsing a genuinely infinite score
    (ACTIVE per `is_active`, a real divergent signal that multiplies through
    a partition's totals) into NaN (INACTIVE, poisons any product it is
    part of). That is exactly the distinction the union-graph active-edge
    computation below depends on, so decoding it correctly here is what
    keeps this notebook an independent check on the engine rather than one
    that quietly agrees with it by losing the same information the same way.
    \"\"\"
    if isinstance(v, str):
        return {"nan": float("nan"), "inf": float("inf"), "-inf": float("-inf")}[v]
    return float(v)


def decode_scores(vs):
    return [decode_score(v) for v in vs]


dump = json.load(open(DUMP_PATH))
T = dump["step_count"]

print(f"enumeration_complete   {dump['enumeration_complete']}")
print(f"truncated              {dump['truncated']}")
print(f"agg_recovery_truncated {dump['agg_recovery_truncated']}")
print(f"saved steps            {T}")
print(f"edges in the discovery edge set  {len(dump['edges'])}")
print(f"stocks                 {len(dump['stocks'])}")
print(f"cycle partitions       {len(dump['partitions'])} "
      f"(sizes: {sorted((len(p) for p in dump['partitions']), reverse=True)[:8]})")

assert Path(dump["model"]).name == MODEL_PATH.name, (dump["model"], MODEL_PATH)
same = len(dump["discovered"]) == len(analysis.loops)
print(f"\\ndump reported {len(dump['discovered'])} loops, analyze() reported "
      f"{len(analysis.loops)}: {'AGREE' if same else 'DISAGREE'}")
if not same:
    print("  the dump binary and the pysimlin extension were built from different "
          "engine revisions; rebuild pysimlin before reading section 5")

# pysimlin/dump completeness cross-check: `analyze()`'s Analysis fields
# (section 1, above) and the dump's copies of the same three fields come
# from two SEPARATE discovery runs, so agreement here is not definitional --
# it is what makes trusting either one (rather than re-running discovery a
# third time) justified for the rest of this notebook.
completeness_agree = (
    analysis.enumeration_complete == dump["enumeration_complete"]
    and analysis.retained_loops == dump["retained_loops"]
    and analysis.universe_loops == dump["universe_loops"]
)
print(
    f"\\npysimlin/dump completeness cross-check: "
    f"{'AGREE' if completeness_agree else 'DISAGREE'} "
    f"(enumeration_complete {analysis.enumeration_complete}=={dump['enumeration_complete']}, "
    f"retained_loops {analysis.retained_loops}=={dump['retained_loops']}, "
    f"universe_loops {analysis.universe_loops}=={dump['universe_loops']})"
)
assert completeness_agree, (
    "analyze() and the dump disagree on completeness -- rebuild pysimlin "
    "and regenerate the dump from the same engine revision before trusting "
    "either"
)
"""
    )

    # -----------------------------------------------------------------
    # 2. Union graph
    # -----------------------------------------------------------------
    md(
        """
## 2. The union graph

A loop's score is the **product** of its links' scores, so a loop can only be nonzero
at a step where *every* one of its links is active. That makes the union of
ever-active edges the right object to enumerate over: any cycle with a nonzero score
anywhere is an elementary cycle of this graph, and no per-step re-enumeration can
find anything else.

Two rules, both from `ActivityGraph::build`:

- **Active** means finite-nonzero, or infinite. Infinity is a real divergent signal;
  only NaN (no `PREVIOUS` value yet, or an undefined partial) and an exact zero are
  inactive.
- **Membership is decided over steps `1..T`.** Step 0's link scores are
  startup-degenerate, so an edge alive only there is not in the union graph.
- **Self-edges are dropped.** An elementary cycle never repeats a node, so a
  self-edge can never be part of one of length >= 2 -- and a one-variable "loop" is
  not feedback in the SD sense.
"""
    )

    code(
        """
node_ids: dict[str, int] = {}
node_names: list[str] = []


def nid(name: str) -> int:
    i = node_ids.get(name)
    if i is None:
        i = len(node_names)
        node_ids[name] = i
        node_names.append(name)
    return i


# Node ids follow the engine's: first-seen over the (sorted) link-offset list,
# then any stock not yet seen. The circuit SET does not depend on the id
# assignment, but the min-root pruning does, so matching it keeps the two
# searches comparable descent for descent.
edge_index: dict[tuple[int, int], int] = {}
pairs: list[tuple[int, int]] = []
rows: list[list[float]] = []
act_rows: list[list[float]] = []
duplicate_pairs = 0
for e in dump["edges"]:
    uv = (nid(e["from"]), nid(e["to"]))
    scores = decode_scores(e["scores"])
    # Activity is read from the series discovery reads for ACTIVITY: for a
    # module-input edge that is the NaN-shadow-repaired composite (dumped as
    # `activity_scores` where it differs), while products use `scores` --
    # exactly the split `IndexedEdge::value_at` makes in production.
    act = decode_scores(e["activity_scores"]) if "activity_scores" in e else scores
    if uv in edge_index:
        duplicate_pairs += 1
        rows[edge_index[uv]] = scores  # last wins, as `link_offset_map` does
        act_rows[edge_index[uv]] = act
    else:
        edge_index[uv] = len(pairs)
        pairs.append(uv)
        rows.append(scores)
        act_rows.append(act)
for s in dump["stocks"]:
    nid(s)
NN = len(node_names)
S_ALL = np.array(rows, dtype=np.float64)
A_ALL = np.array(act_rows, dtype=np.float64)
repaired_edges = int((S_ALL.view(np.uint64) != A_ALL.view(np.uint64)).any(axis=1).sum())

active = ((A_ALL != 0.0) & np.isfinite(A_ALL)) | np.isinf(A_ALL)
ever_active = active[:, 1:].any(axis=1)
self_edges = np.array([u == v for u, v in pairs])
in_union = ever_active & ~self_edges
union_rows = np.nonzero(in_union)[0]
U = [pairs[i] for i in union_rows]
S = S_ALL[union_rows]
UACT = active[union_rows]

print(f"nodes {NN} | discovery edges {len(pairs)} (duplicate (from,to) pairs: "
      f"{duplicate_pairs})")
print(f"ever-active edges {int(ever_active.sum())}, of which self-edges "
      f"{int((ever_active & self_edges).sum())}")
print(f"union graph: {len(U)} edges")
print(f"module-input edges whose activity is read through the composite's NaN shadow: {repaired_edges}")
print(f"active edges per step: {int(active[:, 1:].sum(axis=0).min())}.."
      f"{int(active[:, 1:].sum(axis=0).max())}")
"""
    )

    code(
        """
def tarjan_scc(adj_lists, n):
    \"\"\"Iterative Tarjan; returns (scc_of[node], component count).\"\"\"
    idx = [-1] * n
    low = [0] * n
    on = [False] * n
    scc_of = [-1] * n
    stack: list[int] = []
    work: list[list[int]] = []
    counter = comps = 0
    for root in range(n):
        if idx[root] != -1:
            continue
        idx[root] = low[root] = counter
        counter += 1
        stack.append(root)
        on[root] = True
        work.append([root, 0])
        while work:
            frame = work[-1]
            v = frame[0]
            if frame[1] < len(adj_lists[v]):
                w = adj_lists[v][frame[1]]
                frame[1] += 1
                if idx[w] == -1:
                    idx[w] = low[w] = counter
                    counter += 1
                    stack.append(w)
                    on[w] = True
                    work.append([w, 0])
                elif on[w] and idx[w] < low[v]:
                    low[v] = idx[w]
            else:
                work.pop()
                if work and low[v] < low[work[-1][0]]:
                    low[work[-1][0]] = low[v]
                if low[v] == idx[v]:
                    while True:
                        w = stack.pop()
                        on[w] = False
                        scc_of[w] = comps
                        if w == v:
                            break
                    comps += 1
    return scc_of, comps


adj: list[list[tuple[int, int]]] = [[] for _ in range(NN)]
radj: list[list[int]] = [[] for _ in range(NN)]
uedge_index: dict[tuple[int, int], int] = {}
for row, (u, v) in enumerate(U):
    adj[u].append((v, row))
    radj[v].append(u)
    uedge_index[(u, v)] = row

scc_of, n_scc = tarjan_scc([[w for w, _ in adj[u]] for u in range(NN)], NN)
scc_sizes = Counter(scc_of)
nontrivial = sorted((n for n in scc_sizes.values() if n > 1), reverse=True)
print(f"union-graph SCCs: {n_scc} total, {len(nontrivial)} non-trivial")
print(f"non-trivial sizes: {nontrivial}")
print(f"nodes inside a non-trivial SCC: "
      f"{sum(n for n in scc_sizes.values() if n > 1)} of {NN}")
"""
    )

    # -----------------------------------------------------------------
    # 3. Exact enumeration
    # -----------------------------------------------------------------
    md(
        """
## 3. Exact enumeration of the ever-simultaneously-active cycles

Min-root Tiernan search, one root per node in ascending id order. Three things make
it exact and affordable at once:

- **Min-root canonicalization.** Only nodes with id `>= root` are explorable, so each
  cycle is emitted exactly once, rooted at its minimum node.
- **Per-root induced SCC** (Johnson's `A_k`): the strongly-connected component of
  `root` in the subgraph induced by nodes `>= root`. Every cycle rooted at `root`
  lies entirely inside it, so restricting to it drops nothing and removes the
  dead-end wandering that dominates a naive search.
- **Activity bitsets.** Each edge carries a bitset over saved steps; the running AND
  along a path is nonempty exactly where the whole path is simultaneously active. A
  branch whose AND empties is pruned -- no extension of it can ever score nonzero.
  Bit 0 is carried but masked out of the emptiness test, matching the engine
  (step-0-only activity is not a scorable loop).

Bitsets are plain Python ints here, which makes the AND a single machine-word-batched
operation regardless of how many saved steps there are.
"""
    )

    code(
        """
# Per-edge activity bitsets, as Python ints.
abits = [0] * len(U)
for i in range(len(U)):
    v = 0
    for t in np.nonzero(UACT[i])[0]:
        v |= 1 << int(t)
    abits[i] = v
NOT_STEP0 = ~1  # step 0 never makes a circuit scorable

t0 = time.perf_counter()
circuit_rows: list[tuple[int, ...]] = []
visits = 0
for root in range(NN):
    root_scc = scc_of[root]

    # Nodes >= root, in root's union SCC, reachable FROM root...
    reach = bytearray(NN)
    reach[root] = 1
    stack = [root]
    while stack:
        v = stack.pop()
        for w, _ in adj[v]:
            if w >= root and scc_of[w] == root_scc and not reach[w]:
                reach[w] = 1
                stack.append(w)
    # ...intersected with the nodes that reach root: root's induced SCC.
    inscc = bytearray(NN)
    inscc[root] = 1
    stack = [root]
    size = 0
    while stack:
        v = stack.pop()
        size += 1
        for u in radj[v]:
            if u >= root and scc_of[u] == root_scc and reach[u] and not inscc[u]:
                inscc[u] = 1
                stack.append(u)
    if size < 2:
        continue

    sub = [None] * NN
    for v in range(NN):
        if inscc[v]:
            sub[v] = [(w, r) for w, r in adj[v] if inscc[w]]

    on_path = bytearray(NN)
    on_path[root] = 1
    frames = [[root, 0]]
    edge_path: list[int] = []
    and_stack = [(1 << T) - 1]
    while frames:
        frame = frames[-1]
        v = frame[0]
        out_edges = sub[v]
        if frame[1] < len(out_edges):
            w, row = out_edges[frame[1]]
            frame[1] += 1
            visits += 1
            if w != root and on_path[w]:
                continue
            running = and_stack[-1] & abits[row]
            if not (running & NOT_STEP0):
                continue
            if w == root:
                circuit_rows.append(tuple(edge_path) + (row,))
            else:
                on_path[w] = 1
                edge_path.append(row)
                and_stack.append(running)
                frames.append([w, 0])
        else:
            frames.pop()
            on_path[v] = 0
            if edge_path:
                edge_path.pop()
                and_stack.pop()

enum_s = time.perf_counter() - t0
n_cyc = len(circuit_rows)
lens = np.array([len(c) for c in circuit_rows], dtype=np.int64)
print(f"elementary cycles ever simultaneously active: {n_cyc}")
print(f"  enumerated in {enum_s:.1f}s over {visits} edge visits")
if n_cyc:
    print(f"  length {lens.min()}..{lens.max()}, mean {lens.mean():.1f}; "
          f"total edge rows {int(lens.sum())}")
"""
    )

    md(
        """
### Cross-aggregate loops are stitched from petals, as the engine does

A feedback loop through a hoisted reducer can visit the synthetic aggregate node
twice (`pop[a] -> agg -> growth[b] -> pop[b] -> agg -> growth[a] -> pop[a]`), so it
is not an elementary cycle and no elementary-circuit enumeration emits it. The
engine recovers these by **stitching**: every elementary cycle that visits exactly one
aggregate node is a *petal*; for each aggregate, each subset of two or more petals
with pairwise-disjoint internal node sets is one stitched loop, emitted once per
subset (all cyclic orderings of a subset share one edge multiset and so one score).
The stitched loops join the candidate set BEFORE retention. The cell below mirrors
that rule -- petal priority by internal-set size then node-id sequence, subsets by
cardinality then mask, one concatenation in priority order -- so the universe it
scores is the population the engine's denominators sum.
"""
    )

    code(
        """
AGG_PREFIX = "$\\u205altm\\u205aagg\\u205a"
is_agg_node = [node_names[n].startswith(AGG_PREFIX) for n in range(NN)]
U_FROM_ = [u for u, _ in U]
MAX_AGG_PETALS = 16
CROSS_AGG_LOOP_BUDGET = 10_000

def nodes_of_rows(rws):
    return tuple(U_FROM_[r] for r in rws)

petals_by_agg: dict[int, list] = defaultdict(list)
for rws in circuit_rows:
    nodes = nodes_of_rows(rws)
    aggs = [n for n in nodes if is_agg_node[n]]
    if len(aggs) != 1:
        continue
    a = aggs[0]
    k = nodes.index(a)
    rotated = nodes[k:] + nodes[:k]
    internal = frozenset(rotated[1:])
    # Dedup on the rotation-invariant internal set, as the engine does.
    if any(p[1] == internal for p in petals_by_agg[a]):
        continue
    petals_by_agg[a].append((rotated, internal))

stitched_seqs: list[tuple[int, ...]] = []
emitted = 0
for a in sorted(petals_by_agg):
    petals = petals_by_agg[a]
    if len(petals) < 2:
        continue
    petals.sort(key=lambda p: (len(p[1]), p[0]))
    petals = petals[:MAX_AGG_PETALS]
    k = len(petals)
    masks = sorted(range(1 << k), key=lambda m: (bin(m).count("1"), m))
    for m in masks:
        if bin(m).count("1") < 2:
            continue
        chosen = [i for i in range(k) if (m >> i) & 1]
        union: set = set()
        ok = True
        for i in chosen:
            if petals[i][1] & union:
                ok = False
                break
            union |= petals[i][1]
        if not ok:
            continue
        seq = tuple(n for i in chosen for n in petals[i][0])
        stitched_seqs.append(seq)
        emitted += 1
        if emitted >= CROSS_AGG_LOOP_BUDGET:
            break

# A stitched sequence's edges all exist in the union graph; append each as
# one more candidate (edge rows, wrapping), exactly as the engine pushes it.
n_stitched = 0
for seq in stitched_seqs:
    rws = []
    for i in range(len(seq)):
        uv = (seq[i], seq[(i + 1) % len(seq)])
        rws.append(uedge_index[uv])
    circuit_rows.append(tuple(rws))
    n_stitched += 1
n_cyc = len(circuit_rows)
lens = np.array([len(c) for c in circuit_rows], dtype=np.int64)
print(f"aggregate nodes with petals: {len(petals_by_agg)}; stitched cross-agg loops: {n_stitched}")
print(f"candidate universe (elementary cycles + stitched loops): {n_cyc}")
"""
    )

    md(
        """
### What this count is, and is not

This is the count of elementary cycles that are **ever simultaneously active** --
the loop universe of the recorded series. It is smaller than the union graph's raw
elementary-cycle count, which ignores whether a cycle's links are ever alive at the
same step: a cycle whose edges are each active but never together scores exactly zero
at every saved step and is not a loop that can matter.

Earlier notes on World3 quote ~330k cycles. That figure is the *unconstrained* count,
enumerated without the simultaneity requirement; the constrained universe is
materially smaller, and it is the one the engine (and this notebook) enumerate. Both
numbers are correct answers to different questions, and only the constrained one is a
candidate set.

The remaining scope limit is shared with the engine and with the LTM literature:
activity is sampled at **saved-step** resolution, so a loop that lives only between
saves is invisible to both this audit and the thing it audits.
"""
    )

    # -----------------------------------------------------------------
    # 4. Scoring, retention, ranking
    # -----------------------------------------------------------------
    md(
        """
## 4. Scoring, retention and ranking over the whole universe

Now the same pipeline the engine runs, over the full enumerated set:

1. **Score** each cycle exactly: the per-step product of its edges' signed series,
   in traversal order (float64, so the arithmetic is the engine's arithmetic). A NaN
   product -- including the `Inf * 0` case, which has no NaN link anywhere -- is
   excluded from every total and can satisfy no threshold.
2. **Partition** each cycle by the first stock on it. Every stock of a cycle shares
   its cycle partition by construction (a partition IS a stock-to-stock SCC), so
   "first" is "the". A cycle with no stock gets its own Solo group.
3. **Universe totals**: per partition, per step, the sum of `|score|` over ALL
   enumerated cycles. This is the denominator the engine's `UniverseStats.totals`
   carries (passed to `rank_and_filter` as `universe: Option<&UniverseStats>`).
4. **Retention**: keep a cycle iff at some step its `|score|` is at least
   `MIN_CONTRIBUTION` (0.1%) of its partition's total there. Solo cycles are their
   own denominator and are kept whenever they are ever active.
5. **Rank** competitive-first: cycles whose partition holds at least two loops of
   the UNIVERSE come first, by mean relative contribution over their active steps;
   Solo-group cycles and cycles alone in their partition's universe (whose relative
   score is `+/-1` by construction, carrying no discriminative information) come
   last. A retention non-survivor still holds mass in its partition's denominator,
   so it still makes that partition a place where loops compete -- which is why the
   classification asks the universe rather than the survivor set.
6. **Select** under `MAX_LOOPS`, coverage-aware: within every competing group, each
   step's largest-`|relative score|` survivor is ANCHORED and keeps its slot
   (`k = 1`, unconditional); `k` rises past 1, bounded by `MAX_ANCHOR_K`, but only
   while the ENLARGED anchor set stays at or under `ANCHOR_SHARE_OF_CAP` (one half)
   of the cap, so anchoring can grow coverage without ever crowding the ordinary
   ranking below half of a capped report. Remaining slots go to the ranking in
   order. Membership is the only thing this changes -- the reported list is still
   presented in the ranking's order.
"""
    )

    code(
        """
t0 = time.perf_counter()
flat = np.concatenate([np.asarray(c, dtype=np.int32) for c in circuit_rows])
offs = np.zeros(n_cyc + 1, dtype=np.int64)
np.cumsum(lens, out=offs[1:])

# Products, chunked by cycle length so each chunk is one rectangular gather.
# Multiplied edge-by-edge in traversal order, which is the engine's order, so
# the two float64 results are bit-comparable rather than merely close.
score = np.empty((n_cyc, T), dtype=np.float64)
by_len = np.argsort(lens, kind="stable")
pos = 0
while pos < n_cyc:
    L = int(lens[by_len[pos]])
    end = pos
    while end < n_cyc and lens[by_len[end]] == L:
        end += 1
    group = by_len[pos:end]
    chunk = max(1, int(4e7 / max(L * T, 1)))
    for lo in range(0, len(group), chunk):
        idx = group[lo:lo + chunk]
        rowmat = np.stack([flat[offs[c]:offs[c] + L] for c in idx])
        p = S[rowmat[:, 0]].copy()
        for j in range(1, L):
            p *= S[rowmat[:, j]]
        score[idx] = p
    pos = end
score_s = time.perf_counter() - t0

U_FROM = np.array([u for u, _ in U], dtype=np.int32)
cyc_nodes = [tuple(int(U_FROM[r]) for r in flat[offs[c]:offs[c + 1]])
             for c in range(n_cyc)]

part_of_node: dict[int, int] = {}
for pi, stock_list in enumerate(dump["partitions"]):
    for s in stock_list:
        if s in node_ids:
            part_of_node[node_ids[s]] = pi
cyc_part = np.full(n_cyc, -1, dtype=np.int64)
for c in range(n_cyc):
    for node in cyc_nodes[c]:
        p = part_of_node.get(node)
        if p is not None:
            cyc_part[c] = p
            break

mass = np.where(np.isnan(score), 0.0, np.abs(score))  # NaN contributes nothing

# Reported-cycle identity: the engine trims synthetic aggregate nodes
# (`$⁚ltm⁚agg⁚…`) out of a reported loop, so a direct pathway and its
# hoisted-reducer twin are ONE reported loop -- and only the strongest
# representative (largest mean |score|) banks mass, counts, or can be reported.
# The audit applies the same rule before totals are formed, so its universe is
# the same population the engine's denominators sum.
AGG_PREFIX = "$\u205altm\u205aagg\u205a"


def trimmed_names(c: int) -> tuple[str, ...]:
    return tuple(node_names[n] for n in cyc_nodes[c]
                 if not node_names[n].startswith(AGG_PREFIX))


def rotate_min(names: tuple[str, ...]) -> tuple[str, ...]:
    if not names:
        return names
    k = names.index(min(names))
    return names[k:] + names[:k]


valid_any = ~np.isnan(score)
avg_abs_all = np.where(valid_any.any(axis=1),
                       np.nansum(np.abs(score), axis=1) / np.maximum(valid_any.sum(axis=1), 1),
                       0.0)
representative = np.ones(n_cyc, dtype=bool)
by_identity: dict[tuple[str, ...], int] = {}
for c in range(n_cyc):
    key = rotate_min(trimmed_names(c))
    if not key:
        representative[c] = False   # trims to nothing: not a reportable loop
        continue
    prev = by_identity.get(key)
    if prev is None:
        by_identity[key] = c
    elif avg_abs_all[c] > avg_abs_all[prev]:
        representative[prev] = False
        by_identity[key] = c
    else:
        representative[c] = False
mass[~representative] = 0.0             # a merged twin banks nothing
mass[(mass == 0.0).all(axis=1)] = 0.0    # (no-op, for clarity: massless banks nothing)
totals = {int(p): mass[(cyc_part == p) & representative].sum(axis=0)
          for p in np.unique(cyc_part) if p >= 0}
print(f"cycles merged into a stronger twin's reported identity: {int((~representative).sum())}")

print(f"scored {n_cyc} cycles in {score_s:.1f}s")
print(f"cycles with no stock (Solo groups): {int((cyc_part < 0).sum())}")
print(f"partitions carrying enumerated mass: {len(totals)}")
"""
    )

    code(
        """
kept = np.zeros(n_cyc, dtype=bool)
for p, tot in totals.items():
    sel = np.nonzero(cyc_part == p)[0]
    safe = np.where(tot > 0, tot, np.inf)
    kept[sel] = (mass[sel] / safe[None, :] >= MIN_CONTRIBUTION).any(axis=1)
# Solo: own denominator, ratio 1 wherever it carries mass; a Solo loop with no
# mass at any step is not a loop the universe holds.
kept[cyc_part < 0] = (mass[cyc_part < 0] != 0.0).any(axis=1)
kept &= representative
survivors = np.nonzero(kept)[0]

# Normalization group per survivor: its cycle partition, or its own Solo group.
group_of = {int(c): (("P", int(cyc_part[c])) if cyc_part[c] >= 0 else ("S", int(c)))
            for c in survivors}
group_members: dict[tuple, list[int]] = defaultdict(list)
for c in survivors:
    group_members[group_of[int(c)]].append(int(c))


def group_totals(g):
    return totals[g[1]] if g[0] == "P" else mass[g[1]]


def group_label(g) -> str:
    return f"partition {g[1]}" if g[0] == "P" else f"solo group (cycle {g[1]})"


def canonical(c: int) -> tuple[str, ...]:
    \"\"\"Lexicographically minimal rotation of the cycle's REPORTED node names
    (synthetic aggregate nodes trimmed, as the engine reports them).

    Rotation, not sorting: two distinct directed cycles over the same node set
    are different loops and must not collide on one key.\"\"\"
    return rotate_min(trimmed_names(c))


# How many loops divide each partition's denominator, over the whole
# UNIVERSE -- a retention non-survivor still holds mass there, so it still
# makes its partition a place where loops compete.
massy = representative & (mass != 0.0).any(axis=1)
universe_counts = {int(p): int(((cyc_part == p) & massy).sum())
                   for p in np.unique(cyc_part) if p >= 0}

ranked = []
for c in survivors:
    c = int(c)
    g = group_of[c]
    tot = group_totals(g)
    s = score[c]
    live = (~np.isnan(s)) & (tot > 0)
    rel = np.abs(s[live]) / tot[live]
    rel = rel[~np.isnan(rel)]
    mean_rel = rel.mean() if rel.size else np.nan
    valid = ~np.isnan(s)
    avg_abs = float(np.abs(s[valid]).mean()) if valid.any() else 0.0
    key = canonical(c)
    with np.errstate(invalid="ignore", divide="ignore"):
        r_signed = np.where(tot == 0.0, 0.0, s / np.where(tot == 0.0, 1.0, tot))
    ranked.append({
        "cycle": c,
        "mean_rel": float(mean_rel),
        "competing": g[0] == "P" and universe_counts.get(g[1], 0) >= 2,
        "avg_abs": avg_abs,
        "group": g,
        "rel_abs": np.nan_to_num(np.abs(r_signed), nan=0.0),
        "content_key": ("_".join(sorted(set(key))), key),
        "key": key,
    })

# The engine's `cmp_relative_importance`: active before never-active, competing
# before solo, descending mean relative contribution, descending raw magnitude,
# then the content key.
ranked.sort(key=lambda r: (
    1 if np.isnan(r["mean_rel"]) else 0,
    0 if r["competing"] else 1,
    -(r["mean_rel"] if not np.isnan(r["mean_rel"]) else 0.0),
    -r["avg_abs"],
    r["content_key"],
))


def anchor_depths(rows):
    # Smallest k at which each ranked row is some step's top-k in its group,
    # 0 for a row that never is. A step no member is active at anchors
    # nobody: the mass there belongs to loops outside the retained set, so no
    # retained loop dominated it. Solo groups never anchor -- their relative
    # score is +/-1 at every active step by construction.
    depth = [0] * len(rows)
    groups = {}
    for i, r in enumerate(rows):
        if r["competing"]:
            groups.setdefault(r["group"], []).append(i)
    for members in groups.values():
        sub = np.stack([rows[i]["rel_abs"] for i in members])
        # Stable sort, so a tie for a place goes to the earlier-ranked row.
        order = np.argsort(-sub, axis=0, kind="stable")
        for t in range(sub.shape[1]):
            place = 0
            for slot in range(min(MAX_ANCHOR_K, len(members))):
                row = int(order[slot, t])
                if sub[row, t] <= 0.0:
                    break
                place += 1
                i = members[row]
                if depth[i] == 0 or depth[i] > place:
                    depth[i] = place
    return depth


def select_reported(rows, cap):
    # The engine's coverage-aware cap, as positions into `rows`.
    if len(rows) <= cap:
        return list(range(len(rows)))
    depth = anchor_depths(rows)
    count_at = lambda k: sum(1 for d in depth if d != 0 and d <= k)
    if count_at(1) > cap:
        # Pathological: anchors alone overflow, so the cap applies to them.
        return [i for i, d in enumerate(depth) if d == 1][:cap]
    # k=1 is the unconditional guarantee, exempt from the share bound; k may
    # rise past it only while the resulting anchor set stays at or under
    # ANCHOR_SHARE_OF_CAP of the cap, so anchoring can never crowd the
    # ordinary mean-relative ranking below half of a capped report.
    anchor_cap = cap * ANCHOR_SHARE_OF_CAP
    k = 1
    for kk in range(2, MAX_ANCHOR_K + 1):
        if count_at(kk) > anchor_cap:
            break
        k = kk
    keep = [d != 0 and d <= k for d in depth]
    chosen = sum(keep)
    for i in range(len(keep)):
        if chosen >= cap:
            break
        if not keep[i]:
            keep[i] = True
            chosen += 1
    return [i for i, v in enumerate(keep) if v]


selected = select_reported(ranked, MAX_LOOPS)
py_top = [ranked[i]["key"] for i in selected]
py_top_set = set(py_top)
cycle_of_key = {r["key"]: r["cycle"] for r in ranked}
depths = anchor_depths(ranked)

print(f"retention survivors: {len(survivors)} of {n_cyc} "
      f"({100 * len(survivors) / max(n_cyc, 1):.1f}% of the universe)")
print(f"survivors that compete (universe holds >= 2 loops in the partition): "
      f"{sum(1 for r in ranked if r['competing'])}")
print(f"cap binds: {len(survivors) > MAX_LOOPS} "
      f"({max(0, len(survivors) - MAX_LOOPS)} survivors have no slot)")
print("anchored survivors by k: "
      + ", ".join(f"k<={k}: {sum(1 for d in depths if d != 0 and d <= k)}"
                  for k in range(1, MAX_ANCHOR_K + 1)))
pd.DataFrame([
    {"rank": i, "mean rel": r["mean_rel"], "competing": r["competing"],
     "len": len(r["key"]),
     "loop": " -> ".join(n.split("[")[0] for n in r["key"][:4])
             + (" ..." if len(r["key"]) > 4 else "")}
    for i, r in enumerate(ranked[:10])
])
"""
    )

    # -----------------------------------------------------------------
    # 5. Cross-check against the engine
    # -----------------------------------------------------------------
    md(
        """
## 5. Cross-check against the engine

Four independent comparisons, in increasing order of what a disagreement would mean:

1. **Is every engine-reported loop a real cycle of the universe?** A loop the
   independent enumeration never found would be a fabricated loop.
2. **Do the raw score series agree?** Both sides multiply the same recorded link
   scores; a difference means one of them is multiplying the wrong things. (A loop
   through a module instance is the one legitimate exception: the engine reports the
   per-exit-port override series rather than the raw product, so the cell reports how
   many loops that applies to rather than assuming none.)
3. **Do the relative score series agree?** This is the ranking and dominance
   statistic, and it depends on the *denominator* being the whole universe. It is the
   comparison that would catch a retention pass that quietly dropped mass.
4. **Does the engine's reported list match the independently-ranked top-200?**
"""
    )

    code(
        """
engine_keys = []
for d in dump["discovered"]:
    names = tuple(d["nodes"])
    k = names.index(min(names))
    engine_keys.append(names[k:] + names[:k])
engine_set = set(engine_keys)
assert len(engine_set) == len(engine_keys), "the engine dedupes reported cycles"

not_in_universe = [k for k in engine_keys if k not in cycle_of_key]
print(f"engine loops absent from the independent universe: {len(not_in_universe)} "
      f"of {len(engine_keys)}")

overlap = len(engine_set & py_top_set)
print(f"reported-200 overlap: {overlap}/{len(engine_set)} of the engine's reported "
      f"loops are in the independent top-{MAX_LOOPS} "
      f"(independent list holds {len(py_top_set)})")

rank_of = {r["key"]: i for i, r in enumerate(ranked)}
stat_of = {r["key"]: r for r in ranked}
only_engine = [k for k in engine_keys if k not in py_top_set]
only_python = [k for k in py_top if k not in engine_set]
if only_engine or only_python:
    diff = pd.DataFrame(
        [{"side": side, "independent rank": rank_of.get(k),
          "mean rel": stat_of[k]["mean_rel"] if k in stat_of else None,
          "loop": " -> ".join(n.split("[")[0] for n in k[:3])}
         for side, keys in (("engine only", only_engine),
                            ("independent only", only_python))
         for k in keys]
    ).sort_values("independent rank", na_position="last")
    display(diff.head(20))
else:
    print("the two lists are identical as sets")
"""
    )

    code(
        """
raw_max = 0.0
rel_max = 0.0
raw_mismatched = []
for d in dump["discovered"]:
    names = tuple(d["nodes"])
    k = names.index(min(names))
    key = names[k:] + names[:k]
    c = cycle_of_key.get(key)
    if c is None:
        continue

    e_raw = np.array(decode_scores(d["scores"]), dtype=np.float64)
    p_raw = score[c]
    both = (~np.isnan(e_raw)) & (~np.isnan(p_raw))
    if both.any():
        denom = np.maximum(np.abs(e_raw[both]), 1e-300)
        worst = float((np.abs(e_raw[both] - p_raw[both]) / denom).max())
        raw_max = max(raw_max, worst)
        if worst > 1e-9:
            raw_mismatched.append((worst, key))

    if d["rel_scores"]:
        g = group_of.get(c)
        if g is not None:
            tot = group_totals(g)
            p_rel = np.where(tot == 0.0, 0.0, p_raw / np.where(tot == 0.0, 1.0, tot))
            e_rel = np.array(decode_scores(d["rel_scores"]), dtype=np.float64)
            both = (~np.isnan(e_rel)) & (~np.isnan(p_rel))
            if both.any():
                rel_max = max(rel_max, float(np.abs(e_rel[both] - p_rel[both]).max()))

print(f"max relative difference in raw loop scores: {raw_max:.3e} "
      f"({len(raw_mismatched)} loops differ by more than 1e-9)")
print(f"max |rel score| difference: {rel_max:.3e}")
if raw_mismatched:
    print("  loops whose reported score is NOT the raw product of their links "
          "(module-override series, or a genuine defect):")
    for worst, key in sorted(raw_mismatched, reverse=True)[:5]:
        print(f"    {worst:.2e}  " + " -> ".join(n.split("[")[0] for n in key[:4]))
"""
    )

    md(
        """
### Step-dominant coverage

At each saved step, which loop has the largest relative importance -- and is it in the
engine's report? A report that misses the step-dominant loop names the wrong loop for
that step, which is what a dominance-over-time reading is entirely made of.

The measurement has to be taken **within a competing group**, not globally. A loop
alone in its normalization group is its own denominator, so its relative score is
`+/-1` at every step it is active, by construction. A global argmax therefore returns
such a loop at essentially every step and measures nothing about the model -- exactly
the degeneracy the competitive-first ranking exists to prevent. Both numbers are
reported below so the artifact is visible rather than implicit.

The per-competing-group number is the one that matters: it is what a coverage-aware
cap has to drive to 100%.
"""
    )

    code(
        """
rel_abs = np.zeros((len(survivors), T))
pos_of = {}
for i, c in enumerate(survivors):
    c = int(c)
    pos_of[c] = i
    tot = group_totals(group_of[c])
    with np.errstate(invalid="ignore", divide="ignore"):
        r = np.where(tot == 0.0, 0.0, score[c] / np.where(tot == 0.0, 1.0, tot))
    rel_abs[i] = np.nan_to_num(np.abs(r), nan=0.0)

# Global argmax (the degenerate view).
competing_of = {r["cycle"]: r["competing"] for r in ranked}
g_arg = rel_abs.argmax(axis=0)
g_max = rel_abs.max(axis=0)
g_steps = [t for t in range(1, T) if g_max[t] > 0]
g_cov = sum(1 for t in g_steps
            if canonical(int(survivors[g_arg[t]])) in engine_set)
g_solo = sum(1 for t in g_steps if not competing_of[int(survivors[g_arg[t]])])
print(f"global argmax: {g_cov}/{len(g_steps)} steps covered "
      f"({g_solo}/{len(g_steps)} of those argmaxes are non-competing loops whose "
      f"relative score is +/-1 by construction)")

# Per-competing-group argmax (the meaningful view).
covered = total_steps = 0
per_group = []
group_series = {}
for g, members in sorted(group_members.items(), key=lambda kv: -len(kv[1])):
    if len(members) < 2:
        continue
    sub = rel_abs[[pos_of[c] for c in members]]
    arg = sub.argmax(axis=0)
    peak = sub.max(axis=0)
    hit = np.zeros(T, dtype=bool)
    live = np.zeros(T, dtype=bool)
    for t in range(1, T):
        if peak[t] <= 0:
            continue
        live[t] = True
        hit[t] = canonical(members[arg[t]]) in engine_set
    group_series[g] = (peak, live, hit)
    covered += int(hit.sum())
    total_steps += int(live.sum())
    per_group.append({"group": group_label(g), "survivors": len(members),
                      "steps live": int(live.sum()), "dominant covered": int(hit.sum())})

coverage_pct = 100 * covered / total_steps if total_steps else float("nan")
print(f"step-dominant coverage (within competing groups): {covered}/{total_steps}"
      + (f" ({coverage_pct:.1f}%)" if total_steps else ""))
display(pd.DataFrame(per_group))
"""
    )

    code(
        """
if group_series:
    biggest = max(group_series, key=lambda g: len(group_members[g]))
    peak, live, hit = group_series[biggest]
else:
    # No competing group at all (every survivor alone in its normalization
    # group). The plot then shows the degenerate global view, which is the
    # honest picture of a model with no competition to measure.
    biggest = ("G", -1)
    peak = g_max
    live = np.zeros(T, dtype=bool)
    hit = np.zeros(T, dtype=bool)
    for t in g_steps:
        live[t] = True
        hit[t] = canonical(int(survivors[g_arg[t]])) in engine_set
steps = np.arange(T)
fig, ax = plt.subplots(figsize=(10, 3.6))
ok = live & hit
miss = live & ~hit
ax.scatter(steps[ok], peak[ok], s=8, label="dominant loop reported")
ax.scatter(steps[miss], peak[miss], s=14, color="crimson",
           label="dominant loop MISSING from the report")
label = "every survivor (no competing group)" if biggest[0] == "G" else group_label(biggest)
ax.set_title(f"{MODEL_KEY}: per-step dominance within {label}, "
             f"and whether the engine reported it")
ax.set_xlabel("saved step")
ax.set_ylabel("max relative importance")
ax.legend()
ax.grid(alpha=0.3)
plt.tight_layout()
plt.show()
print(f"{label}: {int(miss.sum())} of {int(live.sum())} live steps have a "
      f"dominant loop the engine did not report")
"""
    )

    # -----------------------------------------------------------------
    # 6. Verdict
    # -----------------------------------------------------------------
    code(
        """
verdict_rows = [
    ("elementary cycles ever simultaneously active", f"{n_cyc}"),
    ("retention survivors", f"{len(survivors)}"),
    ("engine reported loops", f"{len(engine_set)}"),
    ("engine loops absent from the independent universe", f"{len(not_in_universe)}"),
    ("reported-200 overlap", f"{overlap}/{len(engine_set)}"),
    ("max relative difference in raw loop scores", f"{raw_max:.3e}"),
    ("max |rel score| difference", f"{rel_max:.3e}"),
    ("step-dominant coverage (competing groups)", f"{covered}/{total_steps}"),
    ("enumeration_complete", f"{analysis.enumeration_complete}"),
    ("retained_loops (pysimlin)", f"{analysis.retained_loops}"),
    ("universe_loops (pysimlin)", f"{analysis.universe_loops}"),
]
for label, value in verdict_rows:
    print(f"{label}: {value}")

exact_set = (len(not_in_universe) == 0 and overlap == len(engine_set))
scores_exact = raw_max <= 1e-9 and rel_max <= 1e-9
# The two counts that expose an omission the set comparisons cannot: an
# engine that dropped universe cycles below retention or outside the top-200
# could still have every reported loop inside `cycle_of_key`, but its
# universe_loops / retained_loops would then differ from the independent
# mass-bearing universe and survivor counts.
independent_universe = int(massy.sum())
counts_agree = (analysis.universe_loops == independent_universe
                and analysis.retained_loops == len(survivors))
gap = total_steps - covered
audit_pass = bool(analysis.enumeration_complete and exact_set and scores_exact
                  and counts_agree)
print(f"independent mass-bearing universe: {independent_universe} "
      f"(pysimlin universe_loops {analysis.universe_loops}); independent survivors "
      f"{len(survivors)} (pysimlin retained_loops {analysis.retained_loops}): "
      f"{'AGREE' if counts_agree else 'DISAGREE'}")
print(f"AUDIT VERDICT: {'PASS' if audit_pass else 'FAIL'} "
      f"(enumeration_complete={analysis.enumeration_complete}, exact_set={exact_set}, "
      f"scores_exact={scores_exact}, counts_agree={counts_agree})")
display(Markdown(f\"\"\"
### Verdict

- **Candidate generation**: `pysimlin.Analysis.enumeration_complete` is
  `{analysis.enumeration_complete}` (cross-checked against the dump's own copy,
  section 1), and an independent enumeration of the same union
  graph finds **{n_cyc}** ever-simultaneously-active elementary cycles. Every one of
  the engine's {len(engine_set)} reported loops
  {'is' if len(not_in_universe) == 0 else 'is NOT all'} in that universe
  ({len(not_in_universe)} absent).
- **Scoring**: raw loop scores agree to {raw_max:.1e} relative, partition-relative
  scores to {rel_max:.1e} absolute -- {'exact' if scores_exact else 'NOT exact'}.
  Both sides compute the product of the same recorded series, so this is a check that
  the engine multiplies the links it says it does, and that the denominators are the
  whole universe.
- **Selection**: {len(survivors)} of {n_cyc} cycles clear the 0.1% retention
  threshold, and the engine reports {len(engine_set)}. The independently-ranked
  top-{MAX_LOOPS} and the engine's list agree on {overlap}
  {'-- selection is reproducible from the published rules' if exact_set else 'and disagree elsewhere'}.
- **Coverage**: at **{gap}** of {total_steps} live steps the most dominant loop
  within a competing group is absent from the report
  ({coverage_pct:.1f}% covered). That gap is a property of the
  `MAX_LOOPS` cap, not of candidate generation: every one of those loops was
  enumerated and retained, and only the cap dropped it. It is the number a
  coverage-aware cap has to drive to zero.
\"\"\"))
"""
    )

    nb["cells"] = cells
    return nb


def execute(nb: nbf.NotebookNode, path: Path) -> None:
    import nbformat
    from nbclient import NotebookClient

    path.write_text(nbformat.writes(nb), encoding="utf-8")
    print(f"wrote {path} ({len(nb['cells'])} cells)")

    client = NotebookClient(
        nb,
        timeout=3600,
        kernel_name="python3",
        resources={"metadata": {"path": str(NOTEBOOKS_DIR)}},
    )
    client.execute()
    path.write_text(nbformat.writes(nb), encoding="utf-8")
    print(f"executed and saved {path}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--model",
        choices=sorted(MODELS),
        action="append",
        help="model to audit (repeatable); default: all",
    )
    parser.add_argument(
        "--skip-dump",
        action="store_true",
        help="reuse the existing dump JSON instead of rebuilding and re-running "
        "the engine example (for iterating on the notebook itself)",
    )
    args = parser.parse_args()
    keys = args.model or sorted(MODELS)

    for key in keys:
        path = dump_path(cargo_target_dir(), key)
        if not args.skip_dump:
            path = regenerate_dump(key)
        elif not path.exists():
            raise SystemExit(f"--skip-dump but no dump at {path}")
        execute(build_notebook(key, path), notebook_path(key))
    return 0


if __name__ == "__main__":
    sys.exit(main())
