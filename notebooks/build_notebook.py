#!/usr/bin/env python3
"""Build the C-LEARN LTM experience notebook.

Constructs notebooks/clearn_ltm_experience.ipynb cell-by-cell via nbformat,
then executes it (populating outputs) with nbclient. The executed .ipynb and
its HTML render are generated artifacts (gitignored); this script is the
source of truth, so the notebook can be regenerated against any engine build.

Run from the pysimlin venv (which needs nbformat/nbclient/matplotlib/pillow
on top of the dev deps):

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

[C-LEARN](https://www.climateinteractive.org/) (v77, 2010) is the simulation core that became C-ROADS: a
globally-aggregated climate-policy model built in Vensim. It couples a carbon cycle, an
energy-balance temperature model, other greenhouse gases, permafrost feedbacks, and sea-level rise
to regional emissions-policy levers, and it runs from 1850 to 2100. Importantly for us, it is a
*real* practitioner model: 911 variables, 24 stocks, arrayed equations, Vensim macros, lookup
tables -- all the things that make production models hard for analysis tooling.

**Loops That Matter** (LTM; Schoenberg, Davidsen & Eberlein 2020; Eberlein & Schoenberg 2020;
Schoenberg, Hayward & Eberlein 2023) is a method for *loop dominance analysis*: it measures, at
every instant of a simulation, how much each feedback loop contributes to the model's behavior.
Where a modeler's intuition says "warming is accelerating because the permafrost feedback kicked
in", LTM aims to make that statement quantitative and checkable.

This notebook does four things, in order:

1. **Explores C-LEARN itself** -- structure, behavior, and its climate feedback loops.
2. **Runs LTM's loop-dominance analysis on C-LEARN's climate core** -- composing loop scores from
   the engine's link scores, and showing how dominance shifts from the balancing carbon-uptake
   loops toward the reinforcing carbon-climate feedbacks over the 21st century.
3. **Demonstrates the engine-native LTM experience** on models where it works end-to-end (the
   logistic growth model, and the three-party arms race model from the LTM papers).
4. **Reports honestly on the gaps**: what still doesn't work, why, and what was fixed and filed
   along the way. This notebook was itself the test that drove six engine/API fixes (plus four
   LTM compilation fixes that landed on `main` between its first and final drafts).

> Built against pysimlin from `main` at `099c5659` (2026-06-01).
""")

# ============================================================================
# Section 1: loading the model
# ============================================================================

md("""
## 1. Loading C-LEARN

`simlin.load()` reads Vensim `.mdl` files directly (no conversion step). The translation -- Vensim
macros, subscripts, lookup tables, and the equation language -- happens in Simlin's native Rust
importer.
""")

code("""
import warnings
from collections import Counter, defaultdict, deque
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

print(f"variables: {len(model.get_var_names())} total")
print(f"  {len(stocks):4d} stocks")
print(f"  {len(flows):4d} flows")
print(f"  {len(auxes):4d} auxiliaries")
print(f"\\ntime: {model.time_spec.start:.0f} to {model.time_spec.stop:.0f} "
      f"(dt={model.time_spec.dt}, units={model.time_spec.units})")
print("\\nstocks:")
for s in stocks:
    print(f"  {s}")
""")

md("""
The stocks tell you what kind of model this is. The physical climate core:

* **Carbon cycle**: `c_in_atmosphere`, `c_in_mixed_layer` (surface ocean), `c_in_deep_ocean`,
  `c_in_biomass`, `c_in_humus` (soil)
* **Energy balance**: `heat_in_atmosphere_and_upper_ocean`, `heat_in_deep_ocean`
* **Other greenhouse gases**: `ch4_in_atm`, `n2o`, `sf6`, `hfc`, `pfc`
* **Climate feedbacks**: `total_c_from_permafrost`, `total_ch4_released`
* **Impacts**: `sea_level_rise`

plus policy bookkeeping (`cumulative_co2*`, `im_*` international-agreement stocks).

Most variables are arrayed over a `scenario` dimension -- C-LEARN simulates three climate
sensitivities (deterministic / high / low) side by side in one run.
""")

# ============================================================================
# Section 2: equations and the arrayed-equation API
# ============================================================================

md("""
### Reading equations

Each variable's equation is available through `get_variable()`. Arrayed variables that the Vensim
importer expanded element-by-element expose their formulas through `element_equations` (and
collapse back to a single `equation` when every element has the same formula).
""")

code("""
for name in ["atm_conc_co2", "temperature_change_from_preindustrial", "flux_atm_to_ocean"]:
    var = model.get_variable(name)
    print(f"{name}  [{', '.join(var.dimensions)}]")
    print(f"  units:    {var.units}")
    print(f"  equation: {var.equation}")
    if var.documentation:
        print(f"  doc:      {var.documentation[:90]}")
    print()
""")

# ============================================================================
# Section 3: the diagram
# ============================================================================

md("""
## 2. The model diagram

C-LEARN ships with its Vensim sketch -- 14 views' worth -- which Simlin imports and renders via
`project.render_png()` / `render_svg()`. The full canvas stacks all 14 views vertically; here is
one of the climate-sector views cropped out of it:
""")

