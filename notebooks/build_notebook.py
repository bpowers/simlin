#!/usr/bin/env python3
"""Build the C-LEARN LTM experience notebook.

Constructs notebooks/clearn_ltm_experience.ipynb cell-by-cell via nbformat,
then executes it (populating outputs) with nbclient. The executed .ipynb and
its HTML render are generated artifacts (gitignored); this script is the
source of truth, so the notebook can be regenerated against any engine build.

Run from the pysimlin venv, synced with the `notebooks` extra
(`cd src/pysimlin && uv sync --extra dev --extra notebooks`):

    src/pysimlin/.venv/bin/python notebooks/build_notebook.py
    src/pysimlin/.venv/bin/jupyter nbconvert --to html notebooks/clearn_ltm_experience.ipynb
"""

from pathlib import Path

import nbformat as nbf

NOTEBOOKS_DIR = Path(__file__).resolve().parent

nb = nbf.v4.new_notebook()
nb.metadata["kernelspec"] = {
    "display_name": "Python 3",
    "language": "python",
    "name": "python3",
}

cells = []


def md(source: str) -> None:
    cells.append(nbf.v4.new_markdown_cell(source.strip()))


def code(source: str) -> None:
    cells.append(nbf.v4.new_code_cell(source.strip()))


# ============================================================================
# Title and introduction
# ============================================================================

md("""
# Loops That Matter on C-LEARN

**Feedback-loop dominance analysis of Climate Interactive's C-LEARN climate model, through `pysimlin`.**

[C-LEARN](https://www.climateinteractive.org/) (v77, 2010) is the simulation core that became
C-ROADS: a globally-aggregated climate-policy model built in Vensim. It couples a carbon cycle, an
energy-balance temperature model, other greenhouse gases, permafrost feedbacks, and sea-level rise
to regional emissions-policy levers, and it runs from 1850 to 2100. Importantly for us, it is a
*real* practitioner model: 911 variables, two dozen stocks, arrayed equations, Vensim macros,
lookup tables -- all the things that make production models hard for analysis tooling.

**Loops That Matter** (LTM; Schoenberg, Davidsen & Eberlein 2020; Eberlein & Schoenberg 2020;
Schoenberg, Hayward & Eberlein 2023) is a method for *loop dominance analysis*: it measures, at
every instant of a simulation, how much each feedback loop contributes to the model's behavior.
Where a modeler's intuition says "warming is accelerating because the carbon sinks are weakening",
LTM aims to make that statement quantitative and checkable.

This notebook follows the path a practitioner would actually take:

1. **Load and explore** the model: structure, diagram, baseline behavior.
2. **Discover** the feedback loops that drive that behavior (`model.analyze()`).
3. **Pin and track** the nine textbook climate feedback loops by name, and watch dominance shift
   from the planet's balancing machinery to the reinforcing carbon-climate feedbacks over the
   21st century.
4. **Interrogate the jagged parts** of that story -- the dominance chart has abrupt transitions,
   and we run them to ground: are they sampling artifacts, model structure, or an LTM bug?
5. **Test dominance against leverage** with a counterfactual run -- and find the two are very
   different things, exactly as the LTM papers warn.
6. Close with an **experience report**: what this exercise revealed about the API and the
   implementation (several pieces of which were fixed in the engine as this notebook was written).
""")

# ============================================================================
# 1. Loading C-LEARN
# ============================================================================

md("""
## 1. Loading C-LEARN

`simlin.load` reads the original 53,000-line Vensim `.mdl` directly -- no conversion step.
""")

code("""
import warnings
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd

import simlin

plt.rcParams["figure.figsize"] = (9.5, 4.5)
plt.rcParams["axes.grid"] = True
plt.rcParams["grid.alpha"] = 0.3

REPO = Path.cwd().parent if Path.cwd().name == "notebooks" else Path.cwd()
MDL = REPO / "test" / "xmutil_test_models" / "C-LEARN v77 for Vensim.mdl"

model = simlin.load(MDL)
model
""")

code("""
from simlin import VARTYPE_AUX, VARTYPE_FLOW, VARTYPE_STOCK

stocks = model.get_var_names(VARTYPE_STOCK)
flows = model.get_var_names(VARTYPE_FLOW)
auxes = model.get_var_names(VARTYPE_AUX)

print(f"variables: {len(model.get_var_names())} total "
      f"({len(stocks)} stocks, {len(flows)} flows, {len(auxes)} auxiliaries)")
print(f"time: {model.time_spec.start:.0f} to {model.time_spec.stop:.0f} "
      f"(dt={model.time_spec.dt}, save every 1 {model.time_spec.units})")
print("\\nstocks:")
for s in stocks:
    print(f"  {s}")
""")

md("""
The stocks tell you what kind of model this is. The physical climate core is the handful that
matter for this analysis:

* **carbon cycle**: `c_in_atmosphere`, `c_in_biomass`, `c_in_humus`, `c_in_mixed_ocean` plus a
  4-layer deep ocean -- carbon moving between air, plants, soil, and sea;
* **energy balance**: `heat_in_atmosphere_and_upper_ocean` and `heat_in_deep_ocean` -- the
  planet's heat content, which (divided by heat capacity) is the temperature anomaly;
* the rest are bookkeeping for the other greenhouse gases (CH4, N2O, the HFC/PFC/SF6 families),
  sea-level rise, and cumulative-emissions accounting.

Equations read back as authored. Three from the core:
""")

