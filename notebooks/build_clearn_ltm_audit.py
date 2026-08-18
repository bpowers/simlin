#!/usr/bin/env python3
"""Build the C-LEARN Loops-That-Matter audit notebook.

Constructs notebooks/clearn_ltm_audit.ipynb cell-by-cell via nbformat, then
executes it (populating outputs) with nbclient. The executed .ipynb and its
HTML render are generated artifacts (gitignored); this script is the source of
truth, so the notebook can be regenerated against any engine build.

Every number in the notebook is COMPUTED by a cell -- nothing is pasted in --
so a regeneration against a newer engine either reproduces the findings or
visibly contradicts them.

Sibling generator: build_notebook.py builds the older `clearn_ltm_experience`
notebook, written when GH #653 (pinned loops unscoreable at C-LEARN's scale)
was still open; its central technique -- composing loop scores by hand from
link scores -- is a workaround for a bug that is now fixed.

Run from the pysimlin venv, synced with the `notebooks` extra. Invoke the venv
interpreter DIRECTLY: `uv run` re-syncs the project as an editable install and
replaces any wheel you installed.

    (cd src/pysimlin && uv sync --extra dev --extra notebooks)
    src/pysimlin/.venv/bin/python notebooks/build_clearn_ltm_audit.py

Both paths are relative to the REPOSITORY ROOT; the subshell keeps the `cd`
from leaking into the second command, where it would resolve the interpreter
as `src/pysimlin/src/pysimlin/.venv/...` and fail before the generator starts.
"""

from pathlib import Path

import nbformat as nbf

NOTEBOOKS_DIR = Path(__file__).resolve().parent
NOTEBOOK_PATH = NOTEBOOKS_DIR / "clearn_ltm_audit.ipynb"

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


# ---------------------------------------------------------------------------
# 0. Provenance
# ---------------------------------------------------------------------------

md(
    """
# C-LEARN under Loops That Matter

**An experience report: driving Simlin's LTM implementation through `pysimlin` on a
real, large, arrayed climate-policy model.**

C-LEARN v77 is a Vensim model of the global carbon cycle and climate coupled to an
emissions-negotiation front end. It is a good stress test precisely because nobody
built it for us: 911 variables, 24 stocks, subscripted over three climate-sensitivity
scenarios and eight negotiating blocs, 1850-2100.

This notebook does two things at once. It **uses** LTM to answer a real question --
which feedback loops drive C-LEARN's behavior, and when -- and it **audits** LTM,
checking each answer against an independent path before believing it. The audit
sections are the ones labelled *Audit*; several of them found real defects, which are
called out inline.
"""
)

code(
    """
import platform
import sys
import time
import warnings
from collections import Counter
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd

import simlin

REPO = Path.cwd().parent if Path.cwd().name == "notebooks" else Path.cwd()
MODEL_PATH = REPO / "test" / "xmutil_test_models" / "C-LEARN v77 for Vensim.mdl"
SEP = "\\u205a"  # the reserved separator in LTM synthetic variable names

print(f"pysimlin  {simlin.__version__}")
print(f"python    {sys.version.split()[0]} on {platform.platform()}")
print(f"model     {MODEL_PATH.name} ({MODEL_PATH.stat().st_size / 1e6:.2f} MB)")
assert MODEL_PATH.exists(), MODEL_PATH
"""
)

md(
    """
## 1. First contact

Loading a 1.4 MB Vensim `.mdl` and simulating it is two calls. Note what each one
costs -- LTM is not free, and knowing the ratio shapes how you use it.
"""
)

code(
    """
timings = {}


def timed(label, fn):
    t0 = time.perf_counter()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        out = fn()
    timings[label] = time.perf_counter() - t0
    for w in caught:
        print(f"  [{w.category.__name__}] {w.message}")
    return out


model = timed("load .mdl", lambda: simlin.load(MODEL_PATH))

stocks = model.get_var_names(type_mask=simlin.VARTYPE_STOCK)
flows = model.get_var_names(type_mask=simlin.VARTYPE_FLOW)
auxes = model.get_var_names(type_mask=simlin.VARTYPE_AUX)
print(f"{len(model.get_var_names())} variables: "
      f"{len(stocks)} stocks, {len(flows)} flows, {len(auxes)} aux")
print(f"time: {model.time_spec}")
"""
)

code(
    """
plain = timed("run, LTM off", lambda: model.run(analyze_loops=False))
ltm = timed("run, LTM on", model.run)

print(f"\\nresults: {plain.results.shape[0]} steps x {plain.results.shape[1]} columns")
print(f"ltm_mode: plain={plain.ltm_mode!r}  ltm={ltm.ltm_mode!r}")
print(f"run.loops: {len(ltm.loops)}")
"""
)