code("""
import io

from IPython.display import Image, display
from PIL import Image as PILImage

png = model.project.render_png("main", width=1600)
full = PILImage.open(io.BytesIO(png))
print(f"full rendered canvas: {full.width} x {full.height} px ({len(png) / 1e6:.1f} MB, all 14 views)")

# Crop one sector view out of the stacked canvas.
crop = full.crop((0, 15050, 1450, 16450))
buf = io.BytesIO()
crop.save(buf, format="PNG")
display(Image(data=buf.getvalue(), width=920))
""")

md("""
The render is faithful: each gray rounded box is one Vensim view (imported as a sector group), and
the stocks, flows, and connectors inside come through cleanly.

(A satisfying aside: the first draft of this notebook found every one of these views rendering as a
**solid black rectangle** -- the SVG stylesheet had no rule for sector/group boxes, so they took
SVG's default fill. Models without sector boxes never hit it, which is why small fixtures looked
fine. Fixed in `a51e9191` as part of this exercise.)

Still, at 911 variables a structural diagram is something you *navigate*, not something you *read*.
This is exactly the argument the LTM papers make for **behavior-driven simplified diagrams** (their
"simplified CLD" concept): show only the loops that matter, sized by how much they matter. Simlin
has the metrics for this; the visualization layer doesn't exist yet.
""")

# ============================================================================
# Section 4: baseline run
# ============================================================================

md("""
## 3. The baseline run

`model.run()` simulates the model, and by default also asks for loop analysis
(`analyze_loops=True`). On a model this large, LTM resolves to **discovery mode**: the feedback
structure is too big to enumerate exhaustively (loop counts grow roughly factorially with model
size), so the engine instruments every causal link instead, and `run.loops` only contains loops
someone explicitly asked about. pysimlin warns about exactly that, so an empty loop list is never
mistaken for "this model has no feedback":
""")

code("""
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    run = model.run()

for w in caught:
    print(f"{w.category.__name__}:")
    print(f"  {w.message}")

print(f"\\nltm_mode: {run.ltm_mode!r}")
print(f"results: {run.results.shape[0]} timesteps x {run.results.shape[1]} series")
""")

md("""
(Until very recently this was much worse: the LTM instrumentation for C-LEARN needed ~171k result
slots, which silently overflowed the engine's 16-bit slot addressing and corrupted *every* result
-- and after that was turned into a hard error, LTM simply couldn't run on C-LEARN at all. The
O(N^2)-blowup fixes that landed on `main` this week shrank the instrumentation enough that an
LTM-enabled C-LEARN run now compiles and runs in ~8 seconds, correctly. Section 6 puts that
instrumentation to work.)
""")

md("""
### What C-LEARN projects

The business-as-usual run (no policy levers): atmospheric CO2 roughly triples from its
pre-industrial ~280 ppm, and global mean temperature rises about 4.5°C by 2100. The three lines per
plot are the three climate-sensitivity scenarios the model carries in its `scenario` dimension.
""")

code("""
results = run.results
scenarios = ["deterministic", "high_2xco2_sensitivity", "low_2xco2_sensitivity"]
labels = {"deterministic": "best estimate (3.0°C / 2xCO2)",
          "high_2xco2_sensitivity": "high sensitivity (4.5°C)",
          "low_2xco2_sensitivity": "low sensitivity (2.0°C)"}
colors = {"deterministic": "#1976d2", "high_2xco2_sensitivity": "#c62828",
          "low_2xco2_sensitivity": "#2e7d32"}

fig, axes = plt.subplots(1, 3, figsize=(13, 4))

for scen in scenarios:
    axes[0].plot(results.index, results[f"atm_conc_co2[{scen}]"],
                 color=colors[scen], label=labels[scen])
    axes[1].plot(results.index, results[f"temperature_change_from_preindustrial[{scen}]"],
                 color=colors[scen])
    axes[2].plot(results.index, results[f"sea_level_rise[{scen}]"] * 1000,
                 color=colors[scen])

axes[0].set_title("Atmospheric CO2 (ppm)")
axes[0].legend(fontsize=8, loc="upper left")
axes[1].set_title("Temperature change from pre-industrial (°C)")
axes[2].set_title("Sea level rise (mm)")
for ax in axes:
    ax.set_xlabel("year")
    ax.set_xlim(1900, 2100)
fig.suptitle("C-LEARN business-as-usual projection", y=1.02)
fig.tight_layout()
""")

# ============================================================================
# Section 5: causal structure
# ============================================================================

md("""
## 4. The causal skeleton

Before any loop analysis, the model's *static* causal structure -- which variable affects which,
and with what polarity -- is available from `get_links()`. Polarity is determined by analyzing each
equation's monotonicity: `+` means "more of A gives more of B", `-` the opposite, `?` means the
equation is non-monotone (or beyond the analyzer).

Macro internals (the stocks hidden inside `SMOOTH`, `DELAY3`, etc.) are collapsed into composite
edges, so what you see matches the model as its author drew it.
""")

code("""
links = model.get_links()
polarity_counts = Counter(str(link.polarity) for link in links)

print(f"{len(links)} causal links")
for pol, count in sorted(polarity_counts.items(), key=lambda kv: -kv[1]):
    print(f"  {pol}: {count}")
""")

md("""
A third of the links get a definite polarity from static analysis. The `?` links are mostly
either genuinely non-monotone equations (`IF THEN ELSE` policy switches, products of two changing
quantities) or arrayed equations whose elements the analyzer conservatively declines to summarize
-- one of the improvement areas the experience report comes back to.

### Tracing the climate core

The interesting causal paths for climate dynamics: how does carbon in the atmosphere come back
around to affect itself? We can walk the link graph directly.
""")

