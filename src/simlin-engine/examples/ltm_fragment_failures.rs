// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Root-cause harness for LTM synthetic fragments that fail to compile.
//!
//! `model_ltm_fragment_diagnostics` reports *that* a generated LTM equation
//! did not compile, but not why: the three failure points inside
//! `compile_ltm_equation_fragment` (parse errors, the `Var::new` lowering
//! `Err`, and codegen returning `None`) all collapse to one `Option::None`.
//! With ~1,600 failures on a single real model, "which construct is broken"
//! is not answerable from the warning text alone.
//!
//! This harness joins the failure list back to the generated equation each
//! failing variable carries, buckets the equations by the construct they
//! contain, and prints representative equation text per bucket -- so a small
//! number of root causes can be told apart from a long tail.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --example ltm_fragment_failures
//!   LTM_FAIL_MODEL=path/to/model.mdl cargo run --release ... --example ltm_fragment_failures
//!   LTM_FAIL_SHOW=8 ...   # representative equations printed per bucket
//!   LTM_FAIL_BUCKET=agg-subscript ...   # dump every equation in one bucket

use std::collections::BTreeMap;
use std::path::PathBuf;

use simlin_engine::db::{
    LtmEquation, LtmSyntheticVar, SimlinDb, model_ltm_variables, set_project_ltm_enabled,
    sync_from_datamodel_incremental,
};
use simlin_engine::open_vensim;

/// The generated equation text for a synthetic variable, one entry per arm
/// (a scalar equation has exactly one).
fn arm_texts(equation: &LtmEquation) -> Vec<String> {
    match equation {
        LtmEquation::Scalar(arm) => vec![arm.text.clone()],
        LtmEquation::ApplyToAll(_, arm) => vec![arm.text.clone()],
        LtmEquation::Arrayed {
            elements, default, ..
        } => elements
            .iter()
            .map(|(elem, arm)| format!("[{elem}] {}", arm.text))
            .chain(default.as_ref().map(|d| format!("[default] {}", d.text)))
            .collect(),
    }
}

/// Bucket a failing variable by the construct its generated equation contains.
///
/// Ordered most-specific first: a single equation can exhibit several of
/// these, and the first matching label is the one worth reporting.
fn bucket(name: &str, texts: &[String]) -> &'static str {
    let all = texts.join(" ");

    // A reference to a name that is itself an LTM synthetic (the nested `$:`
    // prefix) means one generated equation reads another generated variable.
    let nested_synthetic = name.matches("$\u{205a}").count() > 1
        || name.starts_with("$\u{205a}$\u{205a}")
        || (name.contains("\u{205a}ltm\u{205a}") && name.matches('$').count() > 1);

    if name.ends_with("\u{205a}arg0") || name.ends_with("\u{205a}arg1") {
        return "implicit-helper: captured builtin argument";
    }
    if nested_synthetic {
        return "name embeds another synthetic (module-instance endpoint)";
    }
    if all.contains("\u{205a}ltm\u{205a}agg\u{205a}") {
        return "reads a synthetic aggregate node";
    }
    if all.contains("smth") || all.contains("delay") || all.contains("trend") {
        return "equation contains a SMOOTH/DELAY/TREND call";
    }
    if all.contains("lookup(") {
        return "equation contains a LOOKUP";
    }
    if all.contains("previous(") && all.contains('[') {
        return "subscripted PREVIOUS";
    }
    if all.contains('[') {
        return "subscripted reference";
    }
    "other"
}

