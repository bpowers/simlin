# MDL Roundtrip Fidelity -- Human Test Plan

## Prerequisites

- Vensim (any edition, including PLE) installed and available
- simlin-engine builds successfully: `cargo build -p simlin-engine`
- All automated tests pass: `cargo test -p simlin-engine --test mdl_roundtrip`
- The original model file exists: `test/bobby/vdf/econ/mark2.mdl`
- `simlin-cli` builds: `cargo build -p simlin-cli`

## Phase 1: Generate the roundtripped MDL file

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Run `cargo run -p simlin-cli -- convert --to mdl test/bobby/vdf/econ/mark2.mdl -o /tmp/roundtrip_mark2.mdl` | Command completes without errors. File `/tmp/roundtrip_mark2.mdl` is created. |
| 1.2 | If simlin-cli does not support `convert --to mdl`, run `cargo test -p simlin-engine --test mdl_roundtrip write_mdl_for_vensim_validation -- --ignored --nocapture` and note the output directory. | Test prints an output directory path. Note: you may need to add mark2 to the test's `models_to_export` list. |
| 1.3 | Alternative: add mark2.mdl to `models_to_export` in `write_mdl_for_vensim_validation`, rebuild, and re-run the ignored test. | Roundtripped file is produced at the temp directory path printed to stderr. |

## Phase 2: Open in Vensim and verify view structure

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Open `/tmp/roundtrip_mark2.mdl` in Vensim. | File opens without errors or warnings. No parse errors displayed. |
| 2.2 | Navigate to the view selector (bottom-left in Vensim). | Exactly 2 views are listed: one containing "1 housing" and one containing "2 investments". |
| 2.3 | Select the "1 housing" view. | Diagram renders with stocks, flows, auxiliaries, clouds, and connectors. No blank/empty view. |
| 2.4 | Select the "2 investments" view. | Diagram renders with its own set of elements. No blank/empty view. |

## Phase 3: Visual comparison of element layout

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | Open the original `test/bobby/vdf/econ/mark2.mdl` in a separate Vensim instance. | Both original and roundtripped files are open side by side. |
| 3.2 | Compare the "1 housing" view. Check: element positions, stock box sizes, flow valve placement, auxiliary variable positions. | Elements appear in approximately the same positions. Stock boxes should have the same relative size. Flow arrows connect the same stocks. |
| 3.3 | Compare the "2 investments" view in the same manner. | Same visual layout as original. |
| 3.4 | Check connector routing (influence arrows). Verify they connect the correct variables and bend points look reasonable. | Connectors link the correct source/target pairs. Angular routing may differ slightly but connections are correct. |
| 3.5 | Check that label text is readable and not overlapping other elements. | Variable names are legible, not clipped, and do not obscure other diagram elements. |
| 3.6 | Check the font rendering: text should appear in Verdana 10pt (the original font). | Text in the diagram uses Verdana, approximately 10-point size. |

## Phase 4: Verify simulation equivalence

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | In Vensim with the roundtripped file open, click "Simulate". | Simulation runs to completion without errors. |
| 4.2 | Plot key variables: "New Homes On Market", "Endogenous Federal Funds Rate", "inflation rate". | Plots show curves that match the expected economic dynamics (no flat lines, no NaN/infinity). |
| 4.3 | In the original file, run the same simulation and plot the same variables. | Identical or nearly identical curves in both files. |
| 4.4 | Use Vensim's "Compare" or "SyntheSim" to overlay results from both runs. | Time series match exactly (or within floating-point tolerance of ~1e-10). |

## Phase 5: Edge case visual checks

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | In the roundtripped file, check for any Vensim warnings about undefined variables, missing equations, or broken links. | No warnings. The model should be in a fully valid state. |
| 5.2 | Right-click a variable with mixed-case naming (e.g., "Endogenous Federal Funds Rate") and select "Equation". | The equation editor shows the variable with its original casing, and the equation text is valid. |
| 5.3 | Check a lookup variable (e.g., "federal funds rate lookup") -- right-click and view its lookup table. | The lookup table has correct range bounds (e.g., [(0,0)-(300,10)]) and data points. The graphical function plot matches the original. |
| 5.4 | Verify that a variable referencing the lookup uses native call syntax: open "historical federal funds rate" equation. | Equation reads like `federal funds rate lookup ( Time )`, NOT `LOOKUP(federal funds rate lookup, Time)`. |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 -- 2 views with correct names | `split_view_two_groups_produces_two_segments`, `multi_view_mdl_output_contains_view_headers`, `mdl_format_roundtrip` | Phase 2, steps 2.1-2.4 |
| AC1.2 -- correct elements per view | `split_view_elements_partitioned_correctly`, `mdl_format_roundtrip` | Phase 3, steps 3.2-3.3 |
| AC1.3 -- single-view models | `split_view_no_groups_returns_single_segment`, `single_view_no_groups_mdl_output` | N/A (mark2 is multi-view) |
| AC1.4 -- font spec per view | `multi_view_uses_font_when_present`, `test_font_flows_to_stock_flow`, `mdl_format_roundtrip` | Phase 3, step 3.6 |
| AC2.1 -- stock dimensions preserved | `test_stock_compat_preserves_dimensions`, `stock_compat_dimensions_emitted` | Phase 3, step 3.2 |
| AC2.2 -- aux/flow/cloud/alias dimensions | parse + write tests in convert.rs and writer.rs | Phase 3, steps 3.2-3.4 |
| AC2.3 -- defaults without compat | `*_default_dimensions_without_compat` (5 tests) | N/A (mark2 has compat data) |
| AC3.1 -- lookup call syntax | `lookup_call_native_vensim_syntax`, `mdl_format_roundtrip` | Phase 5, step 5.4 |
| AC3.2 -- explicit lookup bounds | `test_graphical_function_y_scale_from_explicit_range`, `mdl_format_roundtrip` | Phase 5, step 5.3 |
| AC3.3 -- computed lookup bounds | `test_graphical_function_y_scale_computed_from_data_when_no_explicit_range` | N/A (mark2 uses explicit bounds) |
| AC4.1 -- inline format | `short_equation_uses_inline_format`, `mdl_format_roundtrip` | Phase 5, step 5.2 |
| AC4.2 -- multiline continuations | `long_equation_uses_multiline_format`, `wrap_long_equation_with_continuations` | N/A (format not visible in Vensim) |
| AC4.3 -- variable casing | `equation_lhs_uses_view_element_casing`, `mdl_format_roundtrip` | Phase 5, step 5.2 |
| AC4.4 -- alphabetical ungrouped order | `ungrouped_variables_sorted_alphabetically` | N/A (internal ordering) |
| AC4.5 -- group-based ordering | `grouped_variables_retain_group_order` | N/A (internal ordering) |
| AC5.1 -- test registered/runnable | Cargo auto-discovery verified | N/A |
| AC5.2 -- format roundtrip test | `mdl_format_roundtrip` | N/A |
| AC5.3 -- regression tests pass | Runtime verification (5 pass, 0 fail) | N/A |
| Design DoD #4 -- Vensim visual compatibility | None (requires Vensim) | Phases 2-5 |