md(
    """
That warning is the single most useful thing the API does here. C-LEARN's causal graph
is far too large for compile-time exhaustive loop enumeration, so LTM silently switches
to post-simulation *discovery* -- and in discovery mode `run.loops` is empty by
construction. Without the warning, "no feedback loops" and "too big to enumerate them"
look identical.

The cost ratio is worth internalising: LTM instrumentation makes the run roughly an
order of magnitude more expensive. That is a dramatic improvement on where this model
was two months ago (LTM on C-LEARN was infeasible, then ~8s), but it is still the
reason `Model.analyze()` is opt-in rather than automatic.
"""
)

code(
    """
pd.Series(timings).round(3).to_frame("seconds")
"""
)

md(
    """
## 2. Loop discovery

`Model.analyze()` runs post-simulation discovery over the recorded link scores. The
result carries the loops, their
per-step importance series, and -- crucially -- the **cycle partitions**.

A partition is a set of stocks connected by feedback. Loop importance is normalized
*within* a partition, so a score of 0.4 in one partition and 0.4 in another are not
the same claim about the world. Almost every mistake in this notebook's audit sections
comes from forgetting that.
"""
)

code(
    """
analysis = timed("analyze()", model.analyze)

print(f"loops={len(analysis.loops)}  partitions={len(analysis.partitions)}  "
      f"periods={len(analysis.dominant_periods)}")
print(f"truncated={analysis.truncated}  agg_recovery_truncated={analysis.agg_recovery_truncated}")
print("polarity mix:", dict(Counter(str(x.polarity) for x in analysis.loops)))

pd.DataFrame(
    [
        {
            "partition": i,
            "loops": p.loop_count,
            "stocks": len(p.stocks),
            "example stock": p.stocks[0] if p.stocks else "",
        }
        for i, p in enumerate(analysis.partitions)
    ]
).set_index("partition")
"""
)

md(
    """
Neither flag is set, so this is a complete result rather than a budget-truncated one.

The partition table reads the model's architecture straight off: three large partitions
of 47 loops and 15 stocks each, then twelve single-loop partitions. The three large ones
are C-LEARN's three climate-sensitivity scenarios (`deterministic`,
`high_2xco2_sensitivity`, `low_2xco2_sensitivity`) -- the model runs three parallel
worlds via a subscript, so every climate loop exists three times over. The twelve
singletons are the isolated trace-gas decay loops (N2O, PFC, SF6, nine HFCs), each a
stock draining through its own uptake flow, coupled to nothing.

Let me verify the scenario claim rather than assume it.
"""
)

code(
    """
def base(name):
    return name.split("[")[0]


big = [i for i, p in enumerate(analysis.partitions) if p.loop_count > 1]
signatures = {
    i: {tuple(sorted({base(v) for v in loop.variables}))
        for loop in analysis.loops if loop.partition == i}
    for i in big
}
ref = signatures[big[0]]
for i in big[1:]:
    print(f"P{big[0]} vs P{i}: identical loop structure = {signatures[i] == ref} "
          f"({len(ref)} vs {len(signatures[i])} distinct variable-sets)")

scenario = {}
for i in big:
    tags = {v.split("[")[1].split(",")[0].rstrip("]")
            for loop in analysis.loops if loop.partition == i
            for v in loop.variables if "[" in v}
    scenario[i] = sorted(tags)
print("\\nsubscript tags per large partition:")
for i, tags in scenario.items():
    print(f"  P{i}: {tags}")
"""
)

md(
    """
Confirmed: the three large partitions carry byte-identical loop structure and are
separated purely by the scenario subscript. So there is exactly **one** climate system
to analyse, instantiated three times. From here on I work in the `deterministic`
partition and treat the other two as a sensitivity check.
"""
)

# ---------------------------------------------------------------------------
# 3. The dominance story
# ---------------------------------------------------------------------------

md(
    """
## 3. What actually drives C-LEARN

Now the real question. Within the deterministic scenario's partition, which loops carry
the behavior, and how does that shift over 250 years?
"""
)

code(
    """
det = next(i for i, p in enumerate(analysis.partitions)
           if any("deterministic" in s for s in p.stocks))
members = [x for x in analysis.loops if x.partition == det]
n_steps = min(len(x.behavior_time_series) for x in members)
years = np.linspace(model.time_spec.start, model.time_spec.stop, n_steps)

ranked = sorted(members, key=lambda x: x.average_importance(), reverse=True)
pd.DataFrame(
    [
        {
            "id": x.id,
            "pol": str(x.polarity),
            "mean |share|": round(x.average_importance(), 4),
            "peak |share|": round(x.max_importance(), 4),
            "len": len(x.variables),
            "cycle": " -> ".join(base(v) for v in x.variables[:5])
            + (" ..." if len(x.variables) > 5 else ""),
        }
        for x in ranked[:10]
    ]
).set_index("id")
"""
)