code("""
for name in ["atm_conc_co2", "temperature_change_from_preindustrial", "flux_atm_to_ocean"]:
    print(model.explain(name))
    print()
""")

# ============================================================================
# 2. The model diagram
# ============================================================================

md("""
## 2. The model diagram

The Vensim sketch comes through the importer as one tall canvas of sector views.
`project.render_png` renders it, and the view metadata (group names with bounding boxes) lets us
crop out the two sectors this notebook is about -- the **carbon cycle** and the **climate**
(energy balance) -- rather than hardcoding pixel offsets.
""")

code("""
import io
import json
import re

from IPython.display import Image, display
from PIL import Image as PILImage

project_json = json.loads(model.project.serialize_json().decode("utf-8"))
view = project_json["models"][0]["views"][0]
groups = {e["name"]: e for e in view["elements"] if e.get("type") == "group"}

# The SVG render declares the exact model-coordinate viewBox (content bounds
# plus padding); use it to map sector-group coordinates onto canvas pixels.
svg_head = model.project.render_svg("main")[:600].decode("utf-8")
vb_left, vb_top, vb_w, vb_h = map(
    float, re.search(r'viewBox="(-?[\\d.]+) (-?[\\d.]+) ([\\d.]+) ([\\d.]+)"', svg_head).groups()
)

RENDER_W = 2000
png = model.project.render_png("main", width=RENDER_W)
PILImage.MAX_IMAGE_PIXELS = None  # one tall trusted canvas, not a decompression bomb
canvas = PILImage.open(io.BytesIO(png))
scale = canvas.width / vb_w
print(f"rendered canvas: {canvas.width} x {canvas.height} px, {len(groups)} sector views; "
      "showing two:")


def show_sector(name: str, pad: float = 25.0) -> None:
    g = groups[name]
    box = (
        int((g["x"] - pad - vb_left) * scale),
        int((g["y"] - pad - vb_top) * scale),
        int((g["x"] + g["width"] + pad - vb_left) * scale),
        int((g["y"] + g["height"] + pad - vb_top) * scale),
    )
    crop = canvas.crop(box)
    buf = io.BytesIO()
    crop.save(buf, format="PNG")
    print(f"\\n{name}:")
    display(Image(data=buf.getvalue(), width=940))


show_sector("Carbon Cycle")
show_sector("Climate")
""")

md("""
Both sectors are textbook stock-and-flow chains. In the carbon cycle, atmospheric carbon exchanges
with biomass, humus (soil), and a stack of ocean layers; in the climate sector, net radiative
forcing accumulates as heat, whose ratio to heat capacity is the temperature anomaly that feeds
back everywhere else. Every feedback loop we analyze below is visible in these two diagrams.
""")

# ============================================================================
# 3. The baseline run
# ============================================================================

md("""
## 3. The baseline run

`model.run()` simulates and -- by default -- asks for loop analysis. On a model this size, that
request does something worth paying attention to:
""")

code("""
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    run = model.run()

for w in caught:
    print(f"{w.category.__name__}:\\n  {w.message}\\n")

print(f"ltm_mode: {run.ltm_mode!r}")
print(f"loops reported: {len(run.loops)}")
print(f"results: {run.results.shape[0]} timesteps x {run.results.shape[1]} series")
""")

md("""
Two modes, chosen automatically. For small models the engine **exhaustively enumerates** every
feedback loop (Johnson's algorithm) and scores each one. C-LEARN's causal graph has a
strongly-connected component far past the enumeration gate (loop counts in graphs like this grow
toward factorial -- Urban Dynamics has 43 *million* loops), so the engine auto-switches to
**discovery mode**: no pre-enumerated loop list, and `run.loops` contains only loops you
explicitly ask for (next two sections). The `RuntimeWarning` says this happened and what to do
about it; `run.ltm_mode` makes it checkable in code.

What does business-as-usual project? The `scenario` dimension runs three climate sensitivities in
parallel; `deterministic` is the central case (3°C per CO2 doubling).
""")

code("""
results = run.results
years = results.index

fig, ax1 = plt.subplots()
ax1.plot(years, results["temperature_change_from_preindustrial[deterministic]"],
         color="#c62828", lw=2)
ax1.set_ylabel("temperature change from preindustrial (°C)", color="#c62828")
ax1.set_xlabel("year")
ax2 = ax1.twinx()
ax2.plot(years, results["atm_conc_co2[deterministic]"], color="#1976d2", lw=2)
ax2.set_ylabel("atmospheric CO2 (ppm)", color="#1976d2")
ax2.grid(False)
ax1.set_title("C-LEARN business as usual")
fig.tight_layout()

print(f"2100: {results['temperature_change_from_preindustrial[deterministic]'].iloc[-1]:.2f} °C, "
      f"{results['atm_conc_co2[deterministic]'].iloc[-1]:.0f} ppm CO2")
""")

# ============================================================================
# 4. Discovery: which loops matter?
# ============================================================================

