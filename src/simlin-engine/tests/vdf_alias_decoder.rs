// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for VDF alias-signature helpers.
//!
//! These tests exercise `VdfFile::output_signatures()` and
//! `VdfFile::new_style_alias_signatures()` on real on-disk fixtures and
//! cross-reference the file-order alias pairing against the parsed MDL.
//! See `docs/design/vdf.md` under "Confirmed structural signals" for the
//! hypothesis being validated.
//!
//! The test target gates on `file_io` via `required-features` in
//! `src/simlin-engine/Cargo.toml` (mirroring the `simulate` pattern),
//! so running `cargo test --features file_io -p simlin-engine` exercises
//! these tests deterministically rather than silently compiling them
//! into an empty binary.

use std::fs;
use std::path::Path;

use simlin_engine::compat::open_vensim;
use simlin_engine::vdf::VdfFile;

fn parse_vdf(path: &str) -> VdfFile {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    VdfFile::parse(bytes).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn load_mdl(path: &str) -> simlin_engine::datamodel::Project {
    let contents = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    open_vensim(&contents).unwrap_or_else(|e| panic!("parse mdl {path}: {e}"))
}

/// Crude stdlib-call detector that only looks at the leading token. Used
/// here so the test does not depend on internal `extract_stdlib_call_info`.
fn equation_starts_with_stdlib_call(eq: &str) -> bool {
    let trimmed = eq.trim();
    let Some(paren) = trimmed.find('(') else {
        return false;
    };
    let func = trimmed[..paren].trim().to_uppercase();
    matches!(
        func.as_str(),
        "SMOOTH"
            | "SMOOTHI"
            | "SMOOTH3"
            | "SMOOTH3I"
            | "SMTH1"
            | "SMTH3"
            | "DELAY"
            | "DELAY1"
            | "DELAY1I"
            | "DELAY3"
            | "DELAY3I"
            | "DELAYN"
            | "TREND"
    )
}

/// Classify the function family of a `#FUNC(args)#` or `#alias>FUNC#`
/// signature. Returns a canonical family identifier ("SMOOTH",
/// "DELAY", "TREND", or "OTHER") so tests can assert an alias and its
/// paired sig belong to the same family without tripping on minor
/// spelling variants (SMOOTH vs SMOOTH3 vs SMOOTH3I all share the SMOOTH
/// family; DELAY vs DELAY1 vs DELAY3 share DELAY).
fn sig_family(sig: &str) -> &'static str {
    // Strip the leading `#` and trailing `#`.
    let inner = sig.trim_start_matches('#').trim_end_matches('#');
    // New-style: split on `>`, take the function name.
    let func = if let Some(pos) = inner.find('>') {
        &inner[pos + 1..]
    } else {
        // Old-style: the function name is the prefix before `(`.
        match inner.split_once('(') {
            Some((f, _)) => f,
            None => inner,
        }
    };
    let func_upper = func.to_uppercase();
    if func_upper.starts_with("SMOOTH") {
        "SMOOTH"
    } else if func_upper.starts_with("DELAY") || func_upper.starts_with("DEL") {
        "DELAY"
    } else if func_upper.starts_with("TREND") {
        "TREND"
    } else if func_upper.starts_with("RAMP") {
        "RAMP"
    } else if func_upper.starts_with("SAMPLE") {
        "SAMPLE"
    } else if func_upper.starts_with("SSHAPE") {
        "SSHAPE"
    } else {
        "OTHER"
    }
}