code("""
fwd = defaultdict(list)
for link in links:
    fwd[link.from_var].append(link.to_var)


def shortest_path(src: str, dst: str, max_len: int = 12) -> list[str] | None:
    \"\"\"BFS shortest path src -> dst over the causal link graph.\"\"\"
    queue = deque([[src]])
    seen = {src}
    while queue:
        path = queue.popleft()
        if len(path) > max_len:
            return None
        for nxt in fwd[path[-1]]:
            if nxt == dst:
                return path + [nxt]
            if nxt not in seen:
                seen.add(nxt)
                queue.append(path + [nxt])
    return None


paths_to_show = [
    ("how CO2 warms the planet", "c_in_atmosphere", "heat_in_atmosphere_and_upper_ocean"),
    ("how warming melts permafrost (releasing more CO2)",
     "temperature_change_from_preindustrial", "c_in_atmosphere"),
    ("how warming releases methane", "temperature_change_from_preindustrial", "ch4_in_atm"),
    ("how the ocean takes up carbon", "c_in_atmosphere", "c_in_mixed_layer"),
    ("how warming weakens ocean uptake", "temperature_change_from_preindustrial",
     "equil_c_in_mixed_layer"),
]

for title, src, dst in paths_to_show:
    path = shortest_path(src, dst)
    print(f"{title}:")
    print("    " + "\\n      -> ".join(path))
    print()
""")

# ============================================================================
# Section 6: the feedback loops + pinning
# ============================================================================

md("""
## 5. C-LEARN's climate feedback loops

Putting those paths together, the scientific core of C-LEARN is a set of competing feedback loops
around two coupled stocks -- carbon in the atmosphere, and heat in the atmosphere & upper ocean:

| Loop | Polarity | Mechanism |
|------|----------|-----------|
| **Feedback cooling** | balancing | A hotter planet radiates more heat to space (the Planck response) |
| **Deep-ocean heat uptake** | balancing | Surface warming drives heat into the deep ocean, slowing surface warming |
| **Ocean carbon uptake** | balancing | More atmospheric CO2 -> more dissolves into the surface ocean |
| **Biomass carbon uptake** | balancing | CO2 fertilization: plants grow faster, absorbing carbon |
| **Soil carbon cycle** | balancing | The longer path: biomass -> soil humus -> back to atmosphere |
| **Permafrost carbon release** | reinforcing | Warming melts permafrost, releasing CO2, causing more warming |
| **Ocean uptake weakening** | reinforcing | Warming reduces CO2 solubility in seawater, so more stays in the air |
| **Biomass uptake weakening** | reinforcing | Warming stresses ecosystems, reducing their carbon uptake |

Whether the balancing loops keep winning -- and when the reinforcing ones start to matter -- *is*
the climate question, stated in feedback terms. This is exactly what LTM is supposed to quantify.

### Pinning the loops

C-LEARN is far too large for Simlin to enumerate every feedback loop (the loop count grows roughly
factorially with model size; the engine caps exhaustive enumeration at strongly-connected
components of 50 nodes). For models like this, the LTM literature's answer is to let the modeler
**name the loops they care about** (Stella's `LOOPSCORE` function). Simlin's equivalent is *loop
pinning*: `set_loop_name()` registers a loop by its variable cycle, and the engine then always
tracks it.

Until this week pinning didn't work at all on Vensim-imported models (the `SetLoopName` operation
required variable UIDs that the importer never assigns -- now fixed). Here it is working:
""")

code("""
CLIMATE_LOOPS = {
    "Feedback cooling (B)": [
        "heat_in_atmosphere_and_upper_ocean", "temperature_change_from_preindustrial",
        "feedback_cooling", "heat_in_atmosphere_and_upper_ocean_net_flow",
    ],
    "Deep ocean heat uptake (B)": [
        "heat_in_atmosphere_and_upper_ocean", "temperature_change_from_preindustrial",
        "heat_transfer", "heat_in_atmosphere_and_upper_ocean_net_flow",
    ],
    "Ocean carbon uptake (B)": [
        "c_in_atmosphere", "equil_c_in_mixed_layer", "flux_atm_to_ocean",
    ],
    "Biomass carbon uptake (B)": [
        "c_in_atmosphere", "flux_atm_to_biomass", "c_in_biomass", "flux_biomass_to_atmosphere",
    ],
    "Soil carbon cycle (B)": [
        "c_in_atmosphere", "flux_atm_to_biomass", "c_in_biomass", "flux_biomass_to_humus",
        "c_in_humus", "flux_humus_to_atmosphere",
    ],
    "Permafrost carbon release (R)": [
        "c_in_atmosphere", "co2_radiative_forcing", "total_radiative_forcing",
        "heat_in_atmosphere_and_upper_ocean_net_flow", "heat_in_atmosphere_and_upper_ocean",
        "temperature_change_from_preindustrial", "flux_c_from_permafrost_release",
    ],
    "Ocean uptake weakening (R)": [
        "heat_in_atmosphere_and_upper_ocean", "temperature_change_from_preindustrial",
        "effect_of_temp_on_dic_pco2", "equil_c_in_mixed_layer", "flux_atm_to_ocean",
        "c_in_atmosphere", "co2_radiative_forcing", "total_radiative_forcing",
        "heat_in_atmosphere_and_upper_ocean_net_flow",
    ],
    "Biomass uptake weakening (R)": [
        "heat_in_atmosphere_and_upper_ocean", "temperature_change_from_preindustrial",
        "effect_of_warming_on_c_flux_to_biomass", "flux_atm_to_biomass",
        "c_in_atmosphere", "co2_radiative_forcing", "total_radiative_forcing",
        "heat_in_atmosphere_and_upper_ocean_net_flow",
    ],
}

with model.edit() as (current, patch):
    for name, variables in CLIMATE_LOOPS.items():
        patch.set_loop_name(name, variables)

print(f"pinned {len(CLIMATE_LOOPS)} loops\\n")
for loop in model.loops:
    print(f"  {loop.id}: {' -> '.join(loop.variables)}")
    print()
""")