md("""
## 4. Which loops matter? Ask the engine

In discovery mode the engine runs the LTM strongest-path search (Eberlein & Schoenberg 2020) over
the link scores at every saved timestep: a Dijkstra-like DFS from every stock, following the
highest-|score| links, collecting the feedback loops that are actually *doing something* at each
instant. `model.analyze()` is the explicit, timeout-guarded entry point.
""")

code("""
analysis = model.analyze(timeout=120.0)

print(f"discovered {len(analysis.loops)} loops "
      f"(truncated by timeout: {analysis.truncated})\\n")

print(f"{'rank':>4} {'id':>5} {'pol':>4} {'vars':>5} {'mean |rel score|':>17}   path")
for rank, loop in enumerate(analysis.loops[:18], 1):
    imp = loop.average_importance()
    path = " -> ".join(v.split("[")[0] for v in loop.variables[:3])
    if len(loop.variables) > 3:
        path += " -> ..."
    print(f"{rank:>4} {loop.id:>5} {str(loop.polarity):>4} {len(loop.variables):>5} "
          f"{imp:>17.3f}   {path}")
""")

md("""
A loop's **relative loop score** at an instant is its share of all loop activity in its *cycle
partition* (a group of stocks connected by feedback), signed by polarity: `+` reinforcing, `-`
balancing. The ranking above is by mean |relative score|.

Reading it takes one piece of context: **a loop alone in its partition always scores ±1** -- it
explains 100% of whatever its little subsystem does. That is why the top of the list is a parade
of two-variable gas-uptake loops (`hfc[...] -> hfc_uptake[...]`): each HFC stock decays in its own
isolated partition, trivially "dominated" by its only loop. Correct, just not interesting.

The interesting structure starts where partitions are crowded: the loops through
`c_in_atmosphere`, `c_in_biomass`, and `heat_in_atmosphere_and_upper_ocean` -- the coupled
carbon-climate core, where a dozen loops compete and a 30% share means something. Discovery found
that core without being told anything about climate; per scenario element, even (note the
`[deterministic]` / `[high_2xco2_sensitivity]` subscripts).
""")

# ============================================================================
# 5. Pinning the climate loops
# ============================================================================

md("""
## 5. Pinning the loops that matter

Discovery's loop list is parameterization-dependent (run a different scenario, find different
loops) and its machine ids (`b23`, `r16`) say nothing. The LTM papers' answer is the `LOOPSCORE`
builtin: let the modeler *name a loop up front* and guarantee it gets scored. Simlin implements
this as **loop pinning** -- `set_loop_name` takes the loop's member variables (order-free; the
engine recovers the cycle from the causal graph) and a human name.

Here are C-LEARN's nine textbook climate feedbacks. To be clear about provenance: this list is
**curated from domain knowledge, not copied from the discovery output** -- it is what a climate
modeler would write down after reading the carbon-cycle and climate sector diagrams in section 2
(each loop below traces a visible cycle there). Whether the structure-blind discovery search
agrees with the domain expert is checked, not assumed, right after we pin them.
""")

code("""
HEAT, NETFLOW = "heat_in_atmosphere_and_upper_ocean", "heat_in_atmosphere_and_upper_ocean_net_flow"
TEMP, CATM = "temperature_change_from_preindustrial", "c_in_atmosphere"

CLIMATE_LOOPS = {
    # the planet's balancing machinery
    "Feedback cooling": [HEAT, TEMP, "feedback_cooling", NETFLOW],
    "Deep ocean heat uptake": [HEAT, TEMP, "heat_transfer", NETFLOW],
    "Ocean carbon uptake": [CATM, "equil_c_in_mixed_layer", "flux_atm_to_ocean"],
    "CO2 fertilization": [CATM, "flux_atm_to_biomass"],
    # the land carbon revolving door
    "Biomass carbon recycling": [CATM, "flux_atm_to_biomass", "c_in_biomass",
                                 "flux_biomass_to_atmosphere"],
    "Soil carbon recycling": [CATM, "flux_atm_to_biomass", "c_in_biomass",
                              "flux_biomass_to_humus", "c_in_humus", "flux_humus_to_atmosphere"],
    # the reinforcing carbon-climate feedbacks
    "Warming weakens ocean uptake": [HEAT, TEMP, "effect_of_temp_on_dic_pco2",
                                     "equil_c_in_mixed_layer", "flux_atm_to_ocean", CATM,
                                     "co2_radiative_forcing", "total_radiative_forcing", NETFLOW],
    "Warming weakens land uptake": [HEAT, TEMP, "effect_of_warming_on_c_flux_to_biomass",
                                    "flux_atm_to_biomass", CATM, "co2_radiative_forcing",
                                    "total_radiative_forcing", NETFLOW],
    "Permafrost carbon release": [CATM, "co2_radiative_forcing", "total_radiative_forcing",
                                  NETFLOW, HEAT, TEMP, "flux_c_from_permafrost_release"],
}

with model.edit() as (current, patch):
    for name, variables in CLIMATE_LOOPS.items():
        patch.set_loop_name(name, variables)

for loop in model.loops:
    label = f' "{loop.name}"' if getattr(loop, "name", None) else ""
    print(f"  {loop.id}{label}: " + " -> ".join(loop.variables))
    print()
""")