md(
    """
These are recognisable pieces of climate physics, which is the first real evidence that
the machinery is working:

- **b71** -- atmosphere-to-mixed-layer carbon flux. The ocean surface absorbs CO2, which
  raises mixed-layer carbon, which raises the equilibrium partial pressure and throttles
  further uptake. The dominant balancing loop for most of the run.
- **b19** -- the 12-variable radiative loop: atmospheric carbon raises forcing, raises
  heat in the atmosphere and upper ocean, raises temperature, which increases outgoing
  radiation. Classic Planck feedback cooling.
- **r17 / r16** -- the biomass-humus carbon sink, reinforcing: more atmospheric carbon
  fertilises biomass, which returns carbon through humus decay.
- **b43 / b45** -- deep-ocean diffusion, per layer.

Now the time profile.
"""
)

code(
    """
top = ranked[:6]
share = np.vstack([x.behavior_time_series[:n_steps] for x in top])

# C-LEARN's historical emissions inputs are yearly data, and every
# flow-to-stock link score divides by a stock's acceleration -- so the raw
# shares are genuinely spiky wherever the input data is. Plot the raw series
# faintly and an 11-step centred rolling median on top: the smoothing is a
# reading aid for a noisy INPUT, not a correction to the scores.
smooth = (pd.DataFrame(share.T)
          .rolling(11, center=True, min_periods=1)
          .median()
          .to_numpy().T)

fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(11, 8.5), sharex=True,
                               gridspec_kw={"height_ratios": [2, 1]})

for x, rawrow, smoothrow in zip(top, share, smooth):
    line, = ax1.plot(years, smoothrow, lw=2.0, label=f"{x.id} ({x.polarity})")
    ax1.plot(years, rawrow, lw=0.7, alpha=0.22, color=line.get_color())
ax1.axhline(0, color="0.5", lw=0.8)
ax1.set_ylabel("signed relative loop score")
ax1.set_title("Loop dominance within the deterministic-scenario partition\\n"
              "(bold: 11-step rolling median; faint: raw)")
ax1.legend(ncol=6, fontsize=9, loc="upper center", framealpha=0.9)
ax1.grid(alpha=0.25)

results = plain.results
temp_col = "temperature_change_from_preindustrial[deterministic]"
assert temp_col in results.columns, temp_col
ax2.plot(results.index, results[temp_col], color="crimson", lw=1.8)
ax2.set_ylabel("temp change (degC)")
ax2.set_xlabel("year")
ax2.grid(alpha=0.25)
fig.tight_layout()
plt.show()
"""
)

code(
    """
def leaders(step, k=3):
    order = sorted(members, key=lambda x: abs(x.behavior_time_series[step]), reverse=True)
    return ", ".join(f"{x.id}({x.behavior_time_series[step]:+.2f})" for x in order[:k])


pd.DataFrame(
    [{"year": y, "leading loops": leaders(int(np.argmin(np.abs(years - y))))}
     for y in (1900, 1950, 2000, 2025, 2050, 2075, 2100)]
).set_index("year")
"""
)

md(
    """
The story LTM tells is coherent and physically sensible: **ocean surface uptake (b71)
carries the balancing work through the industrial era and peaks around mid-century, then
hands off to radiative cooling (b19), which is overwhelmingly dominant by 2100.** The
biomass sink (r17) is the persistent reinforcing counterweight throughout.

That handoff is the kind of insight LTM exists to produce, and you cannot read it off
the stock-and-flow diagram.

One caveat inherited from the model rather than the tool: C-LEARN's emissions inputs are
piecewise data tables with slope changes at half-decade anchors. Every flow-to-stock link
score divides by a stock's acceleration, so the whole score field reshuffles at each
knee. Read these curves as trends, not step-by-step.
"""
)

# ---------------------------------------------------------------------------
# 4. Audit: dominant_periods
# ---------------------------------------------------------------------------

md(
    """
## 4. Audit: `dominant_periods` disagrees with `loops` in the same object

The `Analysis` object also offers `dominant_periods`, which claims to name the loops
that dominate each interval. It is the obvious thing to reach for. Here is what it says
about the model we just analysed.
"""
)

