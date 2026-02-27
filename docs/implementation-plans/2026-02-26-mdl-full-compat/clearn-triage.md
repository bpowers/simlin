# C-LEARN Equivalence Triage

**Date:** 2026-02-26
**Baseline:** 51 diffs after Tasks 1-6 (Subcomponents A + B)

## Diff Categories

### 1. :NA: vs NAN normalization (38 diffs)

xmutil preserves the MDL `:NA:` literal through its XMILE output, while the
native parser correctly converts `Expr::Na` to `NAN`. After lowercasing in
`normalize_expr`, this appears as `:na:` vs `nan`.

**Variables affected:** annual_change_of_co2_emissions, annual_rate_of_emissions_to_target,
co2_emissions_at_y2009, delta_ph_from_2000, global_annual_rate_of_co2eq_2050_to_2100,
global_annual_rate_of_co2eq_peak_to_2050, global_annual_rate_of_co2eq_to_target,
historical_gdp, im_1_emissions_vs_rs, intensity_ref_constrained_emissions,
intensity_ref_target, intensity_ref_trajectory, intensity_rs_constrained_emissions,
intensity_rs_target, intensity_rs_trajectory, last_active_target_year,
last_set_target_year, level_co2_ff_below_2005_by_2050,
max_probabilities_of_exceeding_2_deg_c, min_probabilities_of_exceeding_2_deg_c,
per_capita_ref_constrained_emissions, per_capita_ref_target,
per_capita_ref_trajectory, per_capita_rs_constrained_emissions,
per_capita_rs_target, per_capita_rs_trajectory,
percent_co2eq_below_2005_levels_by_2050, refyr_constrained_emissions,
refyr_target, refyr_trajectory, refyr_trajectory_if_linear_or_exp,
refyr_trajectory_if_s_shape, rs_constrained_emissions, rs_target, rs_trajectory,
sea_level_rise_from_2000, slr_feet_from_2000, slr_inches_from_2000,
target_type (Arrayed, 7 elements with :na:), target_value (Arrayed, 7 elements),
temperature_change_from_1990

**Fix:** Normalize `:na:` to `nan` in `normalize_expr()` in the equivalence test.
These are semantically identical -- the xmutil C++ library just does not convert
the MDL-specific `:NA:` syntax when producing XMILE.

### 2. Dimension maps_to target name (2 diffs)

The `maps_to` field for dimension aliases differs:
- `aggregated_regions`: xmutil has `maps_to: Some("cop_developed")`, native has `Some("cop")`
- `semi_agg`: xmutil has `maps_to: Some("oecd_us")`, native has `Some("cop")`

xmutil maps to the first element's parent dimension, while native maps to the
canonical "cop" dimension (which is the actual target the equivalence references).

**Fix:** In `build_dimensions()`, when building an equivalence alias, use the
mapped-to dimension's first element's parent dimension name to match xmutil
behavior. Or normalize maps_to in the equivalence test to resolve both to
the same canonical dimension.

### 3. Middle-dot Unicode (2 diffs -- 1 var name mismatch)

Variable `goal_1.5_for_temperature` (xmutil) vs `goal_1\u00B75_for_temperature` (native).
xmutil normalizes middle-dot (U+00B7) to period, native preserves it.

**Fix:** Normalize middle-dot to period in `space_to_underbar()` or in the lexer's
symbol function.

### 4. Net flow synthesis -- c_in_atmosphere (4 diffs)

xmutil synthesizes a `c_in_atmosphere_net_flow` variable and uses it as the sole
inflow to `c_in_atmosphere`. The native parser decomposes the stock's rate
equation into individual flows (6 inflows, 3 outflows).

Additionally, `flux_c_from_permafrost_release` is typed as Aux by xmutil but
Flow by native.

**Fix:** This is a difference in how net flow synthesis decides whether to create
a synthetic flow vs decompose the rate expression. The native approach is more
granular (and arguably more correct for the model structure). Need to match
xmutil behavior for equivalence or accept the structural difference.

### 5. Net flow equation subscript resolution (2 diffs)

`c_in_deep_ocean_net_flow` and `heat_in_deep_ocean_net_flow` differ in
subscript handling within their Arrayed equations. xmutil fully resolves
subscripts to element names (e.g., `diffusion_flux[deterministic, layer1]`),
while native uses dimension subscript names (e.g., `diffusion_flux[scenario, upper]`)
with aliased element references.

**Fix:** The net flow synthesis code needs to substitute concrete element names
when expanding per-element equations instead of using abstract subscript
references. This is in `link_stocks_and_flows()` in `stocks.rs`.

## Fix Priority

1. **:NA: normalization** -- 38 diffs, simple test normalization fix
2. **Middle-dot** -- 2 diffs, simple character normalization
3. **Dimension maps_to** -- 2 diffs, dimension alias target resolution
4. **Net flow synthesis** -- 4 diffs, structural difference in flow decomposition
5. **Net flow subscript resolution** -- 2 diffs, subscript expansion in net flow equations

## Post-fix Expected Count

After fixing categories 1-3 (normalization issues): ~9 diffs remaining
After fixing all categories: 0 diffs