md("""
The engine validated each variable set against the causal graph, recovered the cycle ordering, and
-- because every one of these variables is arrayed over the 3-element `scenario` dimension --
instantiated each pin **per element** (three score slots per loop, queryable by subscript).

### Does the expert's list agree with the machine's?

Cross-reference each pinned variable set against section 4's discovered loops (comparing
subscript-stripped variable sets):
""")

code("""
def base_vars(loop):
    return frozenset(v.split("[")[0] for v in loop.variables)


def elem_of(loop):
    for v in loop.variables:
        if "[" in v:
            return v.split("[")[1].rstrip("]")
    return "scalar"


for name, variables in CLIMATE_LOOPS.items():
    matches = [
        (rank, loop) for rank, loop in enumerate(analysis.loops, 1)
        if base_vars(loop) == frozenset(variables)
    ]
    if matches:
        found = ", ".join(f"#{rank} {loop.id} [{elem_of(loop)}]" for rank, loop in matches)
        print(f"  {name}:\\n      discovered as {found}")
    else:
        print(f"  {name}:\\n      NOT in the discovery results")
""")

md("""
Two findings, both worth dwelling on:

* **Every active loop on the expert's list was discovered exactly** -- same variable set, found
  *three separate times* (once per scenario element), ranked among 153 loops. The domain expert
  and the structure-blind strongest-path search converge on the same eight cycles, which is about
  as strong a cross-validation as an analysis method can offer. (The converse does not hold:
  discovery also surfaced loops the curated list ignores -- the per-gas uptake loops, deep-ocean
  carbon chains, sub-loops of the core -- which is what the heuristic is *for*.)
* **Permafrost carbon release is absent from discovery -- necessarily.** The strongest-path
  search drops zero-score links at every timestep, and (as the next cell shows) the permafrost
  flux never moves in this scenario, so no search at any timestep can traverse it. A dormant loop
  is structurally invisible to behavior-driven discovery; **pinning is the only way to put one
  under observation**. Discovery and pinning are complements, not alternatives -- precisely the
  papers' argument for shipping `LOOPSCORE` alongside the heuristic.

Now simulate with LTM instrumentation and pull each pinned loop's score series:
""")

code("""
sim = model.simulate(enable_ltm=True)
sim.run_to_end()
ltm_run = sim.get_run()
time_years = sim.get_series("time")

print(f"ltm mode: {sim.get_ltm_mode()}; "
      f"{sim.get_loop_element_count('pin1')} scenario slots per pinned loop\\n")

for loop in ltm_run.loops:
    rel = sim.get_relative_loop_score(loop.id, element="deterministic")
    finite = rel[np.isfinite(rel)]
    label = getattr(loop, "name", None) or loop.id
    print(f"  {loop.id} ({loop.polarity}) {label}: "
          f"mean |share| {np.abs(finite).mean():.1%}, peak {np.abs(finite).max():.1%}")
""")

md("""
Two things worth noticing before the headline chart:

* **Runtime polarity classification works through the data.** Statically, most of these loops
  pass through lookup tables whose monotonicity the engine cannot always prove, so several
  classify as `U` (unknown) from structure alone. The polarity shown above is reclassified from
  the *runtime* score signs: the heat-radiation and carbon-uptake loops come out balancing (`B`),
  the recycling and sink-weakening loops reinforcing (`R`), exactly as the physics says.
* **Permafrost scores flat zero, and that zero is structural truth.** C-LEARN v77 ships with the
  permafrost module switched off: `sensitivity_of_methane_emissions_to_permafrost_and_clathrate`
  is the literal constant `0`, and it multiplies `flux_c_from_permafrost_release` -- so the flux
  is identically 0.0, the loop transmits nothing, and every LTM link score through it is exactly
  0 at every timestep (a link whose variable never changes has score 0 by definition). LTM
  cleanly distinguishes "structurally present but inactive" from "active but small"; a dominance
  method that couldn't represent *zero* would have you chasing ghosts. The loop stays in the
  pinned set as a tripwire: under a scenario that enables the module, it will start scoring.
""")

# ============================================================================
# 6. The dominance story
# ============================================================================

md("""
## 6. A century of shifting dominance

The pinned loops' relative scores are normalized within their shared cycle partition, so at each
instant these nine |scores| (plus any other activity in the partition) sum to 1: a true
share-of-activity decomposition. We plot the deterministic scenario -- the behavior being
explained on top, the loop shares below. Loops that are never active are left off the chart (and
called out below it): drawing a permanently-zero band would only add legend noise.
""")