md("""
The engine recovered each loop's cycle from the unordered variable sets, and the pinned loops now
appear in `model.loops` with stable `pin{n}` ids -- even though this model is in "discovery" LTM
territory where nothing else is enumerated.

### The remaining gap: pin *scores* read as zero

Scoring a pinned loop means running with LTM instrumentation and reading the loop's score series.
The run itself now works (more on that below) -- but on arrayed models like C-LEARN, the engine's
generated pin-score equations mix scalar and arrayed references, fail to compile, and silently fall
back to zero ([#653](https://github.com/bpowers/simlin/issues/653)):
""")

code("""
import numpy as np

sim = model.simulate(enable_ltm=True)
sim.run_to_end()
print(f"LTM-instrumented run succeeded (mode: {sim.get_ltm_mode()})\\n")

run_ltm = sim.get_run()
for loop in run_ltm.loops:
    ts = loop.behavior_time_series
    finite = ts[np.isfinite(ts)] if ts is not None else np.array([])
    n_nonzero = int((finite != 0).sum()) if len(finite) else 0
    n_total = len(ts) if ts is not None else 0
    print(f"  {loop.id}: {n_nonzero} non-zero score values out of {n_total}")
""")

md("""
(That an LTM-instrumented C-LEARN run *succeeds at all* is new as of this week: previously the
~171k result slots of instrumentation silently overflowed the engine's 16-bit slot addressing and
corrupted every result -- the variable that landed on slot 0 overwrote simulated *time*. After
that became a hard error, four O(N^2)-blowup fixes on `main` shrank the instrumentation enough to
fit. The remaining issues are tracked in
[#647](https://github.com/bpowers/simlin/issues/647),
[#651](https://github.com/bpowers/simlin/issues/651), and
[#653](https://github.com/bpowers/simlin/issues/653).)

So the engine computes and carries every link score, but the final pin-score composition step is
broken for arrayed loops. Time to route around it.
""")

# ============================================================================
# Section 6 (NEW): manual loop-score composition -- the dominance analysis
# ============================================================================

md("""
## 6. The loops that matter in C-LEARN

Here is the thing about LTM's design that saves us: a **loop score is just the product of the link
scores around the loop's cycle** -- that *is* the definition (Schoenberg et al. 2020, Eq. 3). The
engine's per-link scores are all present in the LTM-instrumented run as synthetic result series, so
we can compose the loop scores ourselves in a few lines of numpy, exactly as the engine should (and
one day will) do for pinned loops.

We analyze the `deterministic` (best-estimate climate sensitivity) scenario.
""")