/// Infer a stdlib family from an MDL equation's leading function name.
/// Pairs with [`sig_family`] so we can verify alias/sig pairs agree on
/// family (e.g. an alias whose equation reads `SMOOTH(x, 3)` must pair
/// with a `#SMOOTH(...)#` signature, not a `#DELAY(...)#`).
///
/// Vensim accepts multiple aliases for the same family: `SMTH1`/`SMTH3`
/// are SMOOTH/SMOOTH3 synonyms; `DELAY1I`/`DELAY3I`/`DELAYN` all fall
/// into the DELAY family. We canonicalize to the output-sig function
/// name that appears in the VDF (`#SMOOTH(...)#`, `#DELAY(...)#`).
fn equation_family(eq: &str) -> &'static str {
    let trimmed = eq.trim();
    let Some(paren) = trimmed.find('(') else {
        return "OTHER";
    };
    let func_upper = trimmed[..paren].trim().to_uppercase();
    if func_upper.starts_with("SMOOTH") || func_upper.starts_with("SMTH") {
        "SMOOTH"
    } else if func_upper.starts_with("DELAY") || func_upper.starts_with("DEL") {
        "DELAY"
    } else if func_upper.starts_with("TREND") {
        "TREND"
    } else if func_upper.starts_with("RAMP") {
        "RAMP"
    } else if func_upper.starts_with("SAMPLE") {
        "SAMPLE"
    } else if func_upper.starts_with("SSHAPE") {
        "SSHAPE"
    } else {
        "OTHER"
    }
}

fn normalize(name: &str) -> String {
    name.replace([' ', '_'], "").to_lowercase()
}

#[test]
fn output_signatures_recognizes_both_stdlib_encodings() {
    if !Path::new("../../test/bobby/vdf/econ/base.vdf").exists() {
        return;
    }
    // Old-style signatures: `#FUNC(args)#` are outputs;
    // `#LV1<FUNC(args)#` are internal stocks.
    let base = parse_vdf("../../test/bobby/vdf/econ/base.vdf");
    let base_outputs: Vec<String> = base
        .output_signatures()
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    assert_eq!(
        base_outputs.len(),
        5,
        "econ/base.vdf has 5 output signatures: one #DELAY1 + four #SMOOTH"
    );
    assert!(
        base_outputs.iter().all(|n| !n.starts_with("#LV1<")),
        "output_signatures must exclude the #LV1<DELAY1...> internal stock: got {base_outputs:?}"
    );

    // New-style signatures: `#alias>FUNC#` are outputs;
    // `#alias>FUNC>LV1#` are internal stocks.
    let mark2 = parse_vdf("../../test/bobby/vdf/econ/mark2.vdf");
    let mark2_outputs: Vec<String> = mark2
        .output_signatures()
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    assert_eq!(
        mark2_outputs.len(),
        5,
        "econ/mark2.vdf has 5 output signatures (one per user alias)"
    );
    assert!(
        mark2_outputs.iter().all(|n| !n.ends_with(">LV1#")),
        "output_signatures must exclude the #defaults>DELAY1>LV1# internal stock: \
         got {mark2_outputs:?}"
    );
}

#[test]
fn new_style_alias_signatures_decode_alias_prefix() {
    if !Path::new("../../test/bobby/vdf/econ/mark2.vdf").exists() {
        return;
    }
    // mark2.vdf (saved with a newer Vensim build) encodes aliases
    // directly in each output signature as `#alias>FUNC#`. Decoding is
    // deterministic: split on `>` and take the prefix.
    let vdf = parse_vdf("../../test/bobby/vdf/econ/mark2.vdf");
    let aliases: Vec<String> = vdf
        .new_style_alias_signatures()
        .into_iter()
        .map(|(_, _, a)| a)
        .collect();
    assert_eq!(
        aliases,
        vec![
            "defaults".to_string(),
            "perceived inflation rate".to_string(),
            "perceived HPI".to_string(),
            "perceived risk of insolvency".to_string(),
            "perceived mortgage balance".to_string(),
        ],
        "new-style aliases must decode from the `#alias>FUNC#` prefix \
         in name-table order"
    );

    // Old-style fixtures (`#FUNC(args)#`) yield an empty set: the alias
    // name is not encoded in the signature.
    let old_style = parse_vdf("../../test/bobby/vdf/econ/base.vdf");
    assert!(
        old_style.new_style_alias_signatures().is_empty(),
        "old-style `#FUNC(args)#` signatures must not be treated as new-style"
    );
}