/// Every `name[i1,i2,...]` subscript in `text`, as the raw index lists.
fn subscripts(text: &str) -> Vec<Vec<String>> {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '[' {
            let mut depth = 1;
            let mut j = i + 1;
            let mut inner = String::new();
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    '[' => {
                        depth += 1;
                        inner.push('[');
                    }
                    ']' => {
                        depth -= 1;
                        if depth > 0 {
                            inner.push(']');
                        }
                    }
                    c => inner.push(c),
                }
                j += 1;
            }
            out.push(inner.split(',').map(|s| s.trim().to_string()).collect());
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// How a single subscript index is spelled.
///
/// The distinction that matters: a per-element link-score partial is a SCALAR
/// fragment, so every index must select one element. An index left as a bare
/// DIMENSION name still denotes the whole axis and cannot compile there --
/// that is the shape this harness is looking for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum IndexKind {
    /// `dim·elem` -- explicitly qualified, always unambiguous.
    QualifiedElement,
    /// a bare element name of some dimension
    BareElement,
    /// a bare DIMENSION name: selects the whole axis
    BareDimension,
    /// a literal position
    Numeric,
    /// `@2`, arithmetic, a variable read -- resolved at compile or run time
    Expression,
}

struct DimTables {
    dims: std::collections::BTreeSet<String>,
    elements: std::collections::BTreeSet<String>,
}

fn classify_index(idx: &str, tables: &DimTables) -> IndexKind {
    let lower = idx.to_lowercase();
    if idx.contains('\u{00B7}') {
        return IndexKind::QualifiedElement;
    }
    if lower.parse::<f64>().is_ok() {
        return IndexKind::Numeric;
    }
    if tables.dims.contains(&lower) {
        return IndexKind::BareDimension;
    }
    if tables.elements.contains(&lower) {
        return IndexKind::BareElement;
    }
    IndexKind::Expression
}

/// Classify the *endpoint shape* of a link-score name: what kind of thing sits
/// on each side of the arrow. This is orthogonal to `bucket` -- it describes
/// the edge rather than the equation body.
fn endpoint_shape(name: &str) -> &'static str {
    let Some(rest) = name.split("link_score\u{205a}").nth(1) else {
        return "not a link score";
    };
    let Some((from, to)) = rest.split_once('\u{2192}') else {
        return "no arrow in name";
    };
    let synth = |s: &str| s.starts_with('$');
    let sub = |s: &str| s.contains('[');
    match (synth(from), synth(to), sub(from), sub(to)) {
        (true, true, _, _) => "synthetic -> synthetic",
        (true, false, _, true) => "synthetic -> arrayed var",
        (true, false, _, false) => "synthetic -> scalar var",
        (false, true, true, _) => "arrayed var -> synthetic",
        (false, true, false, _) => "scalar var -> synthetic",
        (false, false, true, true) => "arrayed -> arrayed",
        (false, false, true, false) => "arrayed -> scalar",
        (false, false, false, true) => "scalar -> arrayed",
        (false, false, false, false) => "scalar -> scalar",
    }
}