code("""
pin_names = {loop.id: (getattr(loop, "name", None) or loop.id) for loop in ltm_run.loops}

rel = {pid: sim.get_relative_loop_score(pid, element="deterministic") for pid in pin_names}
shares = pd.DataFrame(rel, index=pd.Index(time_years, name="year")).fillna(0.0)

# Split the ACTIVE loops by polarity for the stacked chart; an |score| that
# never leaves zero means the loop transmitted nothing all run.
active = [p for p in shares if shares[p].abs().max() > 0]
inactive = [p for p in shares if p not in active]
balancing = [p for p in active if shares[p].mean() < 0]
reinforcing = [p for p in active if shares[p].mean() >= 0]

fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(10.5, 8), sharex=True,
                               gridspec_kw={"height_ratios": [1, 1.6]})

ax1.plot(time_years, sim.get_series("temperature_change_from_preindustrial[deterministic]"),
         color="#c62828", lw=2)
ax1b = ax1.twinx()
ax1b.plot(time_years, sim.get_series("atm_conc_co2[deterministic]"), color="#1976d2", lw=2)
ax1.set_ylabel("temp change (°C)", color="#c62828")
ax1b.set_ylabel("CO2 (ppm)", color="#1976d2")
ax1b.grid(False)
ax1.set_title("the behavior ...")

blues = plt.cm.Blues(np.linspace(0.45, 0.85, len(balancing)))
reds = plt.cm.Reds(np.linspace(0.45, 0.85, len(reinforcing)))
# Stop at 2090: the last decade is dominated by a score singularity (the
# soil-carbon stock approaches constant-rate growth) explained in section 7.
mask = (time_years >= 1900) & (time_years <= 2090)
ax2.stackplot(
    time_years[mask],
    [shares[p].abs().values[mask] for p in balancing + reinforcing],
    labels=[f"{pin_names[p]} (B)" for p in balancing]
           + [f"{pin_names[p]} (R)" for p in reinforcing],
    colors=list(blues) + list(reds), alpha=0.9,
)
ax2.set_xlim(1900, 2090)
ax2.set_ylim(0, 1)
ax2.set_xlabel("year")
ax2.set_ylabel("share of feedback activity")
ax2.set_title("... and the loops driving it: balancing (blue) loses ground to reinforcing (red)")
ax2.legend(loc="upper left", fontsize=7.5, ncol=2, framealpha=0.95)
fig.tight_layout()

print(f"{len(active)} of {len(shares.columns)} pinned loops drawn; not drawn (relative score "
      "exactly 0 for the entire run):")
for p in inactive:
    print(f"  {pin_names[p]}")
""")

code("""
# Decade MEDIANS: robust to the score spikes investigated in section 7.
def decade_median(start: int) -> pd.Series:
    return shares.abs().loc[start:start + 9].median()

decades = pd.DataFrame({f"{y}s": decade_median(y) for y in (1950, 2000, 2050, 2080)})
decades.index = [pin_names[p] for p in decades.index]
decades = decades.sort_values("2080s", ascending=False)

print("decade-median share of feedback activity:\\n")
print(decades.round(3).to_string())

GROUPS = {
    "carbon sinks (B)": ["Ocean carbon uptake", "CO2 fertilization"],
    "heat balance (B)": ["Feedback cooling", "Deep ocean heat uptake"],
    "land carbon churn (R)": ["Biomass carbon recycling", "Soil carbon recycling"],
    "sink weakening (R)": ["Warming weakens ocean uptake", "Warming weakens land uptake"],
}
print("\\nby mechanism:")
for label, members in GROUPS.items():
    row = decades.loc[members].sum()
    print(f"  {label:22s} " + "  ".join(f"{row[c]:5.0%}" for c in decades.columns))
""")

md("""
**This is the loop-dominance story of C-LEARN's century**, and it is sharper than a single
balancing-vs-reinforcing split:

* The **carbon-sink balancing loops** (ocean carbon uptake, CO2 fertilization) erode relentlessly:
  roughly half of all feedback activity in the 1950s, ~5% by mid-century, ~1% by the 2080s. The
  sinks never stop absorbing carbon -- but their grip on the system's *dynamics* collapses.
* Their mirror image, the **sink-weakening reinforcing loops** ("warming weakens ocean uptake" /
  "... land uptake"), climb from ~1% to ~30%: the textbook carbon-climate feedback -- a warmer
  ocean holds less dissolved carbon, stressed ecosystems fix less of it -- emerging from the
  equations, quantified decade by decade.
* The **land carbon churn** (biomass and soil recycling carbon back to the atmosphere) carries a
  large share throughout: the land carbon system is a fast revolving door, not a one-way sink.
  High activity, though -- as section 8 shows -- little net consequence.
* The **heat-balance loops** (feedback cooling, deep-ocean heat uptake) wax and wane with how hard
  radiative forcing is *accelerating*: prominent mid-century when emissions growth is steepest,
  receding later. Balancing strength tracks the push it must answer.
* **Permafrost stays at exactly zero** (switched off in BAU), so it never clutters the story.

A modeler who suspected "the carbon sinks weaken as the planet warms" can now point at a
quantified, time-resolved decomposition instead of an intuition. That is what *debug your
intuition* means.
""")

# ============================================================================
# 7. Why so jagged?
# ============================================================================

md("""
## 7. Why so jagged? Running the abrupt transitions to ground

A careful reader will have flinched at the dominance chart: the shares **lurch**. Before ~2010
they thrash year to year; after 2010 they move in eerily regular steps, with spike-shaped
reshuffles around 2020, 2030, 2040, 2050. Real loop-dominance transitions in a smooth physical
model should not look like a staircase.

Three hypotheses, in increasing order of alarm:

1. **Sampling artifact** -- the model integrates at dt=0.25yr but saves yearly; maybe we are
   aliasing sub-annual score dynamics.
2. **Model structure** -- something in C-LEARN itself genuinely changes at those years.
3. **An LTM implementation bug.**

### Hypothesis 1: sampling

Rerun the identical analysis saving every dt step instead of every year:
""")