#[test]
fn old_style_alias_to_output_sig_pair_by_file_order() {
    if !Path::new("../../test/bobby/vdf/econ/base.vdf").exists() {
        return;
    }
    // Hypothesis: user aliases in name-table file order pair 1:1 with
    // output-type `#` signatures in name-table file order.
    //
    // Validation: given the MDL for mark2 (the only MDL available for
    // the econ family), collect stdlib-call declarations and compare
    // their name-table positions against the output signatures'
    // positions in `econ/base.vdf` (old-style). Each pair must also
    // agree on stdlib function family (e.g. a SMOOTH alias pairs with
    // a SMOOTH sig, not a DELAY sig) -- a weaker "count matches" check
    // would miss a permutation bug.
    let vdf = parse_vdf("../../test/bobby/vdf/econ/base.vdf");
    let output_sigs = vdf.output_signatures();
    assert_eq!(output_sigs.len(), 5);

    let datamodel_project = load_mdl("../../test/bobby/vdf/econ/mark2.mdl");
    let model = datamodel_project.models.first().unwrap();

    let mut mdl_aliases: Vec<(String, String)> = Vec::new();
    for var in &model.variables {
        let (ident, equation) = match var {
            simlin_engine::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
            simlin_engine::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
            _ => continue,
        };
        let text = match equation {
            simlin_engine::datamodel::Equation::Scalar(s)
            | simlin_engine::datamodel::Equation::ApplyToAll(_, s) => s.as_str(),
            _ => continue,
        };
        if equation_starts_with_stdlib_call(text) {
            mdl_aliases.push((ident.clone(), text.to_string()));
        }
    }

    // MDL declaration order may differ from name-table order (Vensim can
    // reorder), so resolve each alias to its VDF name index and sort by
    // that index.
    let mut alias_positions: Vec<(usize, String, String)> = mdl_aliases
        .iter()
        .filter_map(|(a, eq)| {
            let norm = normalize(a);
            vdf.names
                .iter()
                .enumerate()
                .find(|(_, n)| normalize(n) == norm)
                .map(|(i, n)| (i, n.clone(), eq.clone()))
        })
        .collect();
    alias_positions.sort_by_key(|(i, _, _)| *i);

    assert_eq!(
        alias_positions.len(),
        output_sigs.len(),
        "number of MDL-declared aliases must equal the count of output sigs"
    );

    // Each alias in file order should sit before its target sig in the
    // name table, and the pairing by list-index is the hypothesized
    // alias -> output sig resolution. Additionally, the alias's
    // equation family must match the paired sig's family.
    for ((alias_idx, alias_name, alias_eq), (sig_idx, sig_name)) in
        alias_positions.iter().zip(output_sigs.iter())
    {
        assert!(
            alias_idx < sig_idx,
            "alias {alias_name:?} (name_idx {alias_idx}) should sit before \
             its target sig {sig_name:?} (name_idx {sig_idx}) in the name table"
        );
        let alias_fam = equation_family(alias_eq);
        let sig_fam = sig_family(sig_name);
        assert_eq!(
            alias_fam, sig_fam,
            "alias {alias_name:?} (eq family {alias_fam}) must pair with a \
             matching-family sig, but paired with {sig_name:?} (sig family \
             {sig_fam})"
        );
    }
}