code("""
SEP, ARROW = "\\u205a", "\\u2192"  # the reserved separators in synthetic LTM variable names
SCEN = "deterministic"


def link_score(frm: str, to: str) -> np.ndarray:
    \"\"\"Read the LTM link-score series for the causal link `frm -> to`.

    The engine names most link scores `$:ltm:link_score:{from}->{to}`; links whose
    arrayed source is referenced at a specific element carry that element on the
    `from` side. Try the forms in order.
    \"\"\"
    forms = [
        f"${SEP}ltm{SEP}link_score{SEP}{frm}{ARROW}{to}",
        f"${SEP}ltm{SEP}link_score{SEP}{frm}[{SCEN}]{ARROW}{to}",
        f"${SEP}ltm{SEP}link_score{SEP}{frm}[{SCEN},layer1]{ARROW}{to}",
    ]
    for form in forms:
        try:
            return sim.get_series(form)
        except simlin.SimlinRuntimeError:
            continue
    raise KeyError(f"no link-score series found for {frm} -> {to}")


def loop_score(chain: list[tuple[str, str]]) -> np.ndarray:
    \"\"\"A loop's score series: the product of its link scores (LTM Eq. 3).\"\"\"
    score = np.ones(len(time_years))
    for frm, to in chain:
        score = score * link_score(frm, to)
    return score


time_years = sim.get_series("time")

HEAT, NETFLOW = "heat_in_atmosphere_and_upper_ocean", "heat_in_atmosphere_and_upper_ocean_net_flow"
TEMP, CATM = "temperature_change_from_preindustrial", "c_in_atmosphere"

# The nine climate feedback loops, as causal-link chains.
CLIMATE_LOOP_CHAINS = {
    "Feedback cooling": [
        (HEAT, TEMP), (TEMP, "feedback_cooling"), ("feedback_cooling", NETFLOW), (NETFLOW, HEAT)],
    "Deep ocean heat uptake": [
        (HEAT, TEMP), (TEMP, "heat_transfer"), ("heat_transfer", NETFLOW), (NETFLOW, HEAT)],
    "Ocean carbon uptake": [
        (CATM, "equil_c_in_mixed_layer"), ("equil_c_in_mixed_layer", "flux_atm_to_ocean"),
        ("flux_atm_to_ocean", CATM)],
    "CO2 fertilization": [
        (CATM, "flux_atm_to_biomass"), ("flux_atm_to_biomass", CATM)],
    "Biomass carbon recycling": [
        (CATM, "flux_atm_to_biomass"), ("flux_atm_to_biomass", "c_in_biomass"),
        ("c_in_biomass", "flux_biomass_to_atmosphere"), ("flux_biomass_to_atmosphere", CATM)],
    "Soil carbon recycling": [
        (CATM, "flux_atm_to_biomass"), ("flux_atm_to_biomass", "c_in_biomass"),
        ("c_in_biomass", "flux_biomass_to_humus"), ("flux_biomass_to_humus", "c_in_humus"),
        ("c_in_humus", "flux_humus_to_atmosphere"), ("flux_humus_to_atmosphere", CATM)],
    "Warming weakens ocean uptake": [
        (HEAT, TEMP), (TEMP, "effect_of_temp_on_dic_pco2"),
        ("effect_of_temp_on_dic_pco2", "equil_c_in_mixed_layer"),
        ("equil_c_in_mixed_layer", "flux_atm_to_ocean"), ("flux_atm_to_ocean", CATM),
        (CATM, "co2_radiative_forcing"), ("co2_radiative_forcing", "total_radiative_forcing"),
        ("total_radiative_forcing", NETFLOW), (NETFLOW, HEAT)],
    "Warming weakens land uptake": [
        (HEAT, TEMP), (TEMP, "effect_of_warming_on_c_flux_to_biomass"),
        ("effect_of_warming_on_c_flux_to_biomass", "flux_atm_to_biomass"),
        ("flux_atm_to_biomass", CATM),
        (CATM, "co2_radiative_forcing"), ("co2_radiative_forcing", "total_radiative_forcing"),
        ("total_radiative_forcing", NETFLOW), (NETFLOW, HEAT)],
    "Permafrost carbon release": [
        (HEAT, TEMP), (TEMP, "flux_c_from_permafrost_release"),
        ("flux_c_from_permafrost_release", CATM),
        (CATM, "co2_radiative_forcing"), ("co2_radiative_forcing", "total_radiative_forcing"),
        ("total_radiative_forcing", NETFLOW), (NETFLOW, HEAT)],
}

scores = pd.DataFrame(
    {name: loop_score(chain) for name, chain in CLIMATE_LOOP_CHAINS.items()},
    index=pd.Index(time_years, name="year"),
)

# A loop's polarity is the sign of its score: negative = balancing, positive = reinforcing.
summary = pd.DataFrame({
    "polarity": ["B (balancing)" if scores[c].mean() < 0 else "R (reinforcing)" for c in scores],
    "score @ 1950": scores.iloc[100].round(2),
    "score @ 2000": scores.iloc[150].round(2),
    "score @ 2050": scores.iloc[200].round(2),
    "score @ 2095": scores.iloc[245].round(2),
})
summary
""")

md("""
Every loop's polarity comes out as the physics says it should: the heat-radiation and carbon-uptake
loops are **balancing** (negative scores), the recycling and warming-feedback loops are
**reinforcing** (positive), and permafrost is exactly zero -- C-LEARN's permafrost module is
switched off in the BAU scenario, and LTM correctly reports an inactive loop as contributing
nothing.

Raw loop scores grow without bound as the system accelerates (the denominators in LTM's link
scores shrink), so the readable view is each loop's **share** of the total tracked feedback
activity:
""")

code("""
# Relative contribution among the tracked loops (the LTM "relative loop score",
# restricted to the nine loops we are tracking).
shares = scores.abs().div(scores.abs().sum(axis=1), axis=0)

balancing = [c for c in scores.columns if scores[c].mean() < 0]
reinforcing = [c for c in scores.columns if scores[c].mean() >= 0 and scores[c].abs().max() > 0]

fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(10.5, 8), sharex=True,
                                gridspec_kw={"height_ratios": [1, 1.5]})

# Top: the behavior being explained
ax1.plot(time_years, sim.get_series(f"temperature_change_from_preindustrial[{SCEN}]"),
         color="#c62828", lw=2)
ax1b = ax1.twinx()
ax1b.plot(time_years, sim.get_series(f"atm_conc_co2[{SCEN}]"), color="#1976d2", lw=2)
ax1.set_ylabel("temperature change (°C)", color="#c62828")
ax1b.set_ylabel("atmospheric CO2 (ppm)", color="#1976d2")
ax1b.grid(False)
ax1.set_xlim(1910, 2085)
ax1.set_title("C-LEARN business-as-usual: the behavior ...")

# Bottom: stacked shares of feedback activity
blues = plt.cm.Blues(np.linspace(0.45, 0.85, len(balancing)))
reds = plt.cm.Reds(np.linspace(0.45, 0.85, len(reinforcing)))
# Window: 1910 (skip the noisy near-equilibrium start) to 2085 (after which the
# soil-recycling loop hits an LTM score singularity -- explained below).
lo, hi = 60, 235
ax2.stackplot(
    time_years[lo:hi],
    [shares[c].values[lo:hi] for c in balancing + reinforcing],
    labels=[f"{c} (B)" for c in balancing] + [f"{c} (R)" for c in reinforcing],
    colors=list(blues) + list(reds),
    alpha=0.9,
)
ax2.set_xlim(1910, 2085)
ax2.set_ylim(0, 1)
ax2.set_xlabel("year")
ax2.set_ylabel("share of tracked feedback activity")
ax2.set_title("... and the loops driving it: balancing (blue) loses ground to reinforcing (red)")
ax2.legend(loc="upper left", fontsize=8, ncol=2, framealpha=0.95)
fig.tight_layout()
""")