code("""
model_dt = simlin.load(MDL)
model_dt.project.set_sim_specs(save_step=0.25)
with model_dt.edit() as (current, patch):
    for name, variables in CLIMATE_LOOPS.items():
        patch.set_loop_name(name, variables)

sim_dt = model_dt.simulate(enable_ltm=True)
sim_dt.run_to_end()
t_dt = sim_dt.get_series("time")

rel_dt = {pid: sim_dt.get_relative_loop_score(pid, element="deterministic") for pid in pin_names}
shares_dt = pd.DataFrame(rel_dt, index=pd.Index(t_dt, name="year")).fillna(0.0)

fig, axes = plt.subplots(2, 1, figsize=(10.5, 6.5), sharex=True)
for ax, (df, t, label) in zip(axes, [
    (shares, time_years, "saved yearly"),
    (shares_dt, t_dt, "saved every dt (0.25 yr)"),
]):
    m = (t >= 2010) & (t <= 2060)
    ax.stackplot(t[m], [df[p].abs().values[m] for p in balancing + reinforcing],
                 colors=list(blues) + list(reds), alpha=0.9)
    ax.set_ylim(0, 1)
    ax.set_ylabel("share")
    ax.set_title(f"loop shares 2010-2060, {label}")
axes[1].set_xlabel("year")
fig.tight_layout()
""")

md("""
Same steps, same spikes, at 4x the sampling rate. **Not a sampling artifact.** (This rerun also
exercised `set_sim_specs(save_step=...)`, which turned up a real API bug on the way -- see the
experience report.)

### Hypothesis 2: model structure

Look at the *raw* (un-normalized) pinned-loop scores around the steps. The LTM flow-to-stock link
score divides the change in each flow by the **acceleration** of the stock it feeds (the corrected
formula in Schoenberg, Hayward & Eberlein 2023) -- so if something kinks the second derivative of
the major stocks, every loop score through them will jump in unison.
""")

code("""
SEP = "\\u205a"
fig, ax = plt.subplots(figsize=(10.5, 4.5))
for pid, name in pin_names.items():
    raw = sim_dt.get_series(f"${SEP}ltm{SEP}loop_score{SEP}{pid}")
    m = (t_dt >= 2012) & (t_dt <= 2060)
    ax.plot(t_dt[m], raw[m], label=name, lw=1.2)
ax.set_yscale("symlog", linthresh=0.01)
for knee in (2021, 2026, 2031, 2041, 2051):
    ax.axvline(knee, color="gray", lw=0.7, ls=":")
ax.legend(fontsize=7, ncol=2, loc="lower right")
ax.set_title("raw pinned-loop scores, dt sampling: every loop jumps at the same years")
ax.set_xlabel("year")
fig.tight_layout()
""")

md("""
Every loop's raw score steps at the *same* years (dotted lines): 2021, 2026, 2031, 2041, 2051 --
one year after each half-decade or decade boundary. That is not loop dynamics; that is an input.

C-LEARN's business-as-usual emissions are exogenous **reference-scenario lookup tables over time**
(`RS CO2 FF[region]`: yearly points, 1850-2100). Second-difference the global emissions input and
the smoking gun appears:
""")

code("""
em = sim_dt.get_series("global_co2_ff_emissions")
d2_em = np.diff(em, n=2)
catm_dt = sim_dt.get_series("c_in_atmosphere[deterministic]")
d2_catm = np.diff(catm_dt, n=2)

fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(10.5, 6), sharex=True)
m = (t_dt[2:] >= 2012) & (t_dt[2:] <= 2060)
ax1.plot(t_dt[2:][m], d2_em[m], lw=1, color="#37474f")
ax1.set_ylabel("d²(emissions)/dt²")
ax1.set_title("the input: fossil CO2 emissions follow piecewise growth segments ...")
ax2.plot(t_dt[2:][m], d2_catm[m], lw=1, color="#1976d2")
ax2.set_ylabel("d²(C in atmosphere)/dt²")
ax2.set_title("... so stock accelerations are near-constant within a segment and jump at the knees")
ax2.set_xlabel("year")
for ax in (ax1, ax2):
    for knee in (2021, 2026, 2031, 2041, 2051):
        ax.axvline(knee, color="gray", lw=0.7, ls=":")
fig.tight_layout()
""")

md("""
The projection data was authored as growth-rate segments anchored at half-decades: between anchors
emissions follow one smooth ramp; at each anchor the growth rate snaps to the next segment's
value. The emissions' second derivative is a spike train at the anchor years, the stocks'
accelerations inherit the staircase -- and since **every flow-to-stock link score has a stock
acceleration in its denominator**, the entire loop-score field reshuffles at each knee. The
pre-2010 thrash is the same mechanism at yearly resolution: historical emissions are yearly
*observations* (wars, recessions, oil shocks), so the accelerations flip constantly and the
shares churn with them.

### Verdict

**Model structure -- hypothesis 2.** The abrupt transitions are LTM faithfully reporting that, in
the second-derivative sense loop scores measure, this model's behavior is paced by the *knees of
its exogenous input data*. No bug; instead, a lesson:

> For heavily data-driven models, instantaneous loop dominance inherits the data's roughness.
> Read dominance as **trends over windows** (the decade-median table in section 6), not as
> point-in-time verdicts. The LTM papers make the matching caveat from the other side: the method
> is designed for *endogenous* behavior, and the more a model is exogenously forced, the less of
> its behavior its loops explain (Schoenberg et al. 2020, sec. on limitations).

The same denominator explains the occasional towering spike (~2019, ~2050): when a stock's
acceleration passes through zero while its flows still change, raw scores diverge -- LTM's
documented inflection-point behavior. The *relative* scores stay bounded through those moments,
which is exactly why the method tells you to interpret relative, not raw, scores.
""")