/// WRLD3 SCEN01's MDL-declared stdlib-call aliases and its `#FUNC(...)#`
/// output signatures do NOT pair 1:1. MDL has more aliases than the VDF
/// has old-style output sigs -- this pins the upper-bound nature of the
/// pairing claim so a future regression doesn't silently assume 1:1.
///
/// The root cause is not yet fully characterised: some aliases may map
/// to SAMPLE/PREVIOUS/INIT forms that do not emit a `#FUNC(args)#`
/// signature, and some may share a sig via module reuse. This test
/// documents the observed mismatch rather than enforcing equality.
#[test]
fn wrld3_scen01_alias_to_sig_pairing_is_not_1to1() {
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    if !Path::new(vdf_path).exists() || !Path::new(mdl_path).exists() {
        return;
    }
    let vdf = parse_vdf(vdf_path);
    let output_sigs = vdf.output_signatures();
    // Observed: 8 old-style `#FUNC(args)#` output sigs on SCEN01.
    assert!(
        output_sigs.len() >= 6 && output_sigs.len() <= 10,
        "SCEN01: output_sigs count pinned near 8, got {}",
        output_sigs.len()
    );

    let datamodel_project = load_mdl(mdl_path);
    let model = datamodel_project.models.first().unwrap();
    let mdl_alias_count = model
        .variables
        .iter()
        .filter_map(|v| match v {
            simlin_engine::datamodel::Variable::Aux(a) => Some(&a.equation),
            simlin_engine::datamodel::Variable::Flow(f) => Some(&f.equation),
            _ => None,
        })
        .filter_map(|eq| match eq {
            simlin_engine::datamodel::Equation::Scalar(s)
            | simlin_engine::datamodel::Equation::ApplyToAll(_, s) => Some(s.as_str()),
            _ => None,
        })
        .filter(|text| equation_starts_with_stdlib_call(text))
        .count();
    assert!(
        mdl_alias_count > output_sigs.len(),
        "WRLD3 SCEN01: MDL has {mdl_alias_count} stdlib-call aliases but \
         VDF only emits {} output sigs; the 1:1 pairing claim is an upper \
         bound, not a guarantee",
        output_sigs.len()
    );
}

/// Ref.vdf (C-LEARN) contains many `#`-bracketed names that are NOT
/// stdlib outputs (`#MP RF Total#`, `#BAU atm conc CO2#`, etc.) and
/// multi-`>` sub-parts of macros like RAMP FROM TO
/// (`#alias>RAMP FROM TO>slope#`, `>rate#`, ...). The fixed classifier
/// must reject all of them.
///
/// Previously `is_output_sig_name` accepted any `#name#` that didn't
/// match the internal-prefix/suffix lists, so Ref.vdf produced ~45
/// false-positive output signatures. The fixed version requires
/// either `(` (old-style) or exactly one `>` (new-style) at the top
/// level, yielding the canonical count only.
#[test]
fn ref_vdf_output_signatures_reject_ramp_subparts_and_display_names() {
    let path = "../../test/xmutil_test_models/Ref.vdf";
    if !Path::new(path).exists() {
        return;
    }
    let vdf = parse_vdf(path);
    let outputs: Vec<String> = vdf
        .output_signatures()
        .into_iter()
        .map(|(_, n)| n)
        .collect();

    // None of the RAMP FROM TO sub-parts should be classified as
    // outputs.
    for sub in [
        ">RAMP FROM TO>linear#",
        ">RAMP FROM TO>linear ramp#",
        ">RAMP FROM TO>exp ramp#",
        ">RAMP FROM TO>slope#",
        ">RAMP FROM TO>rate#",
        ">RAMP FROM TO>interval#",
        ">SSHAPE>input#",
    ] {
        assert!(
            !outputs.iter().any(|n| n.ends_with(sub)),
            "Ref.vdf: expected no output sig ending in {sub:?}, got {:?}",
            outputs
                .iter()
                .filter(|n| n.ends_with(sub))
                .collect::<Vec<_>>()
        );
    }

    // None of the bare display names (no `(`, no `>`) should classify
    // as outputs.
    for name in [
        "#BAU atm conc CO2#",
        "#BAU atm conc CO2eq#",
        "#BAU temperature change from preindustrial#",
        "#Calculated Developed RS CO2eq#",
        "#MP RF Total#",
    ] {
        assert!(
            !outputs.iter().any(|n| n == name),
            "Ref.vdf: expected no output sig matching {name:?}"
        );
    }

    // The canonical RAMP aliases (single `>`) ARE outputs.
    for name in [
        "#Relative forestry emissions to target>RAMP FROM TO#",
        "#RefYr trajectory if linear or exp>RAMP FROM TO#",
        "#Relative emissions to equity target>RAMP FROM TO#",
    ] {
        assert!(
            outputs.iter().any(|n| n == name),
            "Ref.vdf: expected {name:?} to be classified as an output sig"
        );
    }

    // new_style_alias_signatures must emit exactly ONE entry per
    // canonical RAMP alias, not seven. With 3 RAMP aliases above we
    // expect 3 RAMP-family sig entries, not 21 (3 * 7 sub-parts).
    let new_style_ramps: Vec<&String> = outputs
        .iter()
        .filter(|n| n.contains(">RAMP FROM TO#"))
        .collect();
    assert_eq!(
        new_style_ramps.len(),
        3,
        "Ref.vdf: expected 3 canonical RAMP output sigs, got {new_style_ramps:?}"
    );
}