code("""
# The headline numbers: how the balance of power shifts over the century.
def share_at(year: float) -> pd.Series:
    idx = int(year - time_years[0])
    return shares.iloc[idx]

years = ["1950", "2000", "2050", "2085"]
shift = pd.DataFrame({y: share_at(int(y)) for y in years}).round(3)
shift["type"] = ["B" if c in balancing else "R" for c in shift.index]
shift = shift.sort_values("2085", ascending=False)

print("Share of tracked feedback activity:\\n")
print(shift.to_string())
print()
b_total = shift[shift.type == "B"][years].sum()
r_total = shift[shift.type == "R"][years].sum()
b_str = " -> ".join(f"{b_total[y]:.0%}" for y in years)
r_str = " -> ".join(f"{r_total[y]:.0%}" for y in years)
print(f"All balancing loops:   {b_str}")
print(f"All reinforcing loops: {r_str}")
""")

md("""
**This is the loop-dominance story of C-LEARN's century**, and it lines up with climate science:

* The **carbon-recycling loops** (biomass and soil returning carbon to the atmosphere) carry a
  large share throughout: the land carbon system is a fast revolving door, not a one-way sink.
* The single biggest *mover* is **"warming weakens ocean uptake"** -- the textbook carbon-climate
  feedback. Its score grows roughly 3,000x over the century (0.03 in 1950 to 90 by 2095): from
  negligible to a first-order driver. "Warming weakens land uptake" follows the same trajectory
  one step behind.
* The planet's **balancing machinery** (heat radiation, ocean/land carbon uptake) still claims
  about half of the feedback activity at mid-century -- but only ~30% by 2085, with the warming
  feedbacks taking the difference. (The volatility before ~2010 is real: the historical emissions
  data drives year-to-year swings in which loops are doing the work.)
* **Permafrost stays at exactly zero** because C-LEARN's BAU scenario has the permafrost module
  switched off -- LTM cleanly distinguishes "inactive structure" from "active but small".

### What happens after 2085 (and why the plot stops there)

Look back at the score table: "Soil carbon recycling" reaches **+1,740** by 2095, two orders of
magnitude beyond every other loop. That is not an error and not (directly) a physical statement --
it is LTM's documented behavior near a *zero-acceleration* point (Schoenberg, Hayward & Eberlein
2023, sec. 6.3). The corrected flow-to-stock link score divides the change in a flow by the
**acceleration** of the stock it feeds; late in the century, C-LEARN's soil-carbon stock settles
into nearly constant-rate growth (its inflow and outflow growth balance), so that denominator
approaches zero and every score through it diverges.

The diverging loop *is* telling you something -- "this subsystem has stopped accelerating" -- but
it makes share-based plots unreadable, which is why the dominance chart above stops at 2085. Any
production LTM tooling needs a presentation answer for these singularities; this is one of the
API observations in the final section.

A modeler who suspected "the carbon sinks weaken as the planet warms" can now point at a
quantified, time-resolved decomposition instead of an intuition. That is what *debug your
intuition* means.
""")

# ============================================================================
# Section 7: what LTM gives you (logistic growth)
# ============================================================================

md("""
## 7. The engine-native LTM experience

Section 6 composed loop scores by hand because of the pinned-loop bug. On models where LTM works
end-to-end, the engine does all of that itself -- loops, relative scores, and dominance periods
arrive ready-made on every `run()`. Two examples show what that looks like (and what C-LEARN's
API experience will be once #653 lands).

### 7a. Logistic growth: the textbook dominance shift

The simplest interesting case: a population growing toward a carrying capacity. Two loops --
reinforcing growth (more population -> more births) and a balancing constraint (more population ->
fuller capacity -> lower growth rate). The S-shaped trajectory everyone knows is *defined* by the
handoff between them.
""")

code("""
logistic = simlin.load(REPO / "test" / "logistic_growth_ltm" / "logistic_growth.stmx")
display(Image(data=logistic.project.render_png("main", width=900), width=750))
""")

code("""
lrun = logistic.run()

print(f"ltm_mode: {lrun.ltm_mode!r}\\n")
for loop in lrun.loops:
    print(f"  {loop.id} ({loop.polarity}): {' -> '.join(loop.variables)}")
    print(f"      avg |relative score| = {loop.average_importance():.3f}")
""")

code("""
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(9.5, 6.5), sharex=True,
                                gridspec_kw={"height_ratios": [1, 1.2]})

time = lrun.results.index
ax1.plot(time, lrun.results["population"], color="#1976d2", lw=2)
ax1.set_ylabel("population")
ax1.set_title("Behavior: S-shaped growth ...")

for loop, color in zip(lrun.loops, ["#2e7d32", "#c62828"]):
    ts = loop.behavior_time_series
    ax2.plot(time, np.abs(ts), color=color, lw=2,
             label=f"{loop.id} ({loop.polarity}): "
                   f"{'growth engine' if str(loop.polarity) == 'R' else 'capacity constraint'}")

ax2.axhline(0.5, color="gray", ls="--", lw=1, alpha=0.7)
ax2.text(0.5, 0.52, "dominance threshold", fontsize=8, color="gray")
ax2.set_xlabel("time")
ax2.set_ylabel("|relative loop score|")
ax2.set_title("... explained by shifting loop dominance")
ax2.legend(loc="center right")
fig.tight_layout()
""")

