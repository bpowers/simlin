// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Differential parity harness pinning the Rust VDF reader
//! (`VdfFile::to_results_via_records`) against the Python inspector
//! (`tools/vdf_xray.py`, `extract_named_results`).
//!
//! The two implementations decode the same undocumented format and were
//! brought into behavioral agreement; nothing else keeps them aligned going
//! forward. This test walks every run-file VDF (0x52 simulation results and
//! any future 0x53 sensitivity run) under `test/`, invokes
//! `python3 tools/vdf_xray.py --extract-json` ONCE for the whole corpus (one
//! interpreter launch amortizes startup), and requires:
//!
//! - identical result-column NAME SETS (after canonicalizing the Python
//!   display names with the same `Ident` rules the Rust emitter uses), in
//!   both directions;
//! - bitwise-identical VALUES for every shared column at every saved step
//!   (both sides decode the same f32 bytes and widen to f64), with
//!   NaN == NaN counting as equal because the JSON transport erases NaN
//!   payload bits.
//!
//! There is deliberately NO per-file exclusion list: any residual divergence
//! is a bug in one of the readers, and the whole point of this harness is
//! zero drift.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use simlin_engine::common::{Canonical, Ident};
use simlin_engine::vdf::{VdfFile, VdfKind, probe_vdf_kind};

/// Directories walked for parity fixtures. `test/` is always present; the
/// zambaqui third_party corpus rides the existence-continue convention
/// (optional checkout, skipped when absent) because it is the only corpus
/// source of 0x53 sensitivity runs and of header-0x74 data-grid blocks on a
/// genuinely third bitmap width (GH #842) -- pinning cross-reader agreement
/// there keeps the two data-grid implementations in lockstep. The rest of
/// `third_party/` stays excluded to bound the harness runtime.
fn parity_corpus_roots() -> &'static [&'static str] {
    &["../../test", "../../third_party/uib_sd/zambaqui"]
}

fn collect_vdf_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_vdf_files(&path));
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("vdf"))
        {
            files.push(path);
        }
    }
    files
}

/// Canonicalize a Python-emitted display name with the SAME rule the Rust
/// emitter applies in `record_results.rs::emit_owner_span`: `#`-prefixed
/// internal-signature names pass through raw (canonicalization would
/// collapse them), everything else goes through `Ident::new`. Keeping this a
/// call into the production canonicalizer -- not a reimplementation -- makes
/// the name mapping correct by construction.
fn canonical_key_for_display_name(name: &str) -> Ident<Canonical> {
    if name.starts_with('#') {
        Ident::<Canonical>::from_str_unchecked(name)
    } else {
        Ident::<Canonical>::new(name)
    }
}

/// Decode one `--extract-json` series value. The JSON transport encodes NaN
/// as `null` and the infinities as `"Infinity"` / `"-Infinity"` (documented
/// on `tools/vdf_xray.py::_encode_series_value`); finite values are plain
/// JSON numbers whose shortest-round-trip decimal form recovers exact f64
/// bits.
fn decode_json_value(value: &serde_json::Value, context: &str) -> f64 {
    match value {
        serde_json::Value::Null => f64::NAN,
        serde_json::Value::Number(n) => n
            .as_f64()
            .unwrap_or_else(|| panic!("{context}: non-f64 JSON number {n}")),
        serde_json::Value::String(s) if s == "Infinity" => f64::INFINITY,
        serde_json::Value::String(s) if s == "-Infinity" => f64::NEG_INFINITY,
        other => panic!("{context}: unexpected JSON series value {other}"),
    }
}