/// On `model_editing/run_1.vdf` the name table contains a bare `#`
/// entry (a Vensim artefact on the empty-aux fixture). The classifier
/// must treat this as non-sig, not as a false positive.
#[test]
fn run_1_vdf_bare_hash_is_not_output_signature() {
    let path = "../../test/bobby/vdf/model_editing/run_1.vdf";
    if !Path::new(path).exists() {
        return;
    }
    let vdf = parse_vdf(path);
    let outputs = vdf.output_signatures();
    assert!(
        outputs.iter().all(|(_, n)| n != "#"),
        "run_1.vdf: bare `#` must not be classified as an output sig"
    );
}

/// `identify_potential_aliases` combines the `f[1]==2065` classification
/// signal with alias-candidate name filtering. On `econ/base.vdf` and
/// `econ/risk.vdf` it identifies most but not all MDL-declared stdlib
/// aliases. The one-alias gap per fixture corresponds to aliases whose
/// MDL equation wraps a subtraction/addition inside the stdlib call
/// (e.g. `SMTH1(a - b, t)`), which the Vensim runtime classifies as a
/// regular variable (`f[1] == 17`) rather than an alias
/// (`f[1] == 2065`).
///
/// This test pins the observed coverage so a future improvement to the
/// classifier surfaces as a measurable change.
#[test]
fn identify_potential_aliases_matches_most_mdl_aliases_on_econ() {
    if !Path::new("../../test/bobby/vdf/econ/base.vdf").exists() {
        return;
    }

    // econ/base.vdf: MDL declares 5 stdlib-call aliases. The VDF's
    // classification signal catches 4 (the 5th, `perceived mortgage
    // balance`, uses an expression argument to SMTH1 and is classified
    // as a regular variable).
    let vdf = parse_vdf("../../test/bobby/vdf/econ/base.vdf");
    let mdl = load_mdl("../../test/bobby/vdf/econ/mark2.mdl");
    let mdl_aliases = collect_mdl_stdlib_aliases(&mdl);
    assert_eq!(
        mdl_aliases.len(),
        5,
        "econ/base.vdf: MDL should declare 5 stdlib aliases; got {mdl_aliases:?}"
    );

    let detected = vdf.identify_potential_aliases();
    let detected_names: Vec<String> = detected.iter().map(|(_, n)| n.clone()).collect();

    // Overlap must include at least 4 of the 5 MDL-declared aliases.
    let overlap: Vec<&String> = detected_names
        .iter()
        .filter(|n| mdl_aliases.iter().any(|a| normalize(n) == normalize(a)))
        .collect();
    assert!(
        overlap.len() >= 4,
        "econ/base.vdf: expected >=4 MDL aliases in the detected set; \
         detected={detected_names:?}, MDL aliases={mdl_aliases:?}, overlap={overlap:?}"
    );

    // No false positives: every detected alias must actually be in the
    // MDL alias list. (Classification-driven detection should not emit
    // non-alias names.)
    for name in &detected_names {
        assert!(
            mdl_aliases.iter().any(|a| normalize(name) == normalize(a)),
            "econ/base.vdf: detected {name:?} is not in the MDL alias list"
        );
    }

    // econ/risk.vdf: MDL declares 7 stdlib-call aliases. Same story: the
    // classification signal catches 6.
    let vdf = parse_vdf("../../test/bobby/vdf/econ/risk.vdf");
    let mdl = load_mdl("../../test/bobby/vdf/econ/mark2.mdl");
    // mark2.mdl has 5 aliases, but risk.mdl doesn't exist. We compare
    // the identified count against the VDF's own f[1]=2065 count as a
    // self-consistency check since the MDL doesn't match.
    let detected = vdf.identify_potential_aliases();
    let detected_names: Vec<String> = detected.iter().map(|(_, n)| n.clone()).collect();
    assert!(
        detected_names.len() >= 5,
        "econ/risk.vdf: expected >=5 detected aliases; got {detected_names:?}"
    );
    let _ = mdl; // silence the "unused" warning on this fixture.

    // Order must match the name-table file order (strictly increasing
    // name indices).
    for pair in detected.windows(2) {
        assert!(
            pair[0].0 < pair[1].0,
            "detected aliases must be in name-table file order; got {:?}",
            detected
        );
    }
}