md("""
This is the canonical LTM result (Figure 3 of the 2020 paper, reproduced by Simlin's engine): the
reinforcing loop dominates while growth accelerates, the balancing loop takes over at the inflection
point, and the crossover is exactly where the S-curve bends. `run.dominant_periods` reads this off
automatically:
""")

code("""
for period in lrun.dominant_periods:
    loops = ", ".join(period.dominant_loops)
    print(f"  {period.start_time:6.1f} .. {period.end_time:6.1f}:  {loops} dominant")
""")

# ============================================================================
# Section 8: arms race
# ============================================================================

md("""
### 7b. The three-party arms race: loops you'd never find by hand

This model is *from* the LTM papers (Eberlein & Schoenberg 2020, "Finding the Loops that Matter"):
three countries, each adjusting its arms stock toward a target based on the other two. It looks
trivial -- 3 stocks, 12 variables -- but it contains 8 distinct feedback loops, including two
three-party loops (A->B->C->A and A->C->B->A) that are **not** in the model's "independent loop
set" and that static analysis methods miss. The paper's headline result is that those two loops are
precisely the ones that dominate long-run behavior.
""")

code("""
arms = simlin.load(REPO / "test" / "arms_race_3party" / "arms_race.stmx")
arun = arms.run()

print(f"ltm_mode: {arun.ltm_mode!r}, {len(arun.loops)} loops\\n")
loops_by_size = sorted(arun.loops, key=lambda l: (len(l.variables), l.id))
for loop in loops_by_size:
    n_stocks = sum(1 for v in loop.variables if v.endswith("_arms"))
    kind = {1: "self-adjustment", 2: "two-party", 3: "THREE-PARTY"}[n_stocks]
    print(f"  {loop.id} ({loop.polarity}, {kind}): {' -> '.join(loop.variables)}")
""")

code("""
fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(9.5, 7), sharex=True,
                                gridspec_kw={"height_ratios": [1, 1.4]})

time = arun.results.index
for var, label, color in [("a_s_arms", "A's arms", "#1976d2"),
                          ("b_s_arms", "B's arms", "#c62828"),
                          ("c_s_arms", "C's arms", "#2e7d32")]:
    ax1.plot(time, arun.results[var], label=label, color=color, lw=2)
ax1.legend(loc="upper left")
ax1.set_ylabel("arms")
ax1.set_title("Behavior: initial adjustment, then a runaway three-way arms race ...")

# Group: three-party loops (3 stocks) vs everything else
def n_stocks(loop):
    return sum(1 for v in loop.variables if v.endswith("_arms"))

for loop in arun.loops:
    ts = loop.behavior_time_series
    if ts is None:
        continue
    is_3party = n_stocks(loop) == 3
    ax2.plot(time, np.abs(ts),
             color="#c62828" if is_3party else "#90a4ae",
             lw=2.5 if is_3party else 1.2,
             label=(f"{loop.id}: three-party loop" if is_3party else None),
             zorder=3 if is_3party else 1)

ax2.plot([], [], color="#90a4ae", lw=1.2, label="self-adjustment & two-party loops")
ax2.axhline(0.5, color="gray", ls="--", lw=1, alpha=0.7)
ax2.set_xlabel("time (years)")
ax2.set_ylabel("|relative loop score|")
ax2.set_title("... driven, in the long run, by the two three-party loops")
ax2.legend(loc="center right", fontsize=9)
fig.tight_layout()
""")

md("""
Early on, the simple self-adjustment and two-party loops do the work (each country closing the gap
to its target). But every two-party loop here has gain <= 1 -- left to themselves they'd settle to
equilibrium. The runaway growth comes from the two long loops that route through *all three*
parties, and LTM picks them out cleanly: by the end of the run they account for essentially all of
the behavior.

For C-LEARN, the equivalent result would be: *which* of the eight climate loops dominates in which
decade, and when (under what emissions path) the reinforcing permafrost/ocean-weakening loops start
to outweigh the balancing uptake loops. That is the analysis this notebook is set up to run, as
soon as the engine can score pinned loops at this scale.
""")

# ============================================================================
# Section 9: experience report
# ============================================================================