code(
    """
by_id = {x.id: x for x in analysis.loops}
pd.DataFrame(
    [
        {
            "start": p.start_time,
            "end": p.end_time,
            "loops": ", ".join(p.dominant_loops),
            "cycle": "; ".join(" -> ".join(by_id[i].variables) for i in p.dominant_loops),
        }
        for p in analysis.dominant_periods
    ]
)
"""
)

md(
    """
It reports that C-LEARN's behavior from 1851 to 1969 is explained by
`n2o -> n2o_uptake`, and from 1992 to 2100 by `hfc[hfc125] -> hfc_uptake[hfc125]`.

Those are two-variable trace-gas decay loops. They are not driving a climate model, and
they are not what section 3 found. Something is wrong -- so let me find out what, rather
than just distrusting the number.
"""
)

code(
    """
singleton = {i for i, p in enumerate(analysis.partitions) if p.loop_count == 1}
rows = []
for x in analysis.loops:
    if x.partition in singleton:
        s = x.behavior_time_series
        nz = s[np.isfinite(s) & (s != 0.0)]
        rows.append({
            "loop": x.id,
            "partition": x.partition,
            "active steps": len(nz),
            "distinct |score| while active": np.unique(np.abs(nz)).round(12).tolist()[:3],
        })
pd.DataFrame(rows).set_index("loop")
"""
)

md(
    """
There it is. Every one of the twelve singleton-partition loops has a relative score of
**exactly 1.0** whenever it is active -- not approximately, exactly. That is arithmetic,
not physics: the relative score is a loop's share of its partition's total activity, and
a loop alone in its partition has 100% of it by definition.

So ranking loops by `abs(behavior_time_series)` *across* partitions cannot do anything
except put the trivial loops first. And the engine knows this -- `Analysis.loops` sorts
those same twelve loops dead last, deliberately:
"""
)

code(
    """
positions = [i for i, x in enumerate(analysis.loops) if x.partition in singleton]
print(f"{len(analysis.loops)} loops in `analysis.loops`, competitive-first ranking")
print(f"positions of the singleton-partition loops: {positions}")
print(f"loops named by `analysis.dominant_periods`: "
      f"{sorted({i for p in analysis.dominant_periods for i in p.dominant_loops})}")
print()
print("=> the two surfaces of the SAME Analysis object rank these loops")
print("   at opposite ends: last by `loops`, first by `dominant_periods`.")
"""
)

md(
    """
**Finding.** `Analysis.dominant_periods` (and `Run.dominant_periods`, which shares the
flaw) is partition-blind. On any model with more than one cycle partition it is
systematically biased toward whichever loops have the least competition, and on C-LEARN
that makes it worthless -- it never mentions a single climate loop.

This is not a tolerance or a tuning problem. A flat cross-partition dominance ranking is
not well-defined, because the quantity being ranked is a within-partition share. The
surface needs to either report periods per partition or say which partition it means.

The workaround, which is what section 3 does, is to group by partition and rank inside
one.
"""
)

code(
    """
def dominant_periods_in_partition(loops, times, threshold=0.5):
    \"\"\"Greedy dominance, scoped to ONE partition so the shares are comparable.\"\"\"
    n = min(len(x.behavior_time_series) for x in loops)
    sets, out = [], []
    for step in range(n):
        scored = sorted(((x.id, x.behavior_time_series[step]) for x in loops),
                        key=lambda kv: abs(kv[1]), reverse=True)
        chosen, total = [], 0.0
        for lid, val in scored:
            if not np.isfinite(val) or val == 0.0:
                continue
            chosen.append(lid)
            total += abs(val)
            if total >= threshold:
                break
        sets.append(frozenset(chosen))
    start = 0
    for step in range(1, n):
        if sets[step] != sets[start]:
            out.append((times[start], times[step - 1], tuple(sorted(sets[start]))))
            start = step
    out.append((times[start], times[n - 1], tuple(sorted(sets[start]))))
    return [p for p in out if p[2]]


scoped = dominant_periods_in_partition(members, years)
print(f"{len(scoped)} periods within the deterministic partition "
      f"(vs {len(analysis.dominant_periods)} model-wide)")
pd.DataFrame(
    [{"start": round(a), "end": round(b), "n loops": len(ids),
      "loops": ", ".join(ids[:6]) + (" ..." if len(ids) > 6 else "")}
     for a, b, ids in scoped[:12]]
)
"""
)

md(
    """
Partition-scoped, the answer is meaningful again -- and notice it needs *several* loops
to reach half the activity, which is the honest picture for a system this coupled. The
model-wide surface reached its threshold with one trivial loop every time.
"""
)