# ============================================================================
# 8. Dominance is not leverage
# ============================================================================

md("""
## 8. Dominance is not leverage

By the 2080s the two sink-weakening loops carry roughly 30% of all feedback activity. Does that
mean they cause a third of the warming? Easy to check -- the model has a master gain on
exactly that feedback (`sensitivity_of_c_uptake_to_temperature`, default 1). Zero it and the
loops are severed:
""")

code("""
base = model.run(analyze_loops=False)
no_feedback = model.run(
    overrides={"sensitivity_of_c_uptake_to_temperature": 0.0}, analyze_loops=False
)

tvar = "temperature_change_from_preindustrial[deterministic]"
fig, ax = plt.subplots()
ax.plot(base.results.index, base.results[tvar], color="#c62828", lw=2, label="BAU")
ax.plot(no_feedback.results.index, no_feedback.results[tvar], color="#2e7d32", lw=2,
        label="carbon-climate feedback severed")
ax.legend()
ax.set_xlim(1950, 2100)
ax.set_ylabel("temperature change (°C)")
ax.set_title("severing the loops that 'dominate' late-century feedback activity")

for year in (2050, 2100):
    b = base.results[tvar].loc[year]
    c = no_feedback.results[tvar].loc[year]
    print(f"{year}: {b:.2f} °C -> {c:.2f} °C  (the feedback contributes {b - c:+.2f} °C)")
""")

md("""
Severing loops that carry ~30% of late-century *feedback activity* changes 2100 warming by only
~0.15 °C of ~4.5 °C. No contradiction -- two different questions:

* **Dominance** asks: *of the change that is happening, which loops transmit it?* In a model where
  exogenous emissions do most of the pushing, even the dominant loops are passing along an
  externally-driven signal -- and the two big recycling loops carry lots of activity while largely
  canceling each other's net carbon effect.
* **Leverage** asks: *where does intervening change the outcome?* That is a counterfactual
  question, and the LTM papers are explicit that the method does not answer it ("identifies which
  loops dominate, but not where to intervene").

LTM told us where to *look*; the override run told us what it was *worth*. The pairing -- pin,
analyze, then perturb -- is the real workflow, and having both two lines apart in one API is what
makes it usable.
""")

# ============================================================================
# 9. Link-level view
# ============================================================================

md("""
## 9. Bonus: the link-level view

Loop scores are products of **link scores**, and those are queryable too: `sim.get_links()`
returns every causal link with its score series. Raw link scores are not comparable across target
variables (each divides by the change in its own target), so the API also carries a **relative**
link score in [-1, 1] -- the fraction of the target's change attributable to that input. Ranking
by it, restricted to targets fed by more than one scored input (single-input links are trivially
~1), surfaces the model's competitive junctions:
""")

code("""
from collections import Counter

links = sim.get_links()
scored = [ln for ln in links if ln.has_score()]

fan_in = Counter(ln.to_var for ln in scored)
competitive = [ln for ln in scored if fan_in[ln.to_var] > 1]
ranked = sorted(competitive, key=lambda ln: -abs(ln.average_relative_score() or 0.0))

print(f"{len(links)} links, {len(scored)} scored, {len(competitive)} into multi-input targets\\n")
print("strongest competitive links (mean relative score):")
for ln in ranked[:10]:
    print(f"  {ln.average_relative_score():+.2f}  {ln}")
""")

md("""
The carbon-cycle and forcing junctions dominate, as the loop analysis predicts -- and the polarity
column (`+`/`-`/`?`) shows where static analysis gave up (`?`, mostly lookup tables) while the
runtime scores still resolve the sign.
""")

# ============================================================================
# 10. Experience report
# ============================================================================