fn main() {
    let model_path = std::env::var("LTM_FAIL_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl")
        });
    let show: usize = std::env::var("LTM_FAIL_SHOW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let dump_bucket = std::env::var("LTM_FAIL_BUCKET").ok();

    let contents = std::fs::read_to_string(&model_path).expect("read model");
    let datamodel = open_vensim(&contents).expect("import vensim model");
    println!("model: {}", model_path.display());

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let main_name = datamodel
        .models
        .iter()
        .find(|m| m.name == "main")
        .map(|m| m.name.clone())
        .unwrap_or_else(|| datamodel.models[0].name.clone());
    let source_model = sync.models[main_name.as_str()].source_model;

    let ltm = model_ltm_variables(&db, source_model, sync.project);
    println!("ltm mode: {:?}", ltm.mode);
    println!("synthetic LTM variables emitted: {}", ltm.vars.len());

    // The failure set, harvested exactly as a consumer sees it: the
    // accumulated warnings from a whole-project diagnostic pass.
    //
    // `collect_all_diagnostics` covers EVERY model in the project, while `ltm`
    // above is the analyzed model's alone -- and a project carries the spliced
    // stdlib SMOOTH/DELAY templates as models of their own. Keying failures by
    // variable name without checking the model would let a sub-model failure
    // be counted as this model's, and -- because the join against `ltm.vars`
    // would then miss -- misclassified as an implicit helper. Same-named
    // variables in different models would collide outright. Filter first, and
    // report what was dropped rather than discarding it silently, since a
    // non-empty drop means the analyzed model is not the whole story.
    let diagnostics = simlin_engine::db::collect_all_diagnostics(&db, sync.project);
    let mut failed_names: Vec<String> = Vec::new();
    let mut reasons: BTreeMap<String, String> = BTreeMap::new();
    let mut other_model_failures: BTreeMap<String, usize> = BTreeMap::new();
    for d in &diagnostics {
        let msg = match &d.error {
            simlin_engine::db::DiagnosticError::Assembly(m) => m,
            _ => continue,
        };
        if !msg.contains("failed to compile") {
            continue;
        }
        if d.model != main_name {
            *other_model_failures.entry(d.model.clone()).or_insert(0) += 1;
            continue;
        }
        if let Some(v) = &d.variable {
            failed_names.push(v.clone());
            if let Some(r) = msg.split("Reason").nth(1) {
                reasons.insert(
                    v.clone(),
                    r.trim_start_matches([':', ' ']).trim().to_string(),
                );
            }
        }
    }
    failed_names.sort();
    failed_names.dedup();
    println!(
        "fragments reported as failing to compile: {}",
        failed_names.len()
    );
    if other_model_failures.is_empty() {
        println!("  (no failures in other project models)");
    } else {
        let total: usize = other_model_failures.values().sum();
        println!("  EXCLUDED -- {total} failure(s) in other project models, by model:");
        for (model, n) in &other_model_failures {
            println!("    {n:6}  {model}");
        }
    }

    // --- The actual compiler reasons ---------------------------------------
    //
    // Normalize each reason to its shape (variable names and offsets differ
    // per fragment) so the distinct root causes are countable.
    fn reason_shape(r: &str) -> String {
        // `SimulationError{code: <per-fragment detail>}` -> `... {code}`; the
        // detail is a variable name and would make every fragment its own
        // bucket.
        if let Some(open) = r.find('{')
            && let Some(colon) = r[open..].find(':')
        {
            let head = &r[..open];
            let code = &r[open + 1..open + colon];
            let detail = r[open + colon + 1..]
                .trim_end_matches(['}', '.', ' '])
                .trim();
            return format!("{head}{{{code}}}: {detail}");
        }
        // `<Code> at <start>..<end>` -- the byte span is per-fragment noise.
        if let Some(at) = r.find(" at ") {
            return r[..at].to_string();
        }
        r.to_string()
    }

    println!("\n=== compiler reasons, by shape ===");
    let mut reason_counts: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for (name, r) in &reasons {
        let e = reason_counts
            .entry(reason_shape(r))
            .or_insert((0, Vec::new()));
        e.0 += 1;
        if e.1.len() < 3 {
            e.1.push(format!("{name}\n        -> {r}"));
        }
    }
    let mut reason_rows: Vec<_> = reason_counts.iter().collect();
    reason_rows.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    for (shape, (n, examples)) in &reason_rows {
        println!("\n  {n:6}  {shape}");
        for e in examples {
            println!("      {e}");
        }
    }
    println!(
        "\n  fragments with NO reason captured: {}",
        failed_names.len() - reasons.len()
    );

    // Do the five distinct errors correspond to five distinct bugs, or are
    // some of them the same defect surfacing at different compiler stages?
    // Cross-tabulate each reason against two structural facts:
    //   * is this an element-pinned (scalarized) partial? -- a `[elem]` in the
    //     name, which is what `compile_ltm_synthetic_fragment` routes through
    //     the direct per-element compile;
    //   * does either endpoint name an implicit module INSTANCE (an expanded
    //     SMOOTH/DELAY), which is what makes the source a `module·port` read.
    println!("\n=== reason x structure ===");
    println!(
        "  {:<46} {:>7} {:>11} {:>9} {:>8} {:>9}",
        "reason", "n", "element-pin", "instance", "LOOKUP", "vector-op"
    );
    let eqn_text: BTreeMap<&str, String> = ltm
        .vars
        .iter()
        .map(|v| (v.name.as_str(), arm_texts(&v.equation).join(" ")))
        .collect();
    let mut cross: BTreeMap<String, (usize, usize, usize, usize, usize)> = BTreeMap::new();
    for (name, r) in &reasons {
        let shape = reason_shape(r);
        // The element pin lives on the TARGET side of the arrow (the source
        // side's bracket marks a per-source-element score instead).
        let element_pinned = name
            .split("link_score\u{205a}")
            .nth(1)
            .and_then(|rest| rest.split('\u{2192}').nth(1))
            .is_some_and(|to| to.contains('['))
            || name.contains("]\u{205a}");
        let instance_endpoint = name
            .split("link_score\u{205a}")
            .nth(1)
            .map(|rest| {
                rest.split('\u{2192}')
                    .any(|side| side.starts_with('$') && side.contains('\u{205a}'))
            })
            .unwrap_or(false);
        // Which construct does the equation carry? A LOOKUP's table argument
        // is the known GH #984 gap (a graphical-function holder is absent from
        // the dependency dims, so its axis can never be resolved to pin the
        // index); the array-producing vector builtins are a separate shape.
        let body = eqn_text.get(name.as_str()).cloned().unwrap_or_default();
        let has_lookup = body.contains("lookup(");
        let has_vector_op = [
            "vector_elm_map",
            "vector_sort_order",
            "rank(",
            "vector_select",
        ]
        .iter()
        .any(|k| body.contains(k));

        let e = cross.entry(shape).or_insert((0, 0, 0, 0, 0));
        e.0 += 1;
        if element_pinned {
            e.1 += 1;
        }
        if instance_endpoint {
            e.2 += 1;
        }
        if has_lookup {
            e.3 += 1;
        }
        if has_vector_op {
            e.4 += 1;
        }
    }
    let mut cross_rows: Vec<_> = cross.into_iter().collect();
    cross_rows.sort_by_key(|(_, (n, _, _, _, _))| std::cmp::Reverse(*n));
    for (shape, (n, pinned, inst, lookup, vecop)) in &cross_rows {
        let short: String = shape.chars().take(46).collect();
        println!("  {short:<46} {n:>7} {pinned:>11} {inst:>9} {lookup:>8} {vecop:>9}");
    }

    let by_name: BTreeMap<&str, &LtmSyntheticVar> =
        ltm.vars.iter().map(|v| (v.name.as_str(), v)).collect();

    // A failing name that is NOT in `ltm.vars` is an implicit helper (the
    // `model_ltm_implicit_var_info` leg of the diagnostic pass), which carries
    // no equation of its own here.
    let mut buckets: BTreeMap<&'static str, Vec<(String, Vec<String>)>> = BTreeMap::new();
    let mut shapes: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut helper_only = 0usize;

    for name in &failed_names {
        shapes
            .entry(endpoint_shape(name))
            .and_modify(|n| *n += 1)
            .or_insert(1);
        match by_name.get(name.as_str()) {
            Some(var) => {
                let texts = arm_texts(&var.equation);
                buckets
                    .entry(bucket(name, &texts))
                    .or_default()
                    .push((name.clone(), texts));
            }
            None => {
                helper_only += 1;
                buckets
                    .entry(bucket(name, &[]))
                    .or_default()
                    .push((name.clone(), vec!["<implicit helper: no equation>".into()]));
            }
        }
    }

    println!("  of which are implicit helpers (no synthetic equation): {helper_only}");

    println!("\n=== failing edges by endpoint shape ===");
    let mut shape_rows: Vec<_> = shapes.into_iter().collect();
    shape_rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (shape, n) in shape_rows {
        println!("  {n:6}  {shape}");
    }

    println!("\n=== failing fragments by construct ===");
    let mut bucket_rows: Vec<_> = buckets.iter().collect();
    bucket_rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (label, entries) in &bucket_rows {
        println!("  {:6}  {}", entries.len(), label);
    }

    for (label, entries) in &bucket_rows {
        println!("\n--- {} ({} fragments) ---", label, entries.len());
        let limit = if dump_bucket.as_deref() == Some(*label) {
            entries.len()
        } else {
            show
        };
        for (name, texts) in entries.iter().take(limit) {
            println!("  name: {name}");
            for t in texts.iter().take(3) {
                let shown: String = t.chars().take(400).collect();
                println!("    eqn: {shown}");
            }
            if texts.len() > 3 {
                println!("    ... {} more arms", texts.len() - 3);
            }
        }
    }

    // --- The hypothesis test -----------------------------------------------
    //
    // A per-element link-score partial is a scalar fragment. If a subscript
    // index in one is left as a bare DIMENSION name it still denotes the whole
    // axis, which cannot compile in scalar context. Count how many failures
    // exhibit that, and -- the control -- how many SUCCEEDING fragments do.
    let mut tables = DimTables {
        dims: Default::default(),
        elements: Default::default(),
    };
    for dim in &datamodel.dimensions {
        tables.dims.insert(dim.name.to_lowercase());
        if let simlin_engine::datamodel::DimensionElements::Named(names) = &dim.elements {
            for elem in names {
                tables.elements.insert(elem.to_lowercase());
            }
        }
    }
    println!(
        "\n=== dimension tables: {} dimensions, {} element names ===",
        tables.dims.len(),
        tables.elements.len()
    );

    let failed_set: std::collections::BTreeSet<&str> =
        failed_names.iter().map(|s| s.as_str()).collect();

    let mut fail_with_bare_dim = 0usize;
    let mut fail_without = 0usize;
    let mut ok_with_bare_dim = 0usize;
    let mut ok_without = 0usize;
    let mut bare_dim_examples: Vec<String> = Vec::new();
    let mut fail_no_bare_dim_examples: Vec<(String, String)> = Vec::new();

    for var in &ltm.vars {
        let texts = arm_texts(&var.equation);
        let mut offenders: Vec<(String, Vec<String>)> = Vec::new();
        for t in &texts {
            for sub in subscripts(t) {
                if sub
                    .iter()
                    .any(|idx| classify_index(idx, &tables) == IndexKind::BareDimension)
                {
                    offenders.push((t.clone(), sub));
                }
            }
        }
        let has_bare_dim = !offenders.is_empty();
        let failed = failed_set.contains(var.name.as_str());
        match (failed, has_bare_dim) {
            (true, true) => {
                fail_with_bare_dim += 1;
                if bare_dim_examples.len() < 6 {
                    let (_t, sub) = &offenders[0];
                    bare_dim_examples
                        .push(format!("{}   offending index list: {:?}", var.name, sub));
                }
            }
            (true, false) => {
                fail_without += 1;
                if fail_no_bare_dim_examples.len() < 6 {
                    fail_no_bare_dim_examples.push((
                        var.name.clone(),
                        texts
                            .first()
                            .cloned()
                            .unwrap_or_default()
                            .chars()
                            .take(300)
                            .collect(),
                    ));
                }
            }
            (false, true) => ok_with_bare_dim += 1,
            (false, false) => ok_without += 1,
        }
    }

    println!("\n=== does a bare dimension-name subscript predict failure? ===");
    println!(
        "  (over the {} synthetic vars that carry an equation)",
        ltm.vars.len()
    );
    println!("                       | FAILED | compiled");
    println!("  bare dim-name index  | {fail_with_bare_dim:6} | {ok_with_bare_dim:8}");
    println!("  none                 | {fail_without:6} | {ok_without:8}");
    println!("\n  examples of a FAILING fragment with a bare dimension-name index:");
    for e in &bare_dim_examples {
        println!("    {e}");
    }
    println!("\n  examples of a FAILING fragment WITHOUT one (needs its own explanation):");
    for (n, t) in &fail_no_bare_dim_examples {
        println!("    {n}\n      {t}");
    }

    // A bare dimension name is only ILLEGAL in a scalar fragment; in an
    // ApplyToAll equation over that same dimension it is the ordinary
    // same-element spelling. Refine by equation variant.
    println!("\n=== refined: bare dimension-name index, split by equation variant ===");
    println!("  variant      bare-dim?   FAILED   compiled");
    let mut refined: BTreeMap<(&'static str, bool), (usize, usize)> = BTreeMap::new();
    for var in &ltm.vars {
        let variant = match &var.equation {
            LtmEquation::Scalar(_) => "Scalar",
            LtmEquation::ApplyToAll(..) => "ApplyToAll",
            LtmEquation::Arrayed { .. } => "Arrayed",
        };
        let texts = arm_texts(&var.equation);
        let has_bare = texts.iter().any(|t| {
            subscripts(t).iter().any(|sub| {
                sub.iter()
                    .any(|idx| classify_index(idx, &tables) == IndexKind::BareDimension)
            })
        });
        let entry = refined.entry((variant, has_bare)).or_insert((0, 0));
        if failed_set.contains(var.name.as_str()) {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }
    for ((variant, has_bare), (f, ok)) in &refined {
        println!("  {variant:<12} {:<11} {f:6}   {ok:8}", has_bare);
    }

    // Are the failing implicit helpers independent causes, or consequences of
    // a parent link score that already failed? A helper is named
    // `$:<parent>:<n>:arg<k>[:<elem>]`; recover the parent and check.
    println!("\n=== are the 606 failing implicit helpers consequences of a failing parent? ===");
    let mut helper_parent_failed = 0usize;
    let mut helper_parent_ok = 0usize;
    let mut helper_parent_unknown = 0usize;
    let mut orphan_examples: Vec<String> = Vec::new();
    for name in &failed_names {
        if by_name.contains_key(name.as_str()) {
            continue; // has its own equation: not a helper
        }
        // Strip the leading synthesis marker and the trailing `:<n>:arg<k>[:<elem>]`.
        let inner = name.strip_prefix("$\u{205a}").unwrap_or(name);
        let parts: Vec<&str> = inner.split('\u{205a}').collect();
        let mut parent: Option<String> = None;
        for cut in (1..parts.len()).rev() {
            let cand = parts[..cut].join("\u{205a}");
            if by_name.contains_key(cand.as_str()) {
                parent = Some(cand);
                break;
            }
        }
        match parent {
            Some(p) if failed_set.contains(p.as_str()) => helper_parent_failed += 1,
            Some(p) => {
                helper_parent_ok += 1;
                if orphan_examples.len() < 6 {
                    orphan_examples.push(format!("{name}\n      parent COMPILED: {p}"));
                }
            }
            None => {
                helper_parent_unknown += 1;
                if orphan_examples.len() < 6 {
                    orphan_examples.push(format!(
                        "{name}\n      parent not found among synthetic vars"
                    ));
                }
            }
        }
    }
    println!("  helper failed AND its parent link score also failed : {helper_parent_failed}");
    println!("  helper failed but its parent COMPILED               : {helper_parent_ok}");
    println!("  helper failed, parent not identifiable             : {helper_parent_unknown}");
    if !orphan_examples.is_empty() {
        println!("\n  helpers not explained by a failing parent:");
        for e in &orphan_examples {
            println!("    {e}");
        }
    }

    // Cluster 2: link scores whose endpoint is an implicit module INSTANCE
    // (an expanded SMOOTH/DELAY), referenced as `"$:...:smth1:...·port"`.
    println!(
        "\n=== cluster: an endpoint is an implicit module instance (expanded SMOOTH/DELAY) ==="
    );
    let mut mod_fail = 0usize;
    let mut mod_ok = 0usize;
    let mut nonmod_fail = 0usize;
    let mut nonmod_ok = 0usize;
    for var in &ltm.vars {
        let endpoint_is_instance = var
            .name
            .split("link_score\u{205a}")
            .nth(1)
            .map(|rest| {
                rest.split('\u{2192}')
                    .any(|side| side.starts_with('$') && side.contains("\u{205a}"))
            })
            .unwrap_or(false);
        let failed = failed_set.contains(var.name.as_str());
        match (endpoint_is_instance, failed) {
            (true, true) => mod_fail += 1,
            (true, false) => mod_ok += 1,
            (false, true) => nonmod_fail += 1,
            (false, false) => nonmod_ok += 1,
        }
    }
    println!("                            FAILED   compiled");
    println!("  endpoint is an instance   {mod_fail:6}   {mod_ok:8}");
    println!("  neither endpoint is       {nonmod_fail:6}   {nonmod_ok:8}");

    // Cause B's exact mechanism. `builtins_visitor::is_module_backed_ident`
    // splits a `module·port` reference on the middle dot and asks whether the
    // BASE is a known module ident; if it is, PREVIOUS() is rewritten through
    // a scalar capture helper, and if it is not, PREVIOUS() gets a reference
    // codegen cannot take. So: are the module bases these failing equations
    // reference actually present in the implicit-module table?
    println!("\n=== cause B: are the referenced module bases known as modules? ===");
    let implicit = simlin_engine::db::model_implicit_var_info(&db, source_model, sync.project);
    let implicit_modules: std::collections::BTreeSet<&str> = implicit
        .iter()
        .filter(|(_, m)| m.is_module)
        .map(|(n, _)| n.as_str())
        .collect();
    let explicit_modules: std::collections::BTreeSet<String> = source_model
        .variables(&db)
        .iter()
        .filter(|(_, sv)| sv.kind(&db) == simlin_engine::db::SourceVariableKind::Module)
        .map(|(n, _)| n.to_string())
        .collect();
    println!(
        "  implicit module instances: {}   explicit module variables: {}",
        implicit_modules.len(),
        explicit_modules.len()
    );

    let mut base_known = 0usize;
    let mut base_unknown = 0usize;
    let mut unknown_examples: Vec<String> = Vec::new();
    for (name, r) in &reasons {
        if !r.contains("PREVIOUS requires a variable reference") {
            continue;
        }
        let body = eqn_text.get(name.as_str()).cloned().unwrap_or_default();
        // Every `·`-bearing quoted ident in the body.
        for tok in body.split('"').skip(1).step_by(2) {
            if let Some((base, _port)) = tok.split_once('\u{00B7}') {
                if implicit_modules.contains(base) || explicit_modules.contains(base) {
                    base_known += 1;
                } else {
                    base_unknown += 1;
                    if unknown_examples.len() < 5 {
                        unknown_examples.push(base.to_string());
                    }
                }
                break;
            }
        }
    }
    println!("  module·port references whose BASE is a known module   : {base_known}");
    println!("  module·port references whose BASE is NOT known        : {base_unknown}");
    if !unknown_examples.is_empty() {
        println!("  unknown bases (sample):");
        for e in &unknown_examples {
            println!("    {e}");
        }
        println!("  nearest known implicit module instances (sample):");
        for m in implicit_modules.iter().take(5) {
            println!("    {m}");
        }
    }

    // Did the PREVIOUS helper rewrite actually fire for the cause-B parents?
    // If it did, a capture aux exists whose `ltm_parent_name` is the failing
    // link score, and the defect is downstream of the rewrite; if it did not,
    // the defect is the rewrite's own decision.
    println!("\n=== cause B: was a capture helper synthesized for the failing parent? ===");
    let ltm_implicit =
        simlin_engine::db::model_ltm_implicit_var_info(&db, source_model, sync.project);
    let mut helpers_by_parent: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (hname, meta) in ltm_implicit.iter() {
        helpers_by_parent
            .entry(meta.ltm_parent_name.as_str())
            .or_default()
            .push(hname.as_str());
    }
    let mut with_helper = 0usize;
    let mut without_helper = 0usize;
    let mut helper_also_failed = 0usize;
    let mut sample: Vec<String> = Vec::new();
    for (name, r) in &reasons {
        if !r.contains("PREVIOUS requires a variable reference") {
            continue;
        }
        match helpers_by_parent.get(name.as_str()) {
            Some(hs) => {
                with_helper += 1;
                if hs.iter().any(|h| failed_set.contains(*h)) {
                    helper_also_failed += 1;
                }
                if sample.len() < 3 {
                    sample.push(format!("{name}\n        helpers: {hs:?}"));
                }
            }
            None => {
                without_helper += 1;
                if sample.len() < 3 {
                    sample.push(format!("{name}\n        NO capture helper synthesized"));
                }
            }
        }
    }
    println!("  cause-B parents WITH a capture helper   : {with_helper}");
    println!("     ...whose helper ALSO failed to compile: {helper_also_failed}");
    println!("  cause-B parents WITHOUT one             : {without_helper}");
    for s in &sample {
        println!("    {s}");
    }

    // Is each failure the generated link score itself, or the capture helper
    // synthesized while parsing it? They are different defects with different
    // fixes, and the reason alone does not distinguish them.
    println!("\n=== reason x (link score vs capture helper) ===");
    let mut kind_split: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (name, r) in &reasons {
        let e = kind_split.entry(reason_shape(r)).or_insert((0, 0));
        if ltm_implicit.contains_key(name.as_str()) {
            e.1 += 1;
        } else {
            e.0 += 1;
        }
    }
    let mut ks: Vec<_> = kind_split.into_iter().collect();
    ks.sort_by_key(|(_, (a, b))| std::cmp::Reverse(a + b));
    println!("  {:<50} {:>11} {:>8}", "reason", "link score", "helper");
    for (shape, (score, helper)) in &ks {
        let short: String = shape.chars().take(50).collect();
        println!("  {short:<50} {score:>11} {helper:>8}");
    }

    // Which target variables absorb the failures, and what shape are they?
    println!("\n=== failing link-score targets, by the target's own equation shape ===");
    let source_vars = source_model.variables(&db);
    let mut target_kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut example_targets: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for name in &failed_names {
        let Some(rest) = name.split("link_score\u{205a}").nth(1) else {
            continue;
        };
        let Some((_from, to)) = rest.split_once('\u{2192}') else {
            continue;
        };
        let to_base = to.split('[').next().unwrap_or(to);
        let to_base = to_base.split('\u{205a}').next().unwrap_or(to_base);
        let kind = match source_vars.get(to_base) {
            None => "target not a model variable",
            Some(sv) => match sv.equation(&db) {
                simlin_engine::datamodel::Equation::Scalar(..) => "target: Scalar equation",
                simlin_engine::datamodel::Equation::ApplyToAll(..) => "target: ApplyToAll equation",
                simlin_engine::datamodel::Equation::Arrayed(..) => {
                    "target: Arrayed (per-element) equation"
                }
            },
        };
        target_kinds
            .entry(kind)
            .and_modify(|n| *n += 1)
            .or_insert(1);
        let ex = example_targets.entry(kind).or_default();
        if ex.len() < 6 && !ex.iter().any(|e| e == to_base) {
            ex.push(to_base.to_string());
        }
    }
    let mut kind_rows: Vec<_> = target_kinds.into_iter().collect();
    kind_rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (kind, n) in kind_rows {
        println!("  {n:6}  {kind}");
        if let Some(ex) = example_targets.get(kind) {
            println!("          e.g. {}", ex.join(", "));
        }
    }
}