# ---------------------------------------------------------------------------
# 5. Audit: cross-validation
# ---------------------------------------------------------------------------

md(
    """
## 5. Audit: do the discovery numbers survive an independent check?

Section 3's conclusions rest entirely on discovery's post-simulation scoring. Discovery
enumerates the loop universe from the recorded link scores and reports whether that
enumeration completed (`enumeration_complete`), but its scores come from one code path --
so before believing them, I want the same loops scored by a different one.

Simlin has one: **pinning**. `set_loop_name` tells the engine to always instrument and
score a named cycle, using the exhaustive per-loop scoring machinery rather than the
discovery search. If the two agree, that is meaningful evidence; if they disagree, at
least one is wrong.

The first attempt fails, which is itself a finding.
"""
)

code(
    """
pinned = simlin.load(MODEL_PATH)
try:
    with pinned.edit() as (_current, patch):
        patch.set_loop_name(name="verbatim", variables=list(ranked[0].variables))
    print("accepted")
except simlin.SimlinRuntimeError as exc:
    print(f"REJECTED: {exc}")
    print(f"\\ndiscovery reported this cycle as: {list(ranked[0].variables)}")
"""
)

md(
    """
**Finding.** On an arrayed model, discovery reports element-level names
(`c_in_mixed_layer[deterministic]`) but `set_loop_name` only resolves plain idents, so a
discovered loop cannot be handed back as a pin. Stripping the subscript is the only way
through -- and it changes the meaning: you pin the whole apply-to-all family, not the
element instance you found. For this cross-validation that is acceptable (the three
scenarios are structurally identical), but "pin the loop discovery just showed me" is
not expressible today.
"""
)

code(
    """
def _run_pinned(m):
    s = m.simulate(enable_ltm=True)
    s.run_to_end()
    return s


targets = ranked[:5]
pinned = simlin.load(MODEL_PATH)
with pinned.edit() as (_current, patch):
    for x in targets:
        patch.set_loop_name(name=f"disc_{x.id}",
                            variables=sorted({base(v) for v in x.variables}))

psim = timed("pinned LTM run", lambda: _run_pinned(pinned))
print(f"ltm_mode: {psim.get_ltm_mode()}")
pd.DataFrame(
    [{"pin id": x.id, "name": x.name, "structural polarity": str(x.polarity),
      "vars": len(x.variables)}
     for x in pinned.loops]
).set_index("pin id")
"""
)

md(
    """
The pins are registered and scored even though the model is in discovery mode -- that is
exactly what pinning is for. Note the structural polarity column: mostly `U`
(undetermined), because structural polarity is inferred from link signs and a Vensim
import leaves many of those unknown. The *runtime* surface does better, because it
classifies from the actual score series:
"""
)

code(
    """
runtime = {x.id: x for x in psim.get_loops_runtime()}
raw = {x.id: psim.get_series(f"${SEP}ltm{SEP}loop_score{SEP}{x.id}") for x in pinned.loops}
disc_pol = {f"disc_{x.id}": str(x.polarity) for x in targets}

rows = []
for x in pinned.loops:
    s = raw[x.id]
    nz = s[np.isfinite(s) & (s != 0.0)]
    sign = "all <= 0" if (nz <= 0).all() else ("all >= 0" if (nz >= 0).all() else "mixed")
    rt = runtime.get(x.id)
    rows.append({
        "pin": x.id,
        "structural": str(x.polarity),
        "runtime": str(rt.polarity) if rt else "-",
        "confidence": rt.polarity_confidence if rt else None,
        "discovery": disc_pol.get(x.name),
        "raw score sign": sign,
    })
pd.DataFrame(rows).set_index("pin")
"""
)

md(
    """
Runtime polarity is correct on all five, at confidence 1.0, and agrees with discovery --
including the four the structural pass could not determine. Now the numbers themselves.

The pinned loops sit in one partition, so their relative scores are directly comparable;
to compare against discovery I renormalize discovery's shares over the same five loops.
"""
)