md("""
## 8. Experience report

This notebook doubled as a stress test of the LTM implementation and the pysimlin API, across two
days and two waves of fixes. Verdict: **the method works, the engine's link-score layer now works
at C-LEARN scale, and the remaining gaps are specific and tracked.** Section 6 -- a quantified,
time-resolved decomposition of a real climate model's feedback structure -- was impossible when
this notebook was started: every LTM number on C-LEARN was silently corrupted memory.

### What worked well

* **Vensim import**: a 53,000-line production `.mdl` loads in 40 ms, simulates correctly
  (validated elsewhere against Vensim's own `Ref.vdf` output), renders its diagram, and exposes
  structure (links, polarities, equations) through a clean Python API.
* **The LTM link-score layer at scale**: after this week's O(N^2) compilation fixes, an
  LTM-instrumented C-LEARN run compiles and executes in ~8 seconds, and the per-link scores are
  physically meaningful (CO2 explains 56% of the change in radiative forcing at year 2000; the
  ocean-flux link is correctly negative; inactive permafrost links are correctly zero).
* **The LTM method itself**: composing those link scores into loop scores (section 6) produces a
  dominance analysis that matches climate science -- on a model 100x larger than anything in the
  LTM papers' examples.
* **Loop pinning workflow**: naming the loops you care about (rather than enumerating millions) is
  the right interaction model for big models, and the `model.edit()` / `set_loop_name()` API makes
  it natural.
* **Honest degradation**: every "can't" is now a clear warning or error with a suggested next
  step, instead of silence or garbage.

### Bugs found by this exercise (all fixed, now on `main`)

(In parallel, four LTM compilation fixes -- `2003b4bb`, `865854c0`, `ff3ef3c2`, `099c5659`, the
O(N^2)-blowup and ceteris-paribus correctness work -- landed on `main` between this notebook's
first and final drafts. Those are what took C-LEARN from "LTM cannot compile" to the working
analysis in section 6.)

| Commit | Severity | What |
|--------|----------|------|
| `fe2e619f` | **critical** | LTM instrumentation past 65,536 result slots silently wrapped 16-bit bytecode offsets -- overwriting `time` at slot 0 and corrupting **every** LTM result on C-LEARN-scale models. Now a fast, clear error. |
| `021dca87` | high | Loop pinning (`SetLoopName`) failed with "variable has no UID" on every Vensim/MDL-imported model -- i.e. on exactly the models pinning was built for. UIDs are now minted on demand. |
| `c93b82ef` | high | `Model.analyze(timeout=30)` ran unbounded (observed 9+ CPU-minutes) because the discovery budget was only checked *between* timesteps and a single C-LEARN timestep's search never finishes. The deadline is now enforced inside the search. |
| `3637eeec` | medium | `Model.get_links()` hard-coded every polarity to `?` -- the polarity analyzer was never called. |
| `b35f278b` | medium | `Model.run()` silently ignored its `time_range`/`dt` arguments; arrayed per-element equations were invisible; LTM failures crashed `run()` instead of degrading with a warning. |
| `a51e9191` | medium | Sector/group boxes (one per imported Vensim view) had no rule in the SVG renderers' stylesheets, so they took SVG's default **opaque black** fill and hid everything inside them -- every multi-view Vensim import rendered as a stack of black rectangles. |

### Issues filed for the remaining gaps

* [#653](https://github.com/bpowers/simlin/issues/653) -- **the gap that matters most now**:
  pinned loop-score equations on arrayed variables produce dimension mismatches and silently read
  as zero. Section 6's manual composition is the workaround; once fixed, that whole section becomes
  `run.loops` + `loop.behavior_time_series`.
* [#651](https://github.com/bpowers/simlin/issues/651) -- the MDL importer expands apply-to-all
  equations into N identical per-element copies: datamodel noise and the main multiplier behind
  LTM's element-level instrumentation size.
* [#652](https://github.com/bpowers/simlin/issues/652) -- raw link scores are useless for ranking
  (near-equilibrium denominators produce 1e22 magnitudes); the papers' *relative* link score
  should be exposed.
* [#647](https://github.com/bpowers/simlin/issues/647) (updated) -- strongest-path *discovery*
  remains infeasible on C-LEARN (a 60-second budget finds zero loops before truncating); the
  element-level granularity of the search is the structural problem. Pinning + composition (section
  6) is the practical alternative.

### API design observations

1. **The auto-flip needs to be visible at decision time, not discovery time.** `run.ltm_mode` (added
   in #648) plus the new warnings handle this -- but the deeper point is that an API whose default
   path costs 90x and returns nothing on big models needed to fail toward *cheap and explained*,
   not *expensive and silent*.
2. **Pinning is the right abstraction; finish it.** The pin workflow (name a loop -> engine always
   scores it) is exactly right for big models, and everything it needs is computed during the run.
   #653 is the last step: make the generated pin-score equations dimension-aware so section 6's
   numpy becomes one engine call.
3. **Score singularities need a first-class presentation answer.** LTM scores legitimately diverge
   when a stock's acceleration crosses zero (sec. 6 found C-LEARN's soil-carbon loop at +1,740 by
   2095 for exactly this reason). The papers handle it with relative scores and careful framing;
   an API should too -- e.g. flag near-singular intervals, or report median-windowed shares.
4. **Two-tier scoring (raw + relative) needs to be the API shape everywhere.** Loops already work
   this way (raw `loop_score` + relative score); links should too (#652). And the synthetic
   link-score variable names (`$:ltm:link_score:{from}->{to}`, with element-qualification rules)
   are currently undocumented internals that section 6 had to reverse-engineer -- either document
   them or, better, expose link/path scores through a real API.
5. **Vensim parity matters more than features.** Most real SD models that would benefit from LTM
   are Vensim models like C-LEARN. The importer fidelity issues (#651, the unit warnings, lookup
   handling #590) are not cosmetic -- they determine whether the analysis layer above gets usable
   structure.
""")

nb.cells = cells

# Write and execute
out_path = NOTEBOOKS_DIR / "clearn_ltm_experience.ipynb"
with open(out_path, "w") as f:
    nbf.write(nb, f)
print(f"wrote {out_path}")

# Execute in-place so output cells are populated
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