md("""
## 10. Experience report

This notebook doubled as a stress test of the LTM implementation and the pysimlin API -- the point
was to *follow the real path* (load, explore, run, pin, analyze, question, perturb) and write down
everything that resisted. Verdict up front: **the method works on a production-scale model, the
auto-switch and pinning workflows are the right shape, and the rough edges found this session were
specific and fixable -- several were fixed in the engine while this notebook was being written.**

### What worked well

* **Vensim import and simulation**: a 53,000-line production `.mdl` loads in well under a second,
  simulates correctly, renders its diagram, and exposes structure (equations, links, polarities,
  sector groups) through a clean Python API.
* **The auto-switch between exhaustive and discovery mode makes sense.** Exhaustive enumeration is
  simply not on the table for a graph with C-LEARN's connectivity, and the engine (a) detects that
  itself, (b) *tells you* via a `RuntimeWarning` plus `run.ltm_mode`, and (c) hands you the two
  tools that work at scale -- `analyze()` for breadth, pins for depth. The failure mode this
  design avoids (silently returning an empty loop list) is exactly the kind of thing that erodes
  trust in analysis tooling.
* **Pinning is the right interaction model for big models** -- the papers were right to insist on
  it (`LOOPSCORE`): discovery is heuristic and scenario-dependent, while the questions a
  practitioner brings ("track *ocean carbon uptake*") are stable. Declaring loops by variable set
  and getting back named, per-scenario-element score series is exactly the workflow the original
  papers sketch, and it held up against a real model. Nothing further to add here: pinning
  *loops* is implemented; pinning *paths* is the one gap (below).
* **Honest zeros and honest polarity**: the switched-off permafrost loop scores exactly 0; loops
  that static analysis can only call `U` are correctly reclassified from runtime scores.
* **Performance is a non-issue at this scale**: LTM-instrumented compile+run takes seconds, and
  the full discovery sweep over 251 timesteps is sub-second on top of it.

### What this exercise fixed (engine/API commits made alongside this notebook)

* **Vanishing diagnostics (engine, salsa accumulators).** Project diagnostics were collected via
  salsa accumulators, whose values are not replayed for queries that are validated-but-not-
  re-executed after an unrelated input change. Net effect: `project.get_errors()` returned 14 unit
  warnings on freshly-loaded C-LEARN, then `[]` after any unrelated patch -- and worse, the patch
  validator's before/after warning comparison would misfire and **reject valid edits** ("patch
  introduces unit warning ... which previously had none", order-dependent).
* **Accepted patches that raised anyway (pysimlin).** The Python layer treated *any* collected
  diagnostics as failure, so on a model with pre-existing unit warnings (i.e. most real Vensim
  imports) `set_sim_specs(save_step=0.25)` raised "Patch produced validation errors" -- *after
  committing the patch*. Rejection (an engine decision) and informational diagnostics are now
  distinct, and rejections carry their details on the exception.
* **Pinned-loop names were unrecoverable (FFI + pysimlin).** You named loops at pin time but got
  back only `pin1`...`pin9`, in patch order. The name now rides through the C ABI onto
  `Loop.name` everywhere loops are surfaced.
* **Discovery importance was raw and unrankable (engine + FFI).** `analyze()` returned loops whose
  importance series were raw loop scores -- unbounded, incomparable across cycle partitions (an
  isolated HFC-uptake loop showed "importance 72", the climate core "17,000"), and inconsistent
  with both the engine's own ranking and the pinned-loop path. The importance series is now the
  signed *relative* loop score in [-1, 1], and dominant periods are computed from it.

### Observations for future work

* **Windowed/smoothed dominance as a first-class view.** Section 7's lesson generalizes: on
  data-driven models, instantaneous shares are noisy, and per-timestep "dominant period" labels
  flicker (C-LEARN produces ~110 mostly one-year periods). The papers' own loop-inclusion
  threshold uses time-averaged magnitudes; an API-level windowed share (or a min-duration
  parameter on dominant periods) would make the default output match how it has to be read.
* **`PATHSCORE` remains the one missing piece of the papers' toolkit.** Link scores compose along
  paths (section 9 builds products by hand); a `get_path_score(["a", "b", "c"])` helper would
  round out parity with the Stella builtins.
* **Singleton-partition loops pin the top of discovery rankings** at ±1.0 by construction (every
  isolated stock-decay loop "dominates" its own partition). Correct per the method, but the API
  could expose the partition (or its loop count) on each loop so callers can group or filter.
* **Arrayed pins default to an argmax-abs aggregate across elements** in
  `get_relative_loop_score(id)` and `run.loops`. Documented, and per-element access works
  (`element="deterministic"`), but summed |shares| only equal 1 *per element*, so the aggregate
  trips up exactly the share-of-activity charts the scores exist for. Per-element expansion in
  `run.loops` would be less surprising.
* **Static lookup-table polarity** is `?` for many physically-monotone curves; better
  monotonicity classification would shrink the `U` loop population (runtime reclassification
  currently papers over it, but only after a simulation).

### The bottom line

The promise of LTM -- *which feedback structure is driving behavior, when, and by how much* --
held up on a real climate-policy model: the engine found the carbon-climate core unprompted,
tracked nine named loops through 250 years, exposed dominance shifting from the planet's balancing
machinery to the reinforcing sink-weakening feedbacks, and explained its own jagged edges down to
the knees in the input data. And the exercise of *writing it down as a notebook* surfaced four
real defects in a day -- which is the strongest argument for keeping an experience-report notebook
in the loop for every analysis feature.
""")

nb["cells"] = cells

out_path = NOTEBOOKS_DIR / "clearn_ltm_experience.ipynb"
with open(out_path, "w") as f:
    nbf.write(nb, f)
print(f"wrote {out_path} ({len(cells)} cells)")

# Execute in-place so output cells are populated.
from nbclient import NotebookClient

client = NotebookClient(
    nb,
    timeout=600,
    kernel_name="python3",
    resources={"metadata": {"path": str(NOTEBOOKS_DIR)}},
)
client.execute()

with open(out_path, "w") as f:
    nbf.write(nb, f)
print("executed and saved with outputs")