/// Bitwise f64 equality with NaN == NaN allowed. NaN payload bits are not
/// compared because the JSON transport normalizes every NaN to `null`.
fn values_equal(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Run `python3 tools/vdf_xray.py --extract-json <paths...>` once and return
/// the parsed payload keyed by the path strings we passed in.
fn python_extract_json(paths: &[PathBuf]) -> serde_json::Map<String, serde_json::Value> {
    // Deliberately no timeout: the tool only reads local files (the whole
    // corpus extracts in ~1s), and a pathological hang is backstopped by the
    // pre-commit/CI wall-clock cap on `cargo test`.
    let output = Command::new("python3")
        .arg("../../tools/vdf_xray.py")
        .arg("--extract-json")
        .args(paths)
        .output()
        .unwrap_or_else(|err| {
            panic!(
                "failed to launch python3 (required for the VDF parity harness, \
                 and already required by the repo's pre-commit pysimlin step): {err}"
            )
        });
    if !output.status.success() {
        panic!(
            "python3 tools/vdf_xray.py --extract-json exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("vdf_xray.py --extract-json emitted invalid JSON: {err}"));
    match parsed {
        serde_json::Value::Object(map) => map,
        other => panic!("vdf_xray.py --extract-json must emit a JSON object, got {other}"),
    }
}

#[test]
fn parity_python_and_rust_vdf_extraction_agree() {
    let mut fixtures: Vec<PathBuf> = Vec::new();
    for root in parity_corpus_roots() {
        let root_path = Path::new(root);
        if !root_path.exists() {
            continue;
        }
        for path in collect_vdf_files(root_path) {
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            // Every run-file KIND participates: 0x52 simulation results and
            // 0x53 sensitivity runs (both readers parse 0x53 with the 0x52
            // rules, so a future 0x53 fixture under test/ must not be
            // silently excluded -- today there are none; they live in
            // third_party, see vdf_sensitivity.rs). Dataset (0x41) files use
            // a different container and are skipped.
            if matches!(
                probe_vdf_kind(&data),
                Some(VdfKind::SimulationResults | VdfKind::SensitivityRun)
            ) {
                fixtures.push(path);
            }
        }
    }
    fixtures.sort();

    assert!(
        fixtures.len() >= 40,
        "expected at least 40 run-file VDF parity fixtures under test/, found {} \
         (an empty walk must fail loudly, not pass vacuously)",
        fixtures.len()
    );
    // Arrayed element-label parity needs arrayed fixtures in the corpus:
    // subscripts.vdf (small multi-dim) and Ref.vdf (C-LEARN, the largest
    // label surface). Guard their presence so label coverage cannot vanish
    // silently if fixtures move.
    for required in ["subscripts.vdf", "Ref.vdf"] {
        assert!(
            fixtures
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == required)),
            "parity corpus must contain the arrayed fixture {required}"
        );
    }

    let payload = python_extract_json(&fixtures);

    for path in &fixtures {
        let path_key = path.to_string_lossy();
        let entries = payload
            .get(path_key.as_ref())
            .unwrap_or_else(|| panic!("{path_key}: missing from --extract-json payload"))
            .as_array()
            .unwrap_or_else(|| panic!("{path_key}: payload entry is not an array"));

        // Canonicalize the Python display names; a duplicate canonical key on
        // either side would silently collapse a column, so fail loudly.
        let mut python_series: HashMap<Ident<Canonical>, (String, Vec<f64>)> = HashMap::new();
        for entry in entries {
            let name = entry["name"]
                .as_str()
                .unwrap_or_else(|| panic!("{path_key}: entry missing string name"));
            let values: Vec<f64> = entry["values"]
                .as_array()
                .unwrap_or_else(|| panic!("{path_key}: {name}: values is not an array"))
                .iter()
                .map(|v| decode_json_value(v, &format!("{path_key}: {name}")))
                .collect();
            let key = canonical_key_for_display_name(name);
            if let Some((prev_name, _)) = python_series.get(&key) {
                panic!(
                    "{path_key}: Python emitted duplicate canonical key {key} \
                     (display names {prev_name:?} and {name:?})"
                );
            }
            python_series.insert(key, (name.to_string(), values));
        }

        let data = std::fs::read(path).unwrap_or_else(|err| panic!("{path_key}: read: {err}"));
        let vdf =
            VdfFile::parse(data).unwrap_or_else(|err| panic!("{path_key}: Rust parse: {err}"));
        let results = vdf
            .to_results_via_records()
            .unwrap_or_else(|err| panic!("{path_key}: to_results_via_records: {err}"));

        // Name-set agreement, both directions. `Results::offsets` is a map,
        // so a duplicate Rust column would shrink the Rust set and surface
        // here as a Python-only name.
        let mut python_only: Vec<String> = python_series
            .keys()
            .filter(|key| !results.offsets.contains_key(*key))
            .map(|key| key.to_string())
            .collect();
        python_only.sort();
        let mut rust_only: Vec<String> = results
            .offsets
            .keys()
            .filter(|key| !python_series.contains_key(*key))
            .map(|key| key.to_string())
            .collect();
        rust_only.sort();
        assert!(
            python_only.is_empty() && rust_only.is_empty(),
            "{path_key}: result-name sets diverge\n  only in Python ({}): {:?}\n  \
             only in Rust ({}): {:?}",
            python_only.len(),
            python_only,
            rust_only.len(),
            rust_only,
        );

        // Value agreement for every shared column, bitwise (NaN-aware).
        for (key, (display_name, py_values)) in &python_series {
            let col = results.offsets[key];
            assert_eq!(
                py_values.len(),
                results.step_count,
                "{path_key}: {display_name}: Python series length {} != Rust step_count {}",
                py_values.len(),
                results.step_count,
            );
            for (step, &py_val) in py_values.iter().enumerate() {
                let rust_val = results.data[step * results.step_size + col];
                assert!(
                    values_equal(py_val, rust_val),
                    "{path_key}: {display_name} (key {key}): step {step}: \
                     Python {py_val:?} (bits {:#018x}) != Rust {rust_val:?} (bits {:#018x})",
                    py_val.to_bits(),
                    rust_val.to_bits(),
                );
            }
        }
    }
}
