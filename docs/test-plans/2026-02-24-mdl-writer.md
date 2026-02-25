# MDL Writer -- Human Test Plan

## Prerequisites
- Rust toolchain installed and working (`rustup`, `cargo`)
- Run `./scripts/dev-init.sh` from the project root
- All automated tests passing: `cargo test -p simlin-engine --test mdl_roundtrip` and `cargo test -p simlin-engine mdl::writer::tests`
- Vensim Model Reader or Vensim DSS installed (Windows or Wine) for AC6.1 verification

## Phase 1: Generate MDL files for Vensim validation

| Step | Action | Expected |
|------|--------|----------|
| 1 | From `/Users/bpowers/src/simlin`, run `cargo test -p simlin-engine --test mdl_roundtrip -- --ignored --nocapture` | Test outputs MDL files to a temp directory. Console prints paths like `/tmp/simlin-mdl-roundtrip/roundtrip_test_abs.mdl`. |
| 2 | Record the output directory path printed at the end of the test output. | Path like `/tmp/simlin-mdl-roundtrip/` or OS-specific temp location. |
| 3 | Copy the generated `.mdl` files to a machine with Vensim installed (or use Wine). | Files transfer without corruption; each file is non-empty text. |

## Phase 2: Vensim open and parse validation (AC6.1)

| Step | Action | Expected |
|------|--------|----------|
| 4 | Open `roundtrip_test_abs.mdl` in Vensim. | File opens without parse errors or warning dialogs. Model structure visible in the diagram. |
| 5 | In the same model, click "SyntheSim" or "Run" to simulate. | Simulation completes without equation errors. Time axis shows values from INITIAL TIME to FINAL TIME. |
| 6 | Open `roundtrip_test_smooth.mdl` in Vensim. | File opens without errors. Model contains stock-and-flow structure with SMOOTH builtin. Diagram displays variables, flows, and connectors. |
| 7 | Run the simulation for the smooth model. | Simulation completes. Output variables show smooth dynamics (no discontinuities or NaN values). |
| 8 | Open `roundtrip_test_forecast.mdl` in Vensim. | File opens. Model contains FORECAST function usage. Diagram renders. |
| 9 | Run the forecast model simulation. | Simulation produces results. FORECAST variable shows expected trend extrapolation behavior. |
| 10 | Open `roundtrip_SIR.mdl` in Vensim. | File opens. Diagram shows classic SIR structure: Susceptible, Infected, Recovered stocks with infection rate and recovery rate flows. All connectors visible and readable. |
| 11 | Run the SIR model simulation. | Simulation completes. Susceptible decreases, Infected peaks then decreases, Recovered increases -- the classic epidemic curve shape. |

## Phase 3: Diagram layout and visual inspection (AC6.1)

| Step | Action | Expected |
|------|--------|----------|
| 12 | For each opened model, visually inspect the diagram layout. | Variables are positioned in readable locations (not overlapping or off-screen). Stock shapes are boxes, flow valves are visible, connectors connect the correct variables. |
| 13 | Check that variable names in the diagram match the original model. | Names like "Susceptible", "Infected", "Teacup Temperature" appear correctly with spaces (not underscores). No truncation or encoding artifacts. |
| 14 | Check connector polarity markings (if present in the model). | Positive links show "+" or "S", negative links show "-" or "O", matching the original model's polarity conventions. |
| 15 | Right-click a variable and choose "Equation" (or double-click). | The equation editor shows the variable's equation in correct Vensim syntax. Units are populated. Documentation text appears in the comment field. |

## Phase 4: Cross-format spot check (XMILE origin)

| Step | Action | Expected |
|------|--------|----------|
| 16 | Write a small script or use simlin-cli to convert `test/test-models/samples/teacup/teacup.xmile` to MDL format. | An MDL file is produced. No errors printed to stderr. |
| 17 | Open the converted file in Vensim. | File opens. The teacup model structure is present: Teacup Temperature stock, Heat Loss to Room flow, Room Temperature and Characteristic Time auxiliaries. |
| 18 | Run the teacup model simulation. | Simulation shows exponential cooling curve: Teacup Temperature starts at 180 and decays toward Room Temperature (~70). |