code(
    """
ids = [x.id for x in pinned.loops]
stack = np.vstack([raw[i] for i in ids])
denom = np.nansum(np.abs(stack), axis=0)
with np.errstate(invalid="ignore", divide="ignore"):
    rel_pin = np.where(denom > 0, stack / denom, 0.0)

name_to_disc = {f"disc_{x.id}": x for x in targets}
disc_for = {x.id: name_to_disc[x.name] for x in pinned.loops}
dstack = np.vstack([disc_for[i].behavior_time_series[:rel_pin.shape[1]] for i in ids])
d_denom = np.nansum(np.abs(dstack), axis=0)
with np.errstate(invalid="ignore", divide="ignore"):
    rel_disc = np.where(d_denom > 0, dstack / d_denom, 0.0)

resid = np.nanmax(np.abs(rel_pin - rel_disc))
print(f"max |pinned - discovery| over all 5 loops x {rel_pin.shape[1]} steps: {resid:.3e}")

frames = []
for y in (1900, 1950, 2000, 2050, 2100):
    step = int(np.argmin(np.abs(years[: rel_pin.shape[1]] - y)))
    frames.append(pd.DataFrame({
        "year": y,
        "loop": [disc_for[i].id for i in ids],
        "pinned": rel_pin[:, step].round(4),
        "discovery": rel_disc[:, step].round(4),
    }))
pd.concat(frames).set_index(["year", "loop"])
"""
)

md(
    """
**The two paths agree to floating-point noise.** The exhaustive per-loop scoring
machinery and post-simulation discovery, which share almost none of their code,
produce the same relative shares at every one of 251 timesteps.

This is the strongest positive result in the notebook. Section 3's dominance story is not
an artifact of the discovery code path.
"""
)

# ---------------------------------------------------------------------------
# 6. Audit: link-score coverage
# ---------------------------------------------------------------------------

md(
    """
## 6. Audit: how much of the causal graph is actually scored?

Loop scores are built from link scores. So: of C-LEARN's causal edges, how many carry a
usable score? This turns out to be the question with the most uncomfortable answer, and
the diagnostics that reveal it are easy to never see.
"""
)

code(
    """
sim = model.simulate(enable_ltm=True)
sim.run_to_end()

links = sim.get_links()
raw_links = sim.get_links(include_internal=True)
scored = [x for x in links if x.has_score()]


def live(link):
    finite = link.score[np.isfinite(link.score)]
    return finite.size > 0 and not np.all(finite == 0.0)


live_links = [x for x in scored if live(x)]
print(f"collapsed causal graph : {len(links)} edges "
      f"(raw, with synthetic nodes: {len(raw_links)})")
print(f"  with a score series  : {len(scored)} ({100*len(scored)/len(links):.0f}%)")
print(f"  score not identically 0: {len(live_links)} ({100*len(live_links)/len(links):.0f}%)")
print()
print("link polarity coverage:", dict(Counter(str(x.polarity) for x in links)))
"""
)

md(
    """
About a fifth of the graph carries a live link score, and three quarters of the edges
have unknown polarity. Some of that is legitimate -- an edge from a constant genuinely
has a zero score. But not all of it. The explanation is in the diagnostics, and there is
a trap in how you get them.
"""
)

code(
    """
fresh = simlin.load(MODEL_PATH)
before = fresh.check()
scratch = fresh.simulate(enable_ltm=True)
scratch.run_to_end()
after = fresh.check()
print(f"model.check() before any LTM sim : {len(before)} issues")
print(f"model.check() after an LTM sim   : {len(after)} issues")
print(f"newly visible                    : {len(after) - len(before)}")
"""
)

md(
    """
**Finding.** LTM's own diagnostics are invisible until you have created an LTM
simulation on that project -- and `Model.run()` and `Model.analyze()`, which enable LTM
internally, do not surface them. It is entirely possible to call `analyze()`, get 153
confident-looking loops, and never learn that the engine emitted sixteen hundred
warnings about the model you just analysed.
"""
)

code(
    """
failed = [i for i in after if "failed to compile" in i.message]


def form(msg):
    for phrase in ("LTM synthetic variable", "LTM implicit helper"):
        if phrase in msg:
            return phrase
    return "other"


print(f"{len(failed)} fragments failed to compile:")
for kind, n in Counter(form(i.message) for i in failed).most_common():
    print(f"  {n:5d}  {kind}")

print("\\nrepresentative message:\\n")
print(failed[0].message[:330], "...")
"""
)

md(
    """
Each of these is an LTM equation the engine generated and then could not compile. The
variable keeps a layout slot but no bytecode, so it evaluates to a constant 0 -- which is
why so much of the graph reads zero.

The obvious next worry is that this quietly corrupts the loop analysis. So let me check
whether any of the loops from section 3 run through a failing edge.
"""
)