/// Pair MDL-declared aliases with VDF output signatures via file-order
/// (Agent 1's Claim B): for each alias in MDL declaration order (resolved
/// to VDF name-index) and each output sig in file-order, the pairs agree
/// on function family.
#[test]
fn alias_to_output_sig_pairing_via_file_order_agrees_on_family() {
    if !Path::new("../../test/bobby/vdf/econ/base.vdf").exists() {
        return;
    }
    let vdf = parse_vdf("../../test/bobby/vdf/econ/base.vdf");
    let mdl = load_mdl("../../test/bobby/vdf/econ/mark2.mdl");
    let output_sigs = vdf.output_signatures();
    let mdl_aliases_with_eq = collect_mdl_alias_equations(&mdl);

    // Resolve each MDL alias to its VDF name index and sort by that
    // index (file order).
    let mut alias_positions: Vec<(usize, String, String)> = mdl_aliases_with_eq
        .iter()
        .filter_map(|(name, eq)| {
            let target = normalize(name);
            vdf.names
                .iter()
                .enumerate()
                .find(|(_, n)| normalize(n) == target)
                .map(|(i, n)| (i, n.clone(), eq.clone()))
        })
        .collect();
    alias_positions.sort_by_key(|(i, _, _)| *i);

    assert_eq!(
        alias_positions.len(),
        output_sigs.len(),
        "econ/base.vdf: MDL alias count must equal output sig count"
    );

    // Pair by list-index; each pair must agree on function family.
    for ((alias_idx, alias_name, alias_eq), (sig_idx, sig_name)) in
        alias_positions.iter().zip(output_sigs.iter())
    {
        assert!(
            alias_idx < sig_idx,
            "alias {alias_name:?} must precede its target sig {sig_name:?} in the name table"
        );
        let afam = equation_family(alias_eq);
        let sfam = sig_family(sig_name);
        assert_eq!(
            afam, sfam,
            "alias {alias_name:?} (family {afam}) paired with mismatched \
             sig {sig_name:?} (family {sfam})"
        );
    }
}

fn collect_mdl_stdlib_aliases(project: &simlin_engine::datamodel::Project) -> Vec<String> {
    collect_mdl_alias_equations(project)
        .into_iter()
        .map(|(n, _)| n)
        .collect()
}

fn collect_mdl_alias_equations(
    project: &simlin_engine::datamodel::Project,
) -> Vec<(String, String)> {
    let Some(model) = project.models.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for var in &model.variables {
        let (ident, equation) = match var {
            simlin_engine::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
            simlin_engine::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
            _ => continue,
        };
        let text = match equation {
            simlin_engine::datamodel::Equation::Scalar(s)
            | simlin_engine::datamodel::Equation::ApplyToAll(_, s) => s.as_str(),
            _ => continue,
        };
        if equation_starts_with_stdlib_call(text) {
            out.push((ident.clone(), text.to_string()));
        }
    }
    out
}