## Phase 5: Full roundtrip fidelity check

| Step | Action | Expected |
|------|--------|----------|
| 19 | Open the original `test/test-models/samples/SIR/SIR.mdl` in Vensim. Note the simulation output values at t=0, t=5, t=10 for Susceptible, Infected, Recovered. | Values recorded as baseline. |
| 20 | Open `roundtrip_SIR.mdl` (from step 10) in Vensim. Check the same output values at t=0, t=5, t=10. | Values match the baseline exactly (or within floating-point display precision). The roundtrip preserved equation semantics. |
| 21 | Compare the diagram layouts side by side (original vs roundtrip). | Variable positions are in approximately the same locations. No variables are missing or added. Connector topology is identical. |

## Human Verification Required

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| AC6.1 -- Output opens in Vensim | Vensim is proprietary Windows GUI software that cannot be automated in CI. Visual inspection is required to confirm diagrams render correctly and simulations produce results. | Steps 4-21 above. At minimum: one scalar model (test_abs), one stock-and-flow model (SIR), one model with builtins (smooth, forecast). Inspect diagrams, run simulations, verify equations. |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 -- Complete MDL file | `equations_section_full_assembly`, `full_assembly_has_all_three_sections`, `settings_section_*`, `mdl_to_mdl_roundtrip` | -- |
| AC1.2 -- compat wrapper | `compat_to_mdl_matches_project_to_mdl` | -- |
| AC1.3 -- Error on multiple models | `project_to_mdl_rejects_multiple_models` | -- |
| AC1.4 -- Error on Module variables | `project_to_mdl_rejects_module_variable` | -- |
| AC2.1 -- Basic expressions | `constants`, `nan_constant`, `variable_references`, `arithmetic_operators`, `precedence_*`, `unary_operators` | -- |
| AC2.2 -- Function renames | `function_rename_*`, `logical_operators_and`, `logical_operators_or`, `function_unknown_uppercased` | -- |
| AC2.3 -- Argument reorders | `arg_reorder_delay_n`, `arg_reorder_smooth_n`, `arg_reorder_random_normal` | -- |
| AC2.4 -- Structural expansions | `pattern_pulse`, `pattern_pulse_train`, `pattern_quantum`, `pattern_sample_if_true`, `pattern_log_2arg`, `pattern_random_0_1`, `pattern_time_base`, `pattern_random_poisson`, `pattern_allocate_by_priority` | -- |
| AC2.5 -- Unrecognized fall through | `pattern_fallthrough_no_match`, `pattern_pulse_not_matched_missing_lt` | -- |
| AC3.1 -- All equation types | `scalar_aux_entry`, `scalar_stock_integ`, `scalar_aux_with_lookup`, `apply_to_all_*`, `arrayed_*`, `dimension_def_*`, `data_equation_*` | -- |
| AC3.2 -- Subscript notation | `arrayed_subscript_names_with_underscores`, `apply_to_all_entry` | -- |
| AC3.3 -- Sim spec variables | `sim_specs_emission`, `sim_specs_saveper_*`, `sim_specs_reciprocal_dt` | -- |
| AC4.1 -- MDL->MDL roundtrip | `mdl_to_mdl_roundtrip` (42 test files) | -- |
| AC4.2 -- XMILE->MDL roundtrip | `xmile_to_mdl_roundtrip` (18 test files) | -- |
| AC4.3 -- Semantics preserved | Implicit via AC4.1 + AC4.2 semantic comparison | -- |
| AC5.1 -- All sketch element types | `sketch_aux_element`, `sketch_stock_element`, `sketch_flow_*`, `sketch_cloud_*`, `sketch_alias_*`, `sketch_link_*` | -- |
| AC5.2 -- Sketch text roundtrip | `sketch_section_structure`, `sketch_roundtrip_teacup`, `view_element_roundtrip` | -- |
| AC6.1 -- Output opens in Vensim | `write_mdl_for_vensim_validation` (generates files) | Steps 4-21 |