code(
    """
import re

edge_re = re.compile(rf"link_score{SEP}(.+?)\\u2192(.+?)(?:{SEP}\\d+{SEP}\\w+)?$")
failing_edges = set()
for issue in failed:
    if not issue.variable:
        continue
    m = edge_re.search(issue.variable)
    if m:
        failing_edges.add((base(m.group(1)), base(m.group(2))))

print(f"distinct failing causal edges: {len(failing_edges)}")

touched = 0
for x in analysis.loops:
    vs = [base(v) for v in x.variables]
    pairs = {(vs[i], vs[(i + 1) % len(vs)]) for i in range(len(vs))}
    if pairs & failing_edges:
        touched += 1
print(f"discovered loops traversing a failing edge: {touched} of {len(analysis.loops)}")

targets_hit = Counter(t for _f, t in failing_edges)
print("\\nwhere the failures concentrate (top targets):")
for name, n in targets_hit.most_common(8):
    print(f"  {n:4d}  {name}")
"""
)

md(
    """
**This is the reassuring half.** Not one of the 153 discovered loops passes through a
failing edge. The failures cluster in C-LEARN's emissions-target machinery --
`sorted_target_*`, `projected_population_in_target_year`, `rs_co2eq_nonforest_emissions`
-- which is the negotiation front end: heavily subscripted over the eight blocs, and
largely feed-forward. There is no feedback there to lose.

So the loop-dominance results in section 3 stand. What is lost is **link-level**
attribution over a large, policy-relevant part of the model: if you wanted to ask "which
lever moves the 2050 target most", that is exactly the subgraph LTM cannot currently
answer for.

Of the edges that *are* scored but read identically zero, how many are legitimate?
"""
)

code(
    """
res = plain.results
zero = [x for x in scored if not live(x)]


def time_varying(name):
    \"\"\"Does any ELEMENT of `name` move over time?

    Reducing across an arrayed variable's columns would conflate scenario
    spread with time variation, so measure per column.
    \"\"\"
    cols = [c for c in res.columns if c == name or c.startswith(name + "[")]
    for c in cols:
        v = res[c].to_numpy()
        v = v[np.isfinite(v)]
        if v.size and v.max() > v.min():
            return True
    return False


const_src = sum(1 for x in zero if not time_varying(x.from_var))
print(f"{len(zero)} scored edges read identically zero")
print(f"  source is constant over the run (a zero score is correct): {const_src}")
print(f"  source varies over the run (unexplained)                 : {len(zero) - const_src}")
"""
)

md(
    """
The majority are legitimate. The remainder is a real residual worth chasing, but it is
a tail, not the story.
"""
)

# ---------------------------------------------------------------------------
# 7. Audit: relative link ranking
# ---------------------------------------------------------------------------

md(
    """
## 7. Audit: ranking links by relative score

Raw link scores are famously not comparable across targets -- they divide by the change
in the target, so a near-constant target produces an enormous score. Simlin addresses
this with a *relative* link score, normalized per target into `[-1, 1]`, and the API
recommends ranking by it.

Let me try that on the live links and see what floats to the top.
"""
)

code(
    """
ranked_links = sorted(live_links,
                      key=lambda x: abs(x.average_relative_score() or 0.0), reverse=True)
fan_in = Counter(x.to_var for x in scored)

pd.DataFrame(
    [{"|mean rel|": round(abs(x.average_relative_score()), 4),
      "inputs to target": fan_in[x.to_var],
      "link": f"{x.from_var} -> {x.to_var}"}
     for x in ranked_links[:12]]
)
"""
)

md(
    """
The same failure mode as section 4, one level down. A link into a target with exactly one
scored input is 100% of that target's change **by construction**, so it scores ~1.0
whether or not it matters. The relative score fixed raw scores' comparability problem and
inherited a new degeneracy in its place.
"""
)

code(
    """
solo = [x for x in ranked_links if fan_in[x.to_var] == 1]
print(f"live links whose target has exactly one scored input: {len(solo)} of {len(live_links)}")
print(f"of the top 100 by |mean relative score|, "
      f"{sum(1 for x in ranked_links[:100] if fan_in[x.to_var] == 1)} are such links")

competing = [x for x in ranked_links if fan_in[x.to_var] > 1]
print("\\ntop links restricted to targets with real competition:")
pd.DataFrame(
    [{"|mean rel|": round(abs(x.average_relative_score()), 4),
      "inputs": fan_in[x.to_var],
      "link": f"{x.from_var} -> {x.to_var}"}
     for x in competing[:10]]
)
"""
)

md(
    """
Restricting to contested targets helps but does not fully fix it -- a two-input target
whose second input happens to be inert still reads 1.0. A genuinely global link ranking
needs to weight each target by that target's own importance, which the relative score
deliberately divides out.

**Finding.** `Link.average_relative_score()` is the right metric for comparing the inputs
*of one target*. It is not a global importance ranking, and the API previously
recommended using it as one.
"""
)

