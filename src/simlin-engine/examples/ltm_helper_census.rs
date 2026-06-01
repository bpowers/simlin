// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Census of PREVIOUS()/INIT() call sites in generated LTM equations (GH #654).
//!
//! Compiles a model's LTM instrumentation (discovery mode) and classifies
//! every `PREVIOUS(...)` argument in the generated equation text by syntactic
//! form, approximating which forms compile to a direct LoadPrev and which
//! force a synthesized helper aux. The classification is heuristic (it cannot
//! see variable shadowing); the ground truth helper count is the
//! `model_ltm_implicit_var_info` total printed above the breakdown.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example ltm_helper_census
//!   CLEARN_MODEL=path/to/model.mdl cargo run --release -p simlin-engine \
//!       --example ltm_helper_census

use std::collections::BTreeMap;

use salsa::Setter;
use simlin_engine::db::{
    SimlinDb, model_ltm_implicit_var_info, model_ltm_variables, sync_from_datamodel_incremental,
};
use simlin_engine::{canonicalize, open_vensim, open_xmile};

/// Extract the balanced-paren argument starting right after an opening paren.
fn balanced_arg(text: &str) -> &str {
    let mut depth = 1usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[..i];
                }
            }
            _ => {}
        }
    }
    text
}

/// Is this string a plain identifier (letters, digits, underscores, quotes)?
fn is_bare_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '"' || c == '$' || c == '\u{205A}')
}

/// Classify one PREVIOUS argument by what the builtins visitor will do
/// with it (see `builtins_visitor.rs` PREVIOUS/INIT opcode routing).
fn classify_arg(arg: &str) -> &'static str {
    let arg = arg.trim();
    // Strip a trailing `, 0` fallback arg (PREVIOUS(x, 0)).
    let arg = arg.strip_suffix(", 0").unwrap_or(arg);

    if arg.contains('\u{00B7}') && !arg.contains('[') {
        return "module-output ref (PREVIOUS(module·output)) -> helper";
    }
    if let Some(bracket) = arg.find('[') {
        let base = &arg[..bracket];
        if !is_bare_ident(base) {
            return "complex expression -> helper";
        }
        let indices = arg[bracket + 1..arg.rfind(']').unwrap_or(arg.len())].to_string();
        // All indices static? qualified `dim·elem` or numeric.
        let all_static = indices.split(',').all(|idx| {
            let idx = idx.trim();
            idx.contains('\u{00B7}') || idx.parse::<f64>().is_ok()
        });
        if all_static {
            return "static subscript -> direct LoadPrev (no helper)";
        }
        // Are the non-static indices bare element names or expressions?
        let any_expr = indices.split(',').any(|idx| {
            let idx = idx.trim();
            !idx.contains('\u{00B7}') && idx.parse::<f64>().is_err() && !is_bare_ident(idx)
        });
        if any_expr {
            return "dynamic-expression subscript -> helper";
        }
        // Since GH #654, a non-shadowed bare element compiles to a direct
        // LoadPrev; only an element name shadowed by a variable still forces
        // a helper.
        return "bare-element subscript -> direct LoadPrev unless shadowed";
    }
    if is_bare_ident(arg) {
        return "bare ident -> direct LoadPrev (no helper)";
    }
    "complex expression -> helper"
}

fn main() {
    let path = std::env::var("CLEARN_MODEL").unwrap_or_else(|_| {
        format!(
            "{}/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let contents = std::fs::read_to_string(&path).unwrap();
    let datamodel = if path.ends_with(".mdl") {
        open_vensim(&contents).unwrap()
    } else {
        open_xmile(&mut contents.as_bytes()).unwrap()
    };
    println!("model: {path}");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let source_project = sync.project;
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let root_name = datamodel
        .models
        .first()
        .map(|m| m.name.as_str())
        .unwrap_or("main");
    let canonical_name = canonicalize(root_name).into_owned();
    let source_model = source_project
        .models(&db)
        .get(canonical_name.as_str())
        .copied()
        .unwrap();

    let ltm_vars = model_ltm_variables(&db, source_model, source_project);
    let implicit_info = model_ltm_implicit_var_info(&db, source_model, source_project);

    let n_implicit_modules = implicit_info.values().filter(|m| m.is_module).count();
    let implicit_slots: usize = implicit_info.values().map(|m| m.size).sum();

    println!("LTM synthetic vars: {}", ltm_vars.vars.len());
    println!(
        "LTM implicit (helper) vars: {} ({} modules), {} slots",
        implicit_info.len(),
        n_implicit_modules,
        implicit_slots
    );

    // Classify every PREVIOUS call site in the generated equations.
    let mut buckets: BTreeMap<&'static str, (usize, Vec<String>)> = BTreeMap::new();
    let mut total_sites = 0usize;
    for v in &ltm_vars.vars {
        let texts: Vec<&str> = match &v.equation {
            simlin_engine::datamodel::Equation::Scalar(t) => vec![t.as_str()],
            simlin_engine::datamodel::Equation::ApplyToAll(_, t) => vec![t.as_str()],
            simlin_engine::datamodel::Equation::Arrayed(_, elems, _, _) => {
                elems.iter().map(|(_, t, _, _)| t.as_str()).collect()
            }
        };
        for text in texts {
            let lower = text.to_lowercase();
            let mut search_from = 0usize;
            while let Some(pos) = lower[search_from..].find("previous(") {
                let abs = search_from + pos + "previous(".len();
                let arg = balanced_arg(&text[abs..]);
                let key = classify_arg(arg);
                let entry = buckets.entry(key).or_default();
                entry.0 += 1;
                total_sites += 1;
                if entry.1.len() < 5 {
                    let truncated: String = arg.chars().take(110).collect();
                    entry.1.push(truncated);
                }
                search_from = abs;
            }
        }
    }

    // Dump a few full LTM vars whose equations contain unqualified
    // bare-element subscripts inside PREVIOUS, including the equation variant.
    if std::env::var("CENSUS_DUMP_BARE").is_ok() {
        let mut shown = 0;
        for v in &ltm_vars.vars {
            let (variant, texts): (&str, Vec<&str>) = match &v.equation {
                simlin_engine::datamodel::Equation::Scalar(t) => ("Scalar", vec![t.as_str()]),
                simlin_engine::datamodel::Equation::ApplyToAll(d, t) => {
                    let _ = d;
                    ("ApplyToAll", vec![t.as_str()])
                }
                simlin_engine::datamodel::Equation::Arrayed(_, elems, _, _) => (
                    "Arrayed",
                    elems.iter().map(|(_, t, _, _)| t.as_str()).collect(),
                ),
            };
            for text in &texts {
                let lower = text.to_lowercase();
                if lower.contains("previous(pct_interim_change_in_ff_emissions[cop_developing_b])")
                    || lower.contains("previous(pct_interim_change_in_ff_emissions[g77_china])")
                {
                    println!("\n=== {} [{}] dims={:?}", v.name, variant, v.dimensions);
                    let truncated: String = text.chars().take(700).collect();
                    println!("    {truncated}");
                    shown += 1;
                    break;
                }
            }
            if shown >= 3 {
                break;
            }
        }
    }

    println!("\nPREVIOUS call sites in generated LTM equations: {total_sites}");
    let mut sorted: Vec<_> = buckets.iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.1.0));
    for (key, (count, examples)) in sorted {
        println!("  {count:>7}  {key}");
        for ex in examples {
            println!("             e.g. PREVIOUS({ex})");
        }
    }
}