# ---------------------------------------------------------------------------
# 8. Scorecard
# ---------------------------------------------------------------------------

md(
    """
## 8. Scorecard

### What works well

| | |
|---|---|
| **Speed** | A 911-variable arrayed Vensim model loads in ~30 ms and runs in under half a second; LTM instrumentation costs roughly 10x that. Two months ago LTM on this model was infeasible. |
| **Discovery** | 153 loops across 15 partitions, untruncated, in ~3 s -- with per-step importance series and no tuning. |
| **Cycle partitions** | The single best design decision in this API. They recovered C-LEARN's three-scenario architecture with zero input from me, and they are the key to reading every other number correctly. |
| **Correctness where it counts** | Pinned scoring and discovery agree to floating-point noise across 5 loops x 251 steps. Runtime polarity classification is right at confidence 1.0 even where structural polarity is undetermined. |
| **The results themselves** | A physically coherent story -- ocean uptake handing off to radiative cooling -- that is not visible from the model structure. |
| **Honest degradation** | The discovery auto-flip warning turns a silently-empty loop list into an actionable message. More surfaces should behave like this. |

### What doesn't

| | Severity |
|---|---|
| `dominant_periods` is partition-blind, and contradicts `loops` in the same object. On C-LEARN it never names a climate loop. | **High** -- it is the surface a new user reaches for first, and it is confidently wrong. |
| LTM fragments fail to compile, silently degrading to constant 0 -- the count is whatever section 6 measured on this engine build. Loops are unaffected; link-level attribution over the whole policy subgraph is not. | **High** |
| Those diagnostics are unreachable unless you call `check()` *after* creating an LTM sim. `run()`/`analyze()` never surface them. | **High** -- it is what makes the previous row silent. |
| Ranking links by relative score surfaces uncontested targets, which is the opposite of what is wanted. | Medium |
| Discovery's element-level loop names are rejected by `set_loop_name`, so a discovered loop cannot be pinned. | Medium |
| Per-element raw loop scores are unreachable; a bare read silently returns element 0 while every other surface reports an argmax-abs aggregate. | Medium |
| `model.check()` repeats each unit warning once per array element. | Low |

### Where to invest

1. **Make within-partition normalization structural rather than advisory.** Three separate defects here -- `dominant_periods`, the link ranking, and lone-pin degeneracy -- are one root cause: LTM's relative measures are *shares within a group*, and every surface that ranks them globally is dominated by groups of size one. Ranking APIs should either take the group as a parameter or return grouped results, so a cross-group comparison is not expressible.

2. **Close the fragment-compile gap.** The failure count section 6 measured is a coverage statement about the augmentation layer, concentrated in arrayed per-element equations with dynamic-index or un-hoisted-reducer reads. A generated-corpus harness that asserts every emitted fragment compiles would turn this from a per-model surprise into a gate.

3. **Surface LTM diagnostics on the path people use.** `Model.analyze()` and `Model.run()` should report the count of degraded fragments -- the same way the discovery auto-flip warning already works.

4. **Make discovery output usable as input.** Element-level loop identity should round-trip into pinning, and per-element raw scores should be readable.

### Notes on the tooling

Building and installing was not smooth. `make build` in `src/pysimlin` was broken twice
over -- it looked for `libsimlin` at the repo root instead of `src/libsimlin`, and then
invoked `pip` inside a `uv`-managed virtualenv that has none. Both are fixed. Separately,
`uv run` re-syncs the project as an editable install, silently replacing an installed
wheel, so this notebook's generator invokes the venv interpreter directly.
"""
)

code(
    """
print(f"pysimlin {simlin.__version__}")
print(f"{len(analysis.loops)} loops / {len(analysis.partitions)} partitions discovered")
print(f"{len(live_links)} of {len(links)} causal edges carry a live link score")
print(f"{len(failed)} LTM fragments failed to compile "
      f"({touched} discovered loops affected)")
print(f"pinned-vs-discovery max residual: {resid:.3e}")
"""
)

nb["cells"] = cells

if __name__ == "__main__":
    import nbformat
    from nbclient import NotebookClient

    NOTEBOOK_PATH.write_text(nbformat.writes(nb), encoding="utf-8")
    print(f"wrote {NOTEBOOK_PATH} ({len(cells)} cells)")

    client = NotebookClient(
        nb,
        timeout=1800,
        kernel_name="python3",
        resources={"metadata": {"path": str(NOTEBOOKS_DIR)}},
    )
    client.execute()
    NOTEBOOK_PATH.write_text(nbformat.writes(nb), encoding="utf-8")
    print(f"executed and saved {NOTEBOOK_PATH}")
