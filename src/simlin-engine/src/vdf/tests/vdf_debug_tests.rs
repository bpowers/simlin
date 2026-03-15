// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

fn kendall_tau(order_a: &[usize], order_b: &[usize]) -> (f64, usize, usize) {
    let n = order_a.len();
    let mut concordant = 0usize;
    let mut discordant = 0usize;
    for i in 0..n {
        for j in (i + 1)..n {
            let a_cmp = order_a[i].cmp(&order_a[j]);
            let b_cmp = order_b[i].cmp(&order_b[j]);
            if a_cmp == b_cmp {
                concordant += 1;
            } else if a_cmp != std::cmp::Ordering::Equal
                && b_cmp != std::cmp::Ordering::Equal
            {
                discordant += 1;
            }
        }
    }
    let total = concordant + discordant;
    let tau = if total == 0 {
        0.0
    } else {
        (concordant as f64 - discordant as f64) / total as f64
    };
    (tau, concordant, discordant)
}

#[derive(Debug, Clone)]
struct DebugSection4Entry {
    tag: u32,
    kind: u16,
    refs: Vec<u32>,
}

#[derive(Debug, Clone)]
struct DebugSlotSpan<'a> {
    offset: u32,
    len: usize,
    data: &'a [u8],
}

fn parse_debug_section4_entries(vdf: &VdfFile) -> Option<(usize, Vec<DebugSection4Entry>, usize)> {
    let sec = vdf.sections.get(4)?;
    let end = sec.region_end.min(vdf.data.len());
    let sec1_data_size = vdf.sections.get(1)?.region_data_size();

    let mut best: Option<(usize, Vec<DebugSection4Entry>, usize)> = None;
    for skip_words in 0..=8usize {
        let mut entries = Vec::new();
        let mut pos = sec.data_offset() + skip_words * 4;

        while pos + 8 <= end {
            let tag = read_u32(&vdf.data, pos);
            let header = read_u32(&vdf.data, pos + 4);
            let kind = (header & 0xffff) as u16;
            let n = (header >> 16) as usize;
            if tag == 0 || kind == 0 || kind > 8 || n > 8 {
                break;
            }

            let refs_start = pos + 8;
            let refs_end = refs_start + (n + 1) * 4;
            if refs_end > end {
                break;
            }

            let refs: Vec<u32> = (0..=n)
                .map(|i| read_u32(&vdf.data, refs_start + i * 4))
                .collect();
            if !refs
                .iter()
                .all(|&r| r > 0 && r % 4 == 0 && (r as usize) < sec1_data_size)
            {
                break;
            }

            entries.push(DebugSection4Entry { tag, kind, refs });
            pos = refs_end;
        }

        let replace_best = match best.as_ref() {
            None => true,
            Some((_, best_entries, best_stop)) => {
                entries.len() > best_entries.len()
                    || (entries.len() == best_entries.len() && pos > *best_stop)
            }
        };
        if replace_best {
            best = Some((skip_words, entries, pos));
        }
    }

    best
}

fn debug_slot_spans(vdf: &VdfFile) -> Vec<DebugSlotSpan<'_>> {
    let Some(sec1) = vdf.sections.get(1) else {
        return Vec::new();
    };
    let sec1_data_start = sec1.data_offset();
    let sec1_data_end = sec1.region_end.min(vdf.data.len());

    let mut sorted_offsets = vdf.slot_table.clone();
    sorted_offsets.sort_unstable();

    let mut next_offset_by_offset: HashMap<u32, u32> = HashMap::new();
    for pair in sorted_offsets.windows(2) {
        next_offset_by_offset.insert(pair[0], pair[1]);
    }

    vdf.slot_table
        .iter()
        .filter_map(|&offset| {
            let abs = sec1_data_start + offset as usize;
            if abs >= sec1_data_end {
                return None;
            }
            let next_offset = next_offset_by_offset
                .get(&offset)
                .copied()
                .map(|o| o as usize)
                .unwrap_or_else(|| sec1.region_data_size());
            let end = sec1_data_start + next_offset.min(sec1.region_data_size());
            if end <= abs || end > sec1_data_end {
                return None;
            }
            Some(DebugSlotSpan {
                offset,
                len: end - abs,
                data: &vdf.data[abs..end],
            })
        })
        .collect()
}

fn debug_visible_vdf_names(vdf: &VdfFile) -> HashSet<String> {
    let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
    vdf.slot_table
        .iter()
        .enumerate()
        .filter_map(|(i, _)| {
            let name = vdf.names.get(i)?;
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                return None;
            }
            Some(normalize_vdf_name(name))
        })
        .collect()
}

fn debug_empirical_direct_visible_map(
    vdf: &VdfFile,
    results: &crate::Results,
) -> HashMap<String, usize> {
    let visible = debug_visible_vdf_names(vdf);
    build_empirical_ot_map(&vdf.extract_data().unwrap(), results)
        .unwrap()
        .into_iter()
        .filter_map(|(id, ot)| {
            if id.as_str() == "time" || id.as_str().starts_with('$') || id.as_str().starts_with('#') {
                return None;
            }
            let normalized = normalize_vdf_name(id.as_str());
            visible.contains(&normalized).then_some((normalized, ot))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DebugVisibleSlotWords {
    slot_ref: u32,
    words: [u32; 4],
}

fn debug_visible_slot_words(vdf: &VdfFile) -> HashMap<String, DebugVisibleSlotWords> {
    let Some(sec1) = vdf.slot_section() else {
        return HashMap::new();
    };
    let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();

    vdf.slot_table
        .iter()
        .enumerate()
        .filter_map(|(i, &slot_ref)| {
            let name = vdf.names.get(i)?;
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                return None;
            }

            let base = sec1.data_offset() + slot_ref as usize;
            if base + 16 > sec1.region_end || base + 16 > vdf.data.len() {
                return None;
            }

            Some((
                normalize_vdf_name(name),
                DebugVisibleSlotWords {
                    slot_ref,
                    words: [
                        read_u32(&vdf.data, base),
                        read_u32(&vdf.data, base + 4),
                        read_u32(&vdf.data, base + 8),
                        read_u32(&vdf.data, base + 12),
                    ],
                },
            ))
        })
        .collect()
}

fn debug_visible_filtered_candidates(vdf: &VdfFile) -> Vec<String> {
    let mut candidates: Vec<String> = vdf.names[..vdf.slot_table.len()]
        .iter()
        .filter(|name| {
            !name.is_empty()
                && !name.starts_with('.')
                && !name.starts_with('-')
                && !SYSTEM_NAMES.contains(&name.as_str())
        })
        .cloned()
        .collect();

    candidates.retain(|n| n.len() != 1 || n.chars().next().is_some_and(|c| c.is_alphanumeric()));
    let vensim_builtins: HashSet<&str> = VENSIM_BUILTINS.into_iter().collect();
    candidates.retain(|n| !vensim_builtins.contains(n.to_lowercase().as_str()));
    candidates.retain(|n| !is_vdf_metadata_entry(n));
    candidates
}

#[test]
#[ignore]
fn test_debug_section6_nonstock_codes_vs_compiled_constants() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project.clone()));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
        let compiled = crate::db::compile_project_incremental(&db, sync.project, "main").unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let codes = vdf.section6_ot_class_codes().unwrap();

        let mut matrix: HashMap<(u8, bool), usize> = HashMap::new();
        let mut outliers = Vec::new();

        for (id, &ot) in &empirical_map {
            let Some(code) = codes.get(ot).copied() else {
                continue;
            };
            let Some(off) = compiled.get_offset(id) else {
                continue;
            };
            let is_constant = compiled.is_constant_offset(off);
            *matrix.entry((code, is_constant)).or_default() += 1;

            if code == VDF_SECTION6_OT_CODE_STOCK || id.as_str() == "time" {
                continue;
            }
            if (code == 0x17 && !is_constant) || (code == 0x11 && is_constant) {
                outliers.push((id.as_str().to_string(), ot, code, is_constant));
            }
        }

        outliers.sort();

        eprintln!("\n=== section6 non-stock code vs compiled constants: {label} ===");
        for code in [0x11_u8, 0x17_u8] {
            let dynamic = matrix.get(&(code, false)).copied().unwrap_or(0);
            let constant = matrix.get(&(code, true)).copied().unwrap_or(0);
            eprintln!("  code 0x{code:02x}: dynamic={dynamic} constant={constant}");
        }
        eprintln!("  outliers: {}", outliers.len());
        for (name, ot, code, is_constant) in outliers.iter().take(40) {
            let kind = if *is_constant { "constant" } else { "dynamic" };
            eprintln!("    OT[{ot:3}] code=0x{code:02x} {kind:8} {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_direct_record_slots() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();

        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut direct_record_counts: HashMap<u32, usize> = HashMap::new();
        for rec in &vdf.records {
            let ot = rec.fields[11] as usize;
            if rec.fields[0] == 0 || rec.fields[10] == 0 || ot == 0 || ot >= vdf.offset_table_count
            {
                continue;
            }
            *direct_record_counts.entry(rec.fields[12]).or_default() += 1;
        }

        let mut saved_with_direct = 0usize;
        let mut saved_without_direct = Vec::new();
        let mut unsaved_with_direct = Vec::new();
        let mut total_saved = 0usize;
        let mut total_unsaved = 0usize;

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }

            let normalized = normalize_vdf_name(name);
            let record_count = direct_record_counts.get(&slot_ref).copied().unwrap_or(0);
            let is_saved = saved_names.contains(&normalized);

            if is_saved {
                total_saved += 1;
                if record_count > 0 {
                    saved_with_direct += 1;
                } else {
                    saved_without_direct.push(name.clone());
                }
            } else {
                total_unsaved += 1;
                if record_count > 0 {
                    unsaved_with_direct.push((name.clone(), record_count));
                }
            }
        }

        unsaved_with_direct.sort_by(|a, b| a.0.cmp(&b.0));
        saved_without_direct.sort();

        eprintln!("\n=== saved names vs direct record slots: {label} ===");
        eprintln!("  saved with direct records: {saved_with_direct}/{total_saved}");
        eprintln!(
            "  unsaved with direct records: {}/{}",
            unsaved_with_direct.len(),
            total_unsaved
        );
        eprintln!("  first saved without direct record:");
        for name in saved_without_direct.iter().take(30) {
            eprintln!("    {name}");
        }
        eprintln!("  first unsaved with direct records:");
        for (name, count) in unsaved_with_direct.iter().take(30) {
            eprintln!("    {name} ({count})");
        }
    }
}

#[test]
#[ignore]
fn test_debug_empirical_visible_ots_vs_slot_record_features() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let direct_empirical = debug_empirical_direct_visible_map(&vdf, &results);

        let mut slot_by_name: HashMap<String, u32> = HashMap::new();
        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            slot_by_name.insert(normalize_vdf_name(name), slot_ref);
        }

        let mut records_by_slot: HashMap<u32, Vec<&VdfRecord>> = HashMap::new();
        for rec in &vdf.records {
            if rec.fields[12] == 0 {
                continue;
            }
            records_by_slot.entry(rec.fields[12]).or_default().push(rec);
        }
        for records in records_by_slot.values_mut() {
            records.sort_by_key(|rec| rec.fields[10]);
        }

        let mut match_f0_f1: HashMap<(u32, u32), usize> = HashMap::new();
        let mut nonmatch_f0_f1: HashMap<(u32, u32), usize> = HashMap::new();
        let mut match_rank_counts: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        let mut match_ot_class: std::collections::BTreeMap<u8, usize> =
            std::collections::BTreeMap::new();
        let mut matched_names = 0usize;
        let mut unmatched_names = Vec::new();
        let mut examples = Vec::new();

        for (normalized, emp_ot) in &direct_empirical {
            let Some(&slot_ref) = slot_by_name.get(normalized) else {
                continue;
            };
            let Some(records) = records_by_slot.get(&slot_ref) else {
                unmatched_names.push((normalized.clone(), slot_ref, *emp_ot, "no-records".to_string()));
                continue;
            };

            let mut matched_here = false;
            for (rank, rec) in records.iter().enumerate() {
                let ot = rec.fields[11] as usize;
                let bucket = if ot == *emp_ot {
                    matched_here = true;
                    &mut match_f0_f1
                } else {
                    &mut nonmatch_f0_f1
                };
                *bucket.entry((rec.fields[0], rec.fields[1])).or_default() += 1;

                if ot == *emp_ot {
                    *match_rank_counts.entry(rank).or_default() += 1;
                    if let Some(code) = vdf.section6_ot_class_code(ot) {
                        *match_ot_class.entry(code).or_default() += 1;
                    }
                    if examples.len() < 40 {
                        examples.push((
                            normalized.clone(),
                            slot_ref,
                            emp_ot,
                            rank,
                            rec.fields[0],
                            rec.fields[1],
                            rec.fields[10],
                            rec.fields[11],
                        ));
                    }
                }
            }

            if matched_here {
                matched_names += 1;
            } else {
                unmatched_names.push((normalized.clone(), slot_ref, *emp_ot, "no-ot-match".to_string()));
            }
        }

        let top_pairs = |map: &HashMap<(u32, u32), usize>| {
            let mut rows: Vec<_> = map.iter().map(|(&(f0, f1), &count)| (count, f0, f1)).collect();
            rows.sort_by(|a, b| b.cmp(a));
            rows
        };

        eprintln!("\n=== empirical visible OTs vs slot-record features: {label} ===");
        eprintln!(
            "  direct visible names={} matched_by_slot_record={} unmatched={}",
            direct_empirical.len(),
            matched_names,
            unmatched_names.len()
        );
        eprintln!("  top matched f0/f1 pairs:");
        for (count, f0, f1) in top_pairs(&match_f0_f1).into_iter().take(16) {
            eprintln!("    count={count:>3} f0={f0:>5} f1={f1:>5}");
        }
        eprintln!("  top non-matched f0/f1 pairs:");
        for (count, f0, f1) in top_pairs(&nonmatch_f0_f1).into_iter().take(16) {
            eprintln!("    count={count:>3} f0={f0:>5} f1={f1:>5}");
        }
        eprintln!("  matched-record rank distribution:");
        for (rank, count) in match_rank_counts.iter().take(16) {
            eprintln!("    rank={rank:>2} count={count:>3}");
        }
        eprintln!("  matched OT classes:");
        for (code, count) in &match_ot_class {
            eprintln!("    code=0x{code:02x} count={count}");
        }
        eprintln!("  example matches:");
        for (name, slot_ref, emp_ot, rank, f0, f1, f10, f11) in examples.iter().take(24) {
            eprintln!(
                "    slot={slot_ref:>5} ot={emp_ot:>3} rank={rank:>2} f0={f0:>5} f1={f1:>5} f10={f10:>4} f11={f11:>4} {name}"
            );
        }
        eprintln!("  unmatched names:");
        unmatched_names.sort();
        for (name, slot_ref, emp_ot, reason) in unmatched_names.iter().take(32) {
            eprintln!("    slot={slot_ref:>5} ot={emp_ot:>3} {reason:>11} {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_vdf_participant_category_counts() {
    for (label, vdf_path) in [
        ("econ", "../../test/bobby/vdf/econ/base.vdf"),
        ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
    ] {
        let vdf = vdf_file(vdf_path);
        let mut visible = 0usize;
        let mut lookupish = 0usize;
        let mut hash_names = 0usize;
        let mut helper_names = 0usize;
        let mut participant_helper_names = 0usize;
        let mut builtin_names = 0usize;
        let mut other_metadata = 0usize;
        let mut helper_list = Vec::new();

        for name in &vdf.names {
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with(':')
                || name.starts_with('"')
                || SYSTEM_NAMES.contains(&name.as_str())
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                other_metadata += 1;
                continue;
            }
            if name.starts_with('#') {
                hash_names += 1;
                continue;
            }
            if matches!(
                name.as_str(),
                "IN" | "INI"
                    | "DEL"
                    | "LV1"
                    | "LV2"
                    | "LV3"
                    | "ST"
                    | "RT1"
                    | "RT2"
                    | "DL"
                    | "OUTPUT"
                    | "SMOOTH"
                    | "SMOOTHI"
                    | "SMOOTH3"
                    | "SMOOTH3I"
                    | "DELAY1"
                    | "DELAY1I"
                    | "DELAY3"
                    | "DELAY3I"
                    | "TREND"
                    | "NPV"
            ) {
                helper_names += 1;
                if !matches!(
                    name.as_str(),
                    "SMOOTH"
                        | "SMOOTHI"
                        | "SMOOTH3"
                        | "SMOOTH3I"
                        | "DELAY1"
                        | "DELAY1I"
                        | "DELAY3"
                        | "DELAY3I"
                        | "TREND"
                        | "NPV"
                        | "IN"
                        | "INI"
                        | "OUTPUT"
                ) {
                    participant_helper_names += 1;
                }
                helper_list.push(name.clone());
                continue;
            }
            if VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
                builtin_names += 1;
                continue;
            }
            if is_probable_lookup_table_name(name) {
                lookupish += 1;
            }
            visible += 1;
        }

        eprintln!("\n=== VDF participant category counts: {label} ===");
        eprintln!("  names total={}", vdf.names.len());
        eprintln!("  visible={visible} lookupish={lookupish}");
        eprintln!("  hash_names={hash_names} helper_names={helper_names}");
        eprintln!("  participant_helper_names={participant_helper_names}");
        eprintln!("  builtin_names={builtin_names} other_metadata={other_metadata}");
        eprintln!("  OT capacity(excluding time)={}", vdf.offset_table_count.saturating_sub(1));
        eprintln!(
            "  visible + hash + helper={}",
            visible + hash_names + helper_names
        );
        eprintln!(
            "  visible - lookupish + hash + helper={}",
            visible.saturating_sub(lookupish) + hash_names + helper_names
        );
        eprintln!(
            "  visible - lookupish + hash + participant_helpers={}",
            visible.saturating_sub(lookupish) + hash_names + participant_helper_names
        );
        helper_list.sort();
        eprintln!("  helper names={helper_list:?}");
    }
}

#[test]
#[ignore]
fn test_debug_direct_stdlib_alias_section6_context() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let vdf = vdf_file(vdf_path);

        let mut slot_by_name: HashMap<String, u32> = HashMap::new();
        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            if let Some(name) = vdf.names.get(i) {
                slot_by_name.insert(normalize_vdf_name(name), slot_ref);
            }
        }
        let slot_name: HashMap<u32, String> = vdf
            .slot_table
            .iter()
            .enumerate()
            .filter_map(|(i, &slot_ref)| vdf.names.get(i).map(|name| (slot_ref, name.clone())))
            .collect();
        let (_, entries, _) = vdf.parse_section6_ref_stream().unwrap();

        eprintln!("\n=== direct stdlib alias section6 context: {label} ===");
        for var in &model.variables {
            let (ident, equation) = match var {
                crate::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
                crate::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
                _ => continue,
            };
            let Some(info) = extract_stdlib_call_info(equation) else {
                continue;
            };
            let normalized = normalize_vdf_name(ident);
            let Some(&slot_ref) = slot_by_name.get(&normalized) else {
                continue;
            };

            let refs_with_alias: Vec<Vec<String>> = entries
                .iter()
                .filter(|entry| entry.refs.contains(&slot_ref))
                .map(|entry| {
                    entry
                        .refs
                        .iter()
                        .map(|r| {
                            slot_name
                                .get(r)
                                .map(|name| format!("{r}:{name}"))
                                .unwrap_or_else(|| format!("{r}:<sec1>"))
                        })
                        .collect()
                })
                .collect();

            eprintln!("  {} via {}", ident, info.func_name);
            eprintln!("    slot={slot_ref}");
            for (sig, is_stock) in info.vensim_signatures() {
                let flavor = if is_stock { "stock" } else { "non-stock" };
                eprintln!("    {flavor:>9} sig={sig}");
            }
            if refs_with_alias.is_empty() {
                eprintln!("    section6 refs: none");
            } else {
                for refs in refs_with_alias.iter().take(8) {
                    eprintln!("    section6 refs={refs:?}");
                }
                if refs_with_alias.len() > 8 {
                    eprintln!("    ... ({} more entries)", refs_with_alias.len() - 8);
                }
            }
        }
    }
}

#[test]
#[ignore]
fn test_debug_visible_stocklike_feature_buckets() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let vdf = vdf_file(vdf_path);

        let mut stocklike_names = HashSet::new();
        for var in &model.variables {
            match var {
                crate::datamodel::Variable::Stock(s) => {
                    stocklike_names.insert(normalize_vdf_name(&s.ident));
                }
                crate::datamodel::Variable::Aux(a) => {
                    if extract_stdlib_call_info(&a.equation).is_some_and(|info| info.output_is_stock()) {
                        stocklike_names.insert(normalize_vdf_name(&a.ident));
                    }
                }
                crate::datamodel::Variable::Flow(f) => {
                    if extract_stdlib_call_info(&f.equation).is_some_and(|info| info.output_is_stock()) {
                        stocklike_names.insert(normalize_vdf_name(&f.ident));
                    }
                }
                crate::datamodel::Variable::Module(_) => {}
            }
        }

        let sec6_ref_set: HashSet<u32> = vdf
            .parse_section6_ref_stream()
            .unwrap()
            .1
            .into_iter()
            .flat_map(|entry| entry.refs)
            .collect();
        let slot_words = debug_visible_slot_words(&vdf);

        let mut stock_word1 = std::collections::BTreeMap::<u32, usize>::new();
        let mut nonstock_word1 = std::collections::BTreeMap::<u32, usize>::new();
        let mut stock_tuple = HashMap::<[u32; 4], usize>::new();
        let mut nonstock_tuple = HashMap::<[u32; 4], usize>::new();
        let mut stock_sec6 = 0usize;
        let mut nonstock_sec6 = 0usize;
        let mut stock_examples = Vec::new();
        let mut nonstock_examples = Vec::new();

        for (normalized, words) in slot_words {
            if is_probable_lookup_table_name(&normalized) {
                continue;
            }
            let is_stocklike = stocklike_names.contains(&normalized);
            let word1 = words.words[1];
            if is_stocklike {
                *stock_word1.entry(word1).or_default() += 1;
                *stock_tuple.entry(words.words).or_default() += 1;
                stock_sec6 += usize::from(sec6_ref_set.contains(&words.slot_ref));
                if stock_examples.len() < 24 {
                    stock_examples.push((normalized, words.slot_ref, words.words));
                }
            } else {
                *nonstock_word1.entry(word1).or_default() += 1;
                *nonstock_tuple.entry(words.words).or_default() += 1;
                nonstock_sec6 += usize::from(sec6_ref_set.contains(&words.slot_ref));
                if nonstock_examples.len() < 24 {
                    nonstock_examples.push((normalized, words.slot_ref, words.words));
                }
            }
        }

        let top_word1 = |map: &std::collections::BTreeMap<u32, usize>| {
            let mut rows: Vec<_> = map.iter().map(|(&word, &count)| (count, word)).collect();
            rows.sort_by(|a, b| b.cmp(a));
            rows
        };
        let top_tuple = |map: &HashMap<[u32; 4], usize>| {
            let mut rows: Vec<_> = map.iter().map(|(&words, &count)| (count, words)).collect();
            rows.sort_by(|a, b| b.cmp(a));
            rows
        };

        eprintln!("\n=== visible stocklike feature buckets: {label} ===");
        eprintln!(
            "  stocklike={} nonstocklike={}",
            stock_word1.values().sum::<usize>(),
            nonstock_word1.values().sum::<usize>()
        );
        eprintln!("  stocklike with section6 ref={stock_sec6}");
        eprintln!("  nonstocklike with section6 ref={nonstock_sec6}");
        eprintln!("  top stocklike word[1] values:");
        for (count, word) in top_word1(&stock_word1).into_iter().take(12) {
            eprintln!("    count={count:>3} word1=0x{word:08x}");
        }
        eprintln!("  top nonstocklike word[1] values:");
        for (count, word) in top_word1(&nonstock_word1).into_iter().take(12) {
            eprintln!("    count={count:>3} word1=0x{word:08x}");
        }
        eprintln!("  top stocklike slot tuples:");
        for (count, words) in top_tuple(&stock_tuple).into_iter().take(12) {
            eprintln!(
                "    count={count:>3} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}]",
                words[0], words[1], words[2], words[3]
            );
        }
        eprintln!("  top nonstocklike slot tuples:");
        for (count, words) in top_tuple(&nonstock_tuple).into_iter().take(12) {
            eprintln!(
                "    count={count:>3} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}]",
                words[0], words[1], words[2], words[3]
            );
        }
        eprintln!("  stocklike examples:");
        for (name, slot_ref, words) in stock_examples.iter().take(16) {
            eprintln!(
                "    slot={slot_ref:>5} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}] {name}",
                words[0], words[1], words[2], words[3]
            );
        }
        eprintln!("  nonstocklike examples:");
        for (name, slot_ref, words) in nonstock_examples.iter().take(16) {
            eprintln!(
                "    slot={slot_ref:>5} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}] {name}",
                words[0], words[1], words[2], words[3]
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_projected_visible_results_vs_direct_empirical() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = crate::Project::from(datamodel_project.clone());
        let sim_project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&sim_project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let predicted = vdf
            .build_stocks_first_ot_map_for_project(&project, "main")
            .unwrap();
        let empirical = debug_empirical_direct_visible_map(&vdf, &results);

        let mut projected = HashMap::<String, usize>::new();
        for (i, _slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || SYSTEM_NAMES.contains(&name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || is_probable_lookup_table_name(name)
            {
                continue;
            }
            let id = Ident::<Canonical>::new(name);
            if let Some(&ot) = predicted.get(&id) {
                projected.insert(normalize_vdf_name(name), ot);
            }
        }

        let mut correct = 0usize;
        let mut wrong = 0usize;
        let mut missing = 0usize;
        for (name, emp_ot) in &empirical {
            match projected.get(name) {
                Some(pred_ot) if pred_ot == emp_ot => correct += 1,
                Some(_) => wrong += 1,
                None => missing += 1,
            }
        }

        eprintln!("\n=== projected visible results vs direct empirical: {label} ===");
        eprintln!("  projected={} empirical={}", projected.len(), empirical.len());
        eprintln!("  correct={correct} wrong={wrong} missing={missing}");
        if correct + wrong > 0 {
            eprintln!(
                "  accuracy={:.1}%",
                100.0 * correct as f64 / (correct + wrong) as f64
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_slot_word_patterns_for_saved_names() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let sec1 = vdf.slot_section().unwrap();
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut saved_patterns: HashMap<[u32; 4], usize> = HashMap::new();
        let mut unsaved_patterns: HashMap<[u32; 4], usize> = HashMap::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }

            let base = sec1.data_offset() + slot_ref as usize;
            if base + 16 > sec1.region_end || base + 16 > vdf.data.len() {
                continue;
            }
            let words = [
                read_u32(&vdf.data, base),
                read_u32(&vdf.data, base + 4),
                read_u32(&vdf.data, base + 8),
                read_u32(&vdf.data, base + 12),
            ];

            let bucket = if saved_names.contains(&normalize_vdf_name(name)) {
                &mut saved_patterns
            } else {
                &mut unsaved_patterns
            };
            *bucket.entry(words).or_default() += 1;
        }

        let top = |patterns: &HashMap<[u32; 4], usize>| {
            let mut items: Vec<_> = patterns
                .iter()
                .map(|(words, count)| (*words, *count))
                .collect();
            items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            items
        };

        eprintln!("\n=== slot word patterns for saved names: {label} ===");
        eprintln!("  top saved patterns:");
        for (words, count) in top(&saved_patterns).into_iter().take(12) {
            eprintln!(
                "    {:>4} [{:08x} {:08x} {:08x} {:08x}]",
                count, words[0], words[1], words[2], words[3]
            );
        }
        eprintln!("  top unsaved patterns:");
        for (words, count) in top(&unsaved_patterns).into_iter().take(12) {
            eprintln!(
                "    {:>4} [{:08x} {:08x} {:08x} {:08x}]",
                count, words[0], words[1], words[2], words[3]
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_section6_refs() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let (_skip, entries, _stop) = vdf.parse_section6_ref_stream().unwrap();
        let ref_set: HashSet<u32> = entries
            .iter()
            .flat_map(|entry| entry.refs.iter().copied())
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut saved_with_ref = 0usize;
        let mut saved_without_ref = Vec::new();
        let mut unsaved_with_ref = Vec::new();
        let mut total_saved = 0usize;
        let mut total_unsaved = 0usize;

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }

            let has_ref = ref_set.contains(&slot_ref);
            let is_saved = saved_names.contains(&normalize_vdf_name(name));
            if is_saved {
                total_saved += 1;
                if has_ref {
                    saved_with_ref += 1;
                } else {
                    saved_without_ref.push(name.clone());
                }
            } else {
                total_unsaved += 1;
                if has_ref {
                    unsaved_with_ref.push(name.clone());
                }
            }
        }

        saved_without_ref.sort();
        unsaved_with_ref.sort();

        eprintln!("\n=== saved names vs section6 refs: {label} ===");
        eprintln!("  saved with ref: {saved_with_ref}/{total_saved}");
        eprintln!(
            "  unsaved with ref: {}/{}",
            unsaved_with_ref.len(),
            total_unsaved
        );
        eprintln!("  first saved without ref:");
        for name in saved_without_ref.iter().take(30) {
            eprintln!("    {name}");
        }
        eprintln!("  first unsaved with ref:");
        for name in unsaved_with_ref.iter().take(30) {
            eprintln!("    {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_section6_ref_roles() {
    #[derive(Clone, Copy, Default)]
    struct RefStats {
        total: usize,
        first: usize,
        last: usize,
        singleton: usize,
    }

    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let (_, entries, _) = vdf.parse_section6_ref_stream().unwrap();
        let mut stats_by_slot: HashMap<u32, RefStats> = HashMap::new();
        for entry in &entries {
            if let Some(&first) = entry.refs.first() {
                stats_by_slot.entry(first).or_default().first += 1;
            }
            if let Some(&last) = entry.refs.last() {
                stats_by_slot.entry(last).or_default().last += 1;
            }
            if entry.refs.len() == 1 {
                stats_by_slot.entry(entry.refs[0]).or_default().singleton += 1;
            }
            for &slot_ref in &entry.refs {
                stats_by_slot.entry(slot_ref).or_default().total += 1;
            }
        }

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut saved_bucket_counts: HashMap<&'static str, usize> = HashMap::new();
        let mut unsaved_bucket_counts: HashMap<&'static str, usize> = HashMap::new();
        let mut saved_notable = Vec::new();
        let mut unsaved_notable = Vec::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }

            let stats = stats_by_slot.get(&slot_ref).cloned().unwrap_or_default();
            let bucket = match stats.total {
                0 => "total=0",
                1 => "total=1",
                2..=3 => "total=2-3",
                4..=7 => "total=4-7",
                _ => "total=8+",
            };

            let target = if saved_names.contains(&normalize_vdf_name(name)) {
                &mut saved_bucket_counts
            } else {
                &mut unsaved_bucket_counts
            };
            *target.entry(bucket).or_default() += 1;

            if saved_names.contains(&normalize_vdf_name(name)) && stats.total == 0 {
                saved_notable.push((name.clone(), slot_ref, stats.total, stats.first, stats.last));
            }
            if !saved_names.contains(&normalize_vdf_name(name)) && stats.total >= 4 {
                unsaved_notable.push((
                    name.clone(),
                    slot_ref,
                    stats.total,
                    stats.first,
                    stats.last,
                ));
            }
        }

        saved_notable.sort();
        unsaved_notable.sort();

        eprintln!("\n=== saved names vs section6 ref roles: {label} ===");
        eprintln!("  saved buckets:");
        for bucket in ["total=0", "total=1", "total=2-3", "total=4-7", "total=8+"] {
            eprintln!(
                "    {bucket:>9}: {}",
                saved_bucket_counts.get(bucket).copied().unwrap_or(0)
            );
        }
        eprintln!("  unsaved buckets:");
        for bucket in ["total=0", "total=1", "total=2-3", "total=4-7", "total=8+"] {
            eprintln!(
                "    {bucket:>9}: {}",
                unsaved_bucket_counts.get(bucket).copied().unwrap_or(0)
            );
        }
        eprintln!("  saved with zero section6 refs:");
        for (name, slot_ref, total, first, last) in saved_notable.iter().take(25) {
            eprintln!("    {name} slot={slot_ref} total={total} first={first} last={last}");
        }
        eprintln!("  unsaved with heavy section6 refs:");
        for (name, slot_ref, total, first, last) in unsaved_notable.iter().take(25) {
            eprintln!("    {name} slot={slot_ref} total={total} first={first} last={last}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_model_guided_vs_empirical() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();
        let empirical = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let predicted = vdf.build_stocks_first_ot_map_for_project(&project, "main").unwrap();

        let mut correct = 0usize;
        let mut wrong = 0usize;
        let mut missing = 0usize;
        for (name, &emp_ot) in &empirical {
            if name.as_str() == "time" {
                continue;
            }
            match predicted.get(name) {
                Some(&pred_ot) if pred_ot == emp_ot => correct += 1,
                Some(_) => wrong += 1,
                None => missing += 1,
            }
        }

        eprintln!("\n=== model-guided vs empirical: {label} ===");
        eprintln!("  predicted entries: {}", predicted.len());
        eprintln!("  empirical entries: {}", empirical.len());
        eprintln!("  correct={correct} wrong={wrong} missing={missing}");
        if correct + wrong > 0 {
            eprintln!(
                "  accuracy={:.1}%",
                100.0 * correct as f64 / (correct + wrong) as f64
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_stdlib_member_empirical_ots() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let direct_calls = {
            let model = datamodel_project
                .models
                .iter()
                .find(|m| m.name == "main")
                .unwrap();
            collect_direct_stdlib_calls(model)
        };
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let empirical = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();

        eprintln!("\n=== stdlib member empirical OTs: {label} ===");
        for (ident, info) in direct_calls {
            let Some(module_name) = info.compiled_stdlib_module_name() else {
                continue;
            };
            let prefix = format!("$⁚{ident}⁚0⁚{module_name}.");
            let mut members: Vec<_> = empirical
                .iter()
                .filter(|(name, _)| name.as_str().starts_with(&prefix))
                .map(|(name, &ot)| (name.as_str().to_string(), ot))
                .collect();
            if members.is_empty() {
                continue;
            }
            members.sort_by_key(|(_, ot)| *ot);

            eprintln!("  {ident} via {}:", info.func_name);
            eprintln!("    VDF signatures:");
            for (sig, is_stock) in info.vensim_signatures() {
                let flavor = if is_stock { "stock" } else { "non-stock" };
                eprintln!("      {flavor:>9}  {sig}");
            }
            eprintln!("    empirical compiled members:");
            for (name, ot) in members {
                eprintln!("      OT[{ot:>3}] {name}");
            }
        }
    }
}

#[test]
#[ignore]
fn test_debug_visible_name_order_hypotheses() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let empirical = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
        let codes = vdf.section6_ot_class_codes().unwrap();
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();

        let mut saved_visible = Vec::<(String, usize, usize, u32, bool)>::new();
        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }
            let canonical = Ident::<Canonical>::new(name);
            let Some(&ot) = empirical.get(&canonical) else {
                continue;
            };
            saved_visible.push((
                name.clone(),
                i,
                ot,
                slot_ref,
                codes.get(ot).copied() == Some(VDF_SECTION6_OT_CODE_STOCK),
            ));
        }

        let names: Vec<String> = saved_visible
            .iter()
            .map(|(name, ..)| name.clone())
            .collect();
        let ot_positions: Vec<usize> = saved_visible.iter().map(|(_, _, ot, ..)| *ot).collect();

        let mut alpha = names.clone();
        alpha.sort_by_key(|name| name.to_lowercase());

        let mut slot_order = saved_visible.clone();
        slot_order.sort_by_key(|(_, _, _, slot_ref, _)| *slot_ref);
        let slot_order_names: Vec<String> =
            slot_order.iter().map(|(name, ..)| name.clone()).collect();

        let mut stocks_alpha = Vec::new();
        let mut nonstocks_alpha = Vec::new();
        let mut stocks_name_order = Vec::new();
        let mut nonstocks_name_order = Vec::new();
        for (name, name_idx, _ot, _slot_ref, is_stock) in &saved_visible {
            if *is_stock {
                stocks_alpha.push(name.clone());
                stocks_name_order.push((name_idx, name.clone()));
            } else {
                nonstocks_alpha.push(name.clone());
                nonstocks_name_order.push((name_idx, name.clone()));
            }
        }
        stocks_alpha.sort_by_key(|name| name.to_lowercase());
        nonstocks_alpha.sort_by_key(|name| name.to_lowercase());
        stocks_name_order.sort_by_key(|(idx, _)| *idx);
        nonstocks_name_order.sort_by_key(|(idx, _)| *idx);
        let stocks_name_first: Vec<String> = stocks_name_order
            .into_iter()
            .map(|(_, name)| name)
            .chain(nonstocks_name_order.into_iter().map(|(_, name)| name))
            .collect();
        let stocks_alpha_first: Vec<String> = stocks_alpha
            .into_iter()
            .chain(nonstocks_alpha.into_iter())
            .collect();

        let hypothesis_positions = |order: &[String]| -> Vec<usize> {
            names
                .iter()
                .map(|name| {
                    order
                        .iter()
                        .position(|candidate| candidate == name)
                        .unwrap()
                })
                .collect()
        };

        let report = |title: &str, order: &[String]| {
            let positions = hypothesis_positions(order);
            let (tau, concordant, discordant) = kendall_tau(&positions, &ot_positions);
            eprintln!(
                "  {title:28} tau={tau:>7.4} concordant={concordant:>5} discordant={discordant:>5}"
            );
        };

        eprintln!("\n=== visible-name order hypotheses: {label} ===");
        eprintln!("  saved visible names: {}", saved_visible.len());
        report("name-table order", &names);
        report("alphabetical", &alpha);
        report("slot-ref order", &slot_order_names);
        report("stocks alpha first", &stocks_alpha_first);
        report("stocks name-order first", &stocks_name_first);
    }
}

#[test]
#[ignore]
fn test_debug_stocks_first_candidate_residuals() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project.clone()));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();
        let vdf = vdf_file(vdf_path);

        let mut candidates: Vec<String> = vdf.names[..vdf.slot_table.len()]
            .iter()
            .filter(|name| {
                !name.is_empty()
                    && !name.starts_with('.')
                    && !name.starts_with('-')
                    && !SYSTEM_NAMES.contains(&name.as_str())
            })
            .cloned()
            .collect();
        if vdf.names.len() > vdf.slot_table.len() {
            for name in &vdf.names[vdf.slot_table.len()..] {
                if name.starts_with('#') {
                    candidates.push(name.clone());
                }
            }
        }

        let mut model_stock_set: HashSet<String> = HashSet::new();
        let mut model_sig_stocks: HashSet<String> = HashSet::new();
        let mut alias_names_normalized: HashSet<String> = HashSet::new();
        for var in &model.variables {
            let (ident, equation) = match var {
                crate::datamodel::Variable::Stock(s) => {
                    model_stock_set.insert(normalize_vdf_name(&s.ident));
                    continue;
                }
                crate::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
                crate::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
                crate::datamodel::Variable::Module(_) => continue,
            };
            if let Some(info) = extract_stdlib_call_info(equation) {
                alias_names_normalized.insert(normalize_vdf_name(ident));
                for (sig, is_stock) in info.vensim_signatures() {
                    if is_stock {
                        model_sig_stocks.insert(normalize_vdf_name(&sig));
                    }
                }
            }
        }

        candidates
            .retain(|n| n.len() != 1 || n.chars().next().is_some_and(|c| c.is_alphanumeric()));
        let vensim_builtins: HashSet<&str> = VENSIM_BUILTINS.into_iter().collect();
        candidates.retain(|n| !vensim_builtins.contains(n.to_lowercase().as_str()));
        candidates.retain(|n| !is_vdf_metadata_entry(n));
        candidates.retain(|n| !alias_names_normalized.contains(&normalize_vdf_name(n)));

        let is_stock_name = |name: &str| {
            let normalized = normalize_vdf_name(name);
            model_stock_set.contains(&normalized) || model_sig_stocks.contains(&normalized)
        };

        let mut stock_names = Vec::new();
        let mut nonstock_names = Vec::new();
        for name in candidates {
            if is_stock_name(&name) {
                stock_names.push(name);
            } else {
                nonstock_names.push(name);
            }
        }
        stock_names.sort_by_key(|n| n.to_lowercase());
        nonstock_names.sort_by_key(|n| n.to_lowercase());

        let empirical = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
        let empirical_direct: HashSet<String> = empirical
            .keys()
            .filter(|id| {
                let name = id.as_str();
                name != "time" && !name.starts_with("$⁚")
            })
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let section6_codes = vdf.section6_ot_class_codes().unwrap();
        let vdf_stock_ots = section6_codes
            .iter()
            .skip(1)
            .filter(|&&code| code == VDF_SECTION6_OT_CODE_STOCK)
            .count();
        let vdf_nonstock_ots = section6_codes.len() - 1 - vdf_stock_ots;

        let residual_nonstocks: Vec<String> = nonstock_names
            .iter()
            .filter(|name| !name.starts_with('#'))
            .filter(|name| !empirical_direct.contains(&normalize_vdf_name(name)))
            .cloned()
            .collect();

        eprintln!("\n=== stocks-first candidate residuals: {label} ===");
        eprintln!(
            "  candidate stocks={} nonstocks={} ; VDF stock OTs={} nonstock OTs={}",
            stock_names.len(),
            nonstock_names.len(),
            vdf_stock_ots,
            vdf_nonstock_ots
        );
        eprintln!(
            "  direct non-# nonstock residuals not seen in empirical direct names: {}",
            residual_nonstocks.len()
        );
        for name in residual_nonstocks.iter().take(60) {
            eprintln!("    {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_section4_refs() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let empirical_map = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let (skip_words, entries, stop) = parse_debug_section4_entries(&vdf).unwrap();
        let ref_set: HashSet<u32> = entries
            .iter()
            .flat_map(|entry| entry.refs.iter().copied())
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut saved_with_ref = 0usize;
        let mut total_saved = 0usize;
        let mut unsaved_with_ref = 0usize;
        let mut total_unsaved = 0usize;
        let mut saved_without_ref = Vec::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }

            let has_ref = ref_set.contains(&slot_ref);
            if saved_names.contains(&normalize_vdf_name(name)) {
                total_saved += 1;
                if has_ref {
                    saved_with_ref += 1;
                } else {
                    saved_without_ref.push(name.clone());
                }
            } else {
                total_unsaved += 1;
                if has_ref {
                    unsaved_with_ref += 1;
                }
            }
        }

        eprintln!("\n=== saved names vs section4 refs: {label} ===");
        eprintln!(
            "  skip_words={skip_words} entries={} stop=0x{stop:08x} expected_field4={}",
            entries.len(),
            vdf.sections[4].field4
        );
        eprintln!("  saved with ref: {saved_with_ref}/{total_saved}");
        eprintln!("  unsaved with ref: {unsaved_with_ref}/{total_unsaved}");
        eprintln!("  first saved without section4 ref:");
        saved_without_ref.sort();
        for name in saved_without_ref.iter().take(25) {
            eprintln!("    {name}");
        }
        eprintln!("  first section4 entries:");
        for entry in entries.iter().take(12) {
            eprintln!(
                "    tag={} kind={} refs={:?}",
                entry.tag, entry.kind, entry.refs
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_slot_word_features() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let empirical_map = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let sec1 = vdf.slot_section().unwrap();
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut saved_by_pos = [
            HashMap::<u32, usize>::new(),
            HashMap::<u32, usize>::new(),
            HashMap::<u32, usize>::new(),
            HashMap::<u32, usize>::new(),
        ];
        let mut unsaved_by_pos = [
            HashMap::<u32, usize>::new(),
            HashMap::<u32, usize>::new(),
            HashMap::<u32, usize>::new(),
            HashMap::<u32, usize>::new(),
        ];

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            {
                continue;
            }

            let base = sec1.data_offset() + slot_ref as usize;
            if base + 16 > sec1.region_end || base + 16 > vdf.data.len() {
                continue;
            }
            let words = [
                read_u32(&vdf.data, base),
                read_u32(&vdf.data, base + 4),
                read_u32(&vdf.data, base + 8),
                read_u32(&vdf.data, base + 12),
            ];
            let target = if saved_names.contains(&normalize_vdf_name(name)) {
                &mut saved_by_pos
            } else {
                &mut unsaved_by_pos
            };
            for (pos, word) in words.into_iter().enumerate() {
                *target[pos].entry(word).or_default() += 1;
            }
        }

        let top_items = |map: &HashMap<u32, usize>| {
            let mut items: Vec<_> = map.iter().map(|(&word, &count)| (word, count)).collect();
            items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            items
        };

        eprintln!("\n=== saved names vs slot-word features: {label} ===");
        for pos in 0..4 {
            eprintln!("  word[{pos}] top saved values:");
            for (word, count) in top_items(&saved_by_pos[pos]).into_iter().take(10) {
                eprintln!("    {count:>4} 0x{word:08x}");
            }
            eprintln!("  word[{pos}] top unsaved values:");
            for (word, count) in top_items(&unsaved_by_pos[pos]).into_iter().take(10) {
                eprintln!("    {count:>4} 0x{word:08x}");
            }
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_slot_span_features() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let spans = debug_slot_spans(&vdf);
        let span_by_offset: HashMap<u32, &DebugSlotSpan<'_>> =
            spans.iter().map(|span| (span.offset, span)).collect();

        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut saved_len_counts: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        let mut unsaved_len_counts: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        let mut saved_refish = 0usize;
        let mut unsaved_refish = 0usize;
        let mut saved_ascii = 0usize;
        let mut unsaved_ascii = 0usize;
        let mut saved_examples = Vec::new();
        let mut unsaved_examples = Vec::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                continue;
            }

            let Some(span) = span_by_offset.get(&slot_ref).copied() else {
                continue;
            };
            let looks_refish = span
                .data
                .chunks_exact(4)
                .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
                .any(|word| word > 0 && word % 4 == 0 && (word as usize) < vdf.sections[1].region_data_size());
            let ascii_count = span
                .data
                .iter()
                .filter(|&&b| (0x20..=0x7e).contains(&b))
                .count();

            let target = if saved_names.contains(&normalize_vdf_name(name)) {
                &mut saved_len_counts
            } else {
                &mut unsaved_len_counts
            };
            *target.entry(span.len).or_default() += 1;

            if saved_names.contains(&normalize_vdf_name(name)) {
                saved_refish += usize::from(looks_refish);
                saved_ascii += usize::from(ascii_count > 0);
                if saved_examples.len() < 20 {
                    saved_examples.push((name.clone(), slot_ref, span.len, looks_refish, ascii_count));
                }
            } else {
                unsaved_refish += usize::from(looks_refish);
                unsaved_ascii += usize::from(ascii_count > 0);
                if unsaved_examples.len() < 20 {
                    unsaved_examples.push((name.clone(), slot_ref, span.len, looks_refish, ascii_count));
                }
            }
        }

        eprintln!("\n=== saved names vs slot-span features: {label} ===");
        eprintln!("  saved span lengths:");
        for (len, count) in saved_len_counts.iter().take(16) {
            eprintln!("    len={len:>3} count={count:>3}");
        }
        eprintln!("  unsaved span lengths:");
        for (len, count) in unsaved_len_counts.iter().take(16) {
            eprintln!("    len={len:>3} count={count:>3}");
        }
        eprintln!("  saved spans with ref-like word: {saved_refish}");
        eprintln!("  unsaved spans with ref-like word: {unsaved_refish}");
        eprintln!("  saved spans with printable ascii: {saved_ascii}");
        eprintln!("  unsaved spans with printable ascii: {unsaved_ascii}");
        eprintln!("  first saved examples:");
        for (name, slot_ref, len, looks_refish, ascii_count) in saved_examples.iter().take(12) {
            eprintln!(
                "    {name} slot={slot_ref} len={len} refish={looks_refish} ascii={ascii_count}"
            );
        }
        eprintln!("  first unsaved examples:");
        for (name, slot_ref, len, looks_refish, ascii_count) in unsaved_examples.iter().take(12) {
            eprintln!(
                "    {name} slot={slot_ref} len={len} refish={looks_refish} ascii={ascii_count}"
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_filewide_slot_ref_counts() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let slot_set: HashSet<u32> = vdf.slot_table.iter().copied().collect();
        let mut total_counts: HashMap<u32, usize> = HashMap::new();
        let mut section_counts: Vec<HashMap<u32, usize>> = vec![HashMap::new(); vdf.sections.len()];

        for (sec_idx, sec) in vdf.sections.iter().enumerate() {
            let mut start = sec.data_offset();
            let end = sec.region_end.min(vdf.data.len());
            if start >= end {
                continue;
            }

            // Skip the slot-table words and the raw name table bytes; we only
            // want secondary references to section-1 slot offsets.
            if sec_idx == 2 {
                continue;
            }
            if sec_idx == 1 {
                start = vdf.slot_table_offset.saturating_add(vdf.slot_table.len() * 4);
                if start >= end {
                    continue;
                }
            }

            let mut pos = start;
            while pos + 4 <= end {
                let word = read_u32(&vdf.data, pos);
                if slot_set.contains(&word) {
                    *total_counts.entry(word).or_default() += 1;
                    *section_counts[sec_idx].entry(word).or_default() += 1;
                }
                pos += 4;
            }
        }

        let mut saved_total_buckets: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        let mut unsaved_total_buckets: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        let mut saved_zero = Vec::new();
        let mut unsaved_heavy = Vec::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                continue;
            }

            let total = total_counts.get(&slot_ref).copied().unwrap_or(0);
            let buckets = if saved_names.contains(&normalize_vdf_name(name)) {
                &mut saved_total_buckets
            } else {
                &mut unsaved_total_buckets
            };
            *buckets.entry(total).or_default() += 1;

            if saved_names.contains(&normalize_vdf_name(name)) && total == 0 {
                saved_zero.push(name.clone());
            }
            if !saved_names.contains(&normalize_vdf_name(name)) && total >= 3 {
                unsaved_heavy.push((name.clone(), total));
            }
        }

        unsaved_heavy.sort_by(|a, b| a.0.cmp(&b.0));
        saved_zero.sort();

        eprintln!("\n=== saved names vs filewide slot-ref counts: {label} ===");
        eprintln!("  saved total-count buckets:");
        for (count, freq) in saved_total_buckets.iter().take(16) {
            eprintln!("    total={count:>2} freq={freq:>3}");
        }
        eprintln!("  unsaved total-count buckets:");
        for (count, freq) in unsaved_total_buckets.iter().take(16) {
            eprintln!("    total={count:>2} freq={freq:>3}");
        }
        for (sec_idx, counts) in section_counts.iter().enumerate() {
            let saved_hits = vdf
                .slot_table
                .iter()
                .enumerate()
                .filter_map(|(i, &slot_ref)| {
                    let name = vdf.names.get(i)?;
                    let saved = saved_names.contains(&normalize_vdf_name(name));
                    let count = counts.get(&slot_ref).copied().unwrap_or(0);
                    Some((saved, count))
                })
                .fold((0usize, 0usize), |(saved_sum, unsaved_sum), (saved, count)| {
                    if saved {
                        (saved_sum + count, unsaved_sum)
                    } else {
                        (saved_sum, unsaved_sum + count)
                    }
                });
            if saved_hits.0 == 0 && saved_hits.1 == 0 {
                continue;
            }
            eprintln!(
                "  section[{sec_idx}] refs: saved_sum={} unsaved_sum={}",
                saved_hits.0, saved_hits.1
            );
        }
        eprintln!("  first saved with zero total refs:");
        for name in saved_zero.iter().take(20) {
            eprintln!("    {name}");
        }
        eprintln!("  first unsaved with heavy total refs:");
        for (name, total) in unsaved_heavy.iter().take(20) {
            eprintln!("    {name} ({total})");
        }
    }
}

#[test]
#[ignore]
fn test_debug_saved_names_vs_slot_word_bits() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let sec1_data_start = vdf.sections[1].data_offset();
        let mut samples: Vec<([u32; 4], bool)> = Vec::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                continue;
            }

            let abs = sec1_data_start + slot_ref as usize;
            if abs + 16 > vdf.data.len() {
                continue;
            }
            let words = [
                read_u32(&vdf.data, abs),
                read_u32(&vdf.data, abs + 4),
                read_u32(&vdf.data, abs + 8),
                read_u32(&vdf.data, abs + 12),
            ];
            samples.push((words, saved_names.contains(&normalize_vdf_name(name))));
        }

        let total = samples.len();
        let positives = samples.iter().filter(|(_, saved)| *saved).count();
        let baseline = positives.max(total - positives);

        #[derive(Debug)]
        struct BitScore {
            word: usize,
            bit: usize,
            predict_saved_when_set: bool,
            correct: usize,
            true_pos: usize,
            false_pos: usize,
            false_neg: usize,
        }

        let mut scores = Vec::new();
        for word in 0..4usize {
            for bit in 0..32usize {
                for predict_saved_when_set in [true, false] {
                    let mut correct = 0usize;
                    let mut true_pos = 0usize;
                    let mut false_pos = 0usize;
                    let mut false_neg = 0usize;

                    for (words, saved) in &samples {
                        let bit_set = (words[word] >> bit) & 1 == 1;
                        let predicted = if predict_saved_when_set { bit_set } else { !bit_set };
                        if predicted == *saved {
                            correct += 1;
                        }
                        match (predicted, *saved) {
                            (true, true) => true_pos += 1,
                            (true, false) => false_pos += 1,
                            (false, true) => false_neg += 1,
                            (false, false) => {}
                        }
                    }

                    scores.push(BitScore {
                        word,
                        bit,
                        predict_saved_when_set,
                        correct,
                        true_pos,
                        false_pos,
                        false_neg,
                    });
                }
            }
        }

        scores.sort_by(|a, b| {
            b.correct
                .cmp(&a.correct)
                .then_with(|| a.false_pos.cmp(&b.false_pos))
                .then_with(|| a.false_neg.cmp(&b.false_neg))
        });

        eprintln!("\n=== saved names vs slot-word bits: {label} ===");
        eprintln!(
            "  samples={} positives={} baseline_majority_correct={}",
            total, positives, baseline
        );
        for score in scores.iter().take(16) {
            let polarity = if score.predict_saved_when_set {
                "set=>saved"
            } else {
                "clear=>saved"
            };
            eprintln!(
                "  w[{}].bit{:>2} {:>10} correct={} tp={} fp={} fn={}",
                score.word,
                score.bit,
                polarity,
                score.correct,
                score.true_pos,
                score.false_pos,
                score.false_neg
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_section6_entry_order_vs_direct_visible_ot_order() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let (_skip, entries, _stop) = vdf.parse_section6_ref_stream().unwrap();

        let mut slot_to_name: HashMap<u32, &str> = HashMap::new();
        for (i, &slot) in vdf.slot_table.iter().enumerate() {
            if let Some(name) = vdf.names.get(i) {
                slot_to_name.entry(slot).or_insert(name.as_str());
            }
        }

        let mut section6_index_by_name: HashMap<String, usize> = HashMap::new();
        for (entry_idx, entry) in entries.iter().enumerate() {
            for slot_ref in &entry.refs {
                let Some(name) = slot_to_name.get(slot_ref).copied() else {
                    continue;
                };
                if name.starts_with('#') {
                    continue;
                }
                section6_index_by_name
                    .entry(normalize_vdf_name(name))
                    .or_insert(entry_idx);
            }
        }

        let mut direct_empirical: Vec<(String, usize, usize)> = empirical
            .iter()
            .filter_map(|(id, &ot)| {
                let normalized = normalize_vdf_name(id.as_str());
                let entry_idx = section6_index_by_name.get(&normalized).copied()?;
                Some((id.as_str().to_string(), ot, entry_idx))
            })
            .collect();
        direct_empirical.sort_by_key(|(_, ot, _)| *ot);

        let mut monotonic_pairs = 0usize;
        let mut compared_pairs = 0usize;
        for pair in direct_empirical.windows(2) {
            compared_pairs += 1;
            if pair[0].2 <= pair[1].2 {
                monotonic_pairs += 1;
            }
        }

        eprintln!("\n=== section6 entry order vs direct visible OT order: {label} ===");
        eprintln!(
            "  covered direct names: {} of {}",
            direct_empirical.len(),
            empirical.len()
        );
        eprintln!(
            "  adjacent monotonic pairs: {}/{} ({:.1}%)",
            monotonic_pairs,
            compared_pairs,
            if compared_pairs == 0 {
                0.0
            } else {
                (monotonic_pairs as f64 / compared_pairs as f64) * 100.0
            }
        );
        eprintln!("  first direct names by OT:");
        for (name, ot, entry_idx) in direct_empirical.iter().take(30) {
            eprintln!("    OT[{ot:>3}] section6[{entry_idx:>3}] {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_section6_tail_shape_and_refs() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let sec = &vdf.sections[6];
        let first_word = read_u32(&vdf.data, sec.data_offset()) as usize;
        let (_skip, _entries, stop) = vdf.parse_section6_ref_stream().unwrap();
        let codes_end = stop + vdf.offset_table_count;
        let tail_len = sec.region_end.min(vdf.data.len()).saturating_sub(codes_end);
        let tail = &vdf.data[codes_end..sec.region_end.min(vdf.data.len())];

        let slot_set: HashSet<u32> = vdf.slot_table.iter().copied().collect();
        let mut tail_slot_counts: HashMap<u32, usize> = HashMap::new();
        let mut tail_words = Vec::new();
        for pos in (0..tail.len().saturating_sub(3)).step_by(4) {
            let word = read_u32(tail, pos);
            tail_words.push(word);
            if slot_set.contains(&word) {
                *tail_slot_counts.entry(word).or_default() += 1;
            }
        }

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut saved_tail_hits = 0usize;
        let mut unsaved_tail_hits = 0usize;
        let mut saved_without_tail = Vec::new();
        let mut unsaved_with_tail = Vec::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                continue;
            }

            let hits = tail_slot_counts.get(&slot_ref).copied().unwrap_or(0);
            if saved_names.contains(&normalize_vdf_name(name)) {
                if hits > 0 {
                    saved_tail_hits += 1;
                } else {
                    saved_without_tail.push(name.clone());
                }
            } else if hits > 0 {
                unsaved_tail_hits += 1;
                unsaved_with_tail.push((name.clone(), hits));
            }
        }

        unsaved_with_tail.sort_by(|a, b| a.0.cmp(&b.0));
        saved_without_tail.sort();

        eprintln!("\n=== section6 tail shape and refs: {label} ===");
        eprintln!("  first_word={first_word} tail_len={tail_len} codes_end=0x{codes_end:08x}");
        eprintln!("  first_word_matches_tail_len={}", first_word == tail_len);
        eprintln!("  tail u32 words={}", tail_words.len());
        eprintln!(
            "  tail slotted-name hits: saved_names_with_hit={} unsaved_names_with_hit={}",
            saved_tail_hits, unsaved_tail_hits
        );
        eprintln!("  first 24 tail words:");
        for (i, word) in tail_words.iter().take(24).enumerate() {
            eprintln!("    {:>2}: 0x{word:08x} ({})", i, word);
        }
        eprintln!("  first saved without tail ref:");
        for name in saved_without_tail.iter().take(20) {
            eprintln!("    {name}");
        }
        eprintln!("  first unsaved with tail ref:");
        for (name, hits) in unsaved_with_tail.iter().take(20) {
            eprintln!("    {name} ({hits})");
        }
    }
}

#[test]
#[ignore]
fn test_debug_section6_tail_suffix_name_index_hits() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let saved_names: HashSet<String> = empirical
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let sec = &vdf.sections[6];
        let (_skip, _entries, stop) = vdf.parse_section6_ref_stream().unwrap();
        let codes_end = stop + vdf.offset_table_count;
        let tail = &vdf.data[codes_end..sec.region_end.min(vdf.data.len())];
        let ot_value_bytes = vdf.offset_table_count * 4;
        let suffix = tail.get(ot_value_bytes..).unwrap_or(&[]);

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut suffix_index_hits: HashMap<usize, usize> = HashMap::new();
        for pos in (0..suffix.len().saturating_sub(3)).step_by(4) {
            let word = read_u32(suffix, pos) as usize;
            if word < vdf.slot_table.len() {
                *suffix_index_hits.entry(word).or_default() += 1;
            }
        }

        let mut saved_hit = 0usize;
        let mut unsaved_hit = 0usize;
        let mut saved_without = Vec::new();
        let mut unsaved_with = Vec::new();

        for i in 0..vdf.slot_table.len() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with('#')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                continue;
            }

            let hits = suffix_index_hits.get(&i).copied().unwrap_or(0);
            if saved_names.contains(&normalize_vdf_name(name)) {
                if hits > 0 {
                    saved_hit += 1;
                } else {
                    saved_without.push(name.clone());
                }
            } else if hits > 0 {
                unsaved_hit += 1;
                unsaved_with.push((name.clone(), hits));
            }
        }

        unsaved_with.sort_by(|a, b| a.0.cmp(&b.0));
        saved_without.sort();

        eprintln!("\n=== section6 tail suffix name-index hits: {label} ===");
        eprintln!(
            "  suffix_bytes={} index_hits(saved/unsaved)={}/{}",
            suffix.len(),
            saved_hit,
            unsaved_hit
        );
        eprintln!("  first 32 suffix words:");
        for pos in (0..suffix.len().saturating_sub(3)).step_by(4).take(32) {
            let word = read_u32(suffix, pos);
            let as_float = f32::from_le_bytes(word.to_le_bytes());
            eprintln!(
                "    word[{:>2}] 0x{:08x} u32={} f32={}",
                pos / 4,
                word,
                word,
                as_float,
            );
        }
        eprintln!("  first saved without suffix index hit:");
        for name in saved_without.iter().take(20) {
            eprintln!("    {name}");
        }
        eprintln!("  first unsaved with suffix index hit:");
        for (name, hits) in unsaved_with.iter().take(20) {
            eprintln!("    {name} ({hits})");
        }
    }
}

#[test]
#[ignore]
fn test_debug_section6_tail_suffix_ot_index_hits() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical = build_empirical_ot_map(&vdf_data, &results).unwrap();

        let sec = &vdf.sections[6];
        let (_skip, _entries, stop) = vdf.parse_section6_ref_stream().unwrap();
        let codes_end = stop + vdf.offset_table_count;
        let tail = &vdf.data[codes_end..sec.region_end.min(vdf.data.len())];
        let ot_value_bytes = vdf.offset_table_count * 4;
        let suffix = tail.get(ot_value_bytes..).unwrap_or(&[]);

        let mut suffix_ot_hits: HashMap<usize, usize> = HashMap::new();
        for pos in (0..suffix.len().saturating_sub(3)).step_by(4) {
            let word = read_u32(suffix, pos) as usize;
            if word < vdf.offset_table_count {
                *suffix_ot_hits.entry(word).or_default() += 1;
            }
        }

        let mut direct_visible_with_hit = Vec::new();
        let mut direct_visible_without_hit = Vec::new();
        for (id, &ot) in &empirical {
            if id.as_str().starts_with('#') {
                continue;
            }
            let hits = suffix_ot_hits.get(&ot).copied().unwrap_or(0);
            if hits > 0 {
                direct_visible_with_hit.push((id.as_str().to_string(), ot, hits));
            } else {
                direct_visible_without_hit.push((id.as_str().to_string(), ot));
            }
        }

        direct_visible_with_hit.sort_by_key(|(_, ot, _)| *ot);
        direct_visible_without_hit.sort_by_key(|(_, ot)| *ot);

        eprintln!("\n=== section6 tail suffix OT-index hits: {label} ===");
        eprintln!(
            "  suffix_bytes={} unique_ot_hits={} direct_visible_with_hit={}/{}",
            suffix.len(),
            suffix_ot_hits.len(),
            direct_visible_with_hit.len(),
            empirical.iter().filter(|(id, _)| !id.as_str().starts_with('#')).count()
        );
        eprintln!("  first visible names with OT hit:");
        for (name, ot, hits) in direct_visible_with_hit.iter().take(30) {
            eprintln!("    OT[{ot:>3}] {name} ({hits})");
        }
        eprintln!("  first visible names without OT hit:");
        for (name, ot) in direct_visible_without_hit.iter().take(30) {
            eprintln!("    OT[{ot:>3}] {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_wrld3_experiment_overlap() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end().unwrap();

    let scen = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");
    let exp = vdf_file("../../test/metasd/WRLD3-03/experiment.vdf");

    let scen_visible = debug_visible_vdf_names(&scen);
    let exp_visible = debug_visible_vdf_names(&exp);
    let scen_empirical = debug_empirical_direct_visible_map(&scen, &results);
    let exp_empirical = debug_empirical_direct_visible_map(&exp, &results);

    let visible_overlap: HashSet<String> = scen_visible.intersection(&exp_visible).cloned().collect();
    let empirical_overlap: HashSet<String> = scen_empirical
        .keys()
        .filter(|name| exp_empirical.contains_key(*name))
        .cloned()
        .collect();

    let mut same_ot = Vec::new();
    let mut diff_ot = Vec::new();
    for name in &empirical_overlap {
        let scen_ot = scen_empirical[name];
        let exp_ot = exp_empirical[name];
        if scen_ot == exp_ot {
            same_ot.push((name.clone(), scen_ot));
        } else {
            diff_ot.push((name.clone(), scen_ot, exp_ot));
        }
    }
    same_ot.sort_by_key(|(_, ot)| *ot);
    diff_ot.sort_by(|a, b| a.0.cmp(&b.0));

    let scen_display: Vec<usize> = scen
        .section6_display_records()
        .unwrap()
        .into_iter()
        .map(|rec| rec.ot_index())
        .collect();
    let exp_display: Vec<usize> = exp
        .section6_display_records()
        .unwrap()
        .into_iter()
        .map(|rec| rec.ot_index())
        .collect();

    let scen_ref_names: HashSet<String> = scen
        .parse_section6_ref_stream()
        .unwrap()
        .1
        .iter()
        .flat_map(|entry| entry.refs.iter())
        .filter_map(|slot_ref| {
            scen.slot_table
                .iter()
                .position(|s| s == slot_ref)
                .and_then(|i| scen.names.get(i))
                .map(|name| normalize_vdf_name(name))
        })
        .collect();
    let exp_ref_names: HashSet<String> = exp
        .parse_section6_ref_stream()
        .unwrap()
        .1
        .iter()
        .flat_map(|entry| entry.refs.iter())
        .filter_map(|slot_ref| {
            exp.slot_table
                .iter()
                .position(|s| s == slot_ref)
                .and_then(|i| exp.names.get(i))
                .map(|name| normalize_vdf_name(name))
        })
        .collect();

    eprintln!("\n=== WRLD3 experiment overlap ===");
    eprintln!(
        "  visible names: scen={} exp={} overlap={}",
        scen_visible.len(),
        exp_visible.len(),
        visible_overlap.len()
    );
    eprintln!(
        "  empirical direct names: scen={} exp={} overlap={}",
        scen_empirical.len(),
        exp_empirical.len(),
        empirical_overlap.len()
    );
    eprintln!(
        "  empirical overlap same OT={} diff OT={}",
        same_ot.len(),
        diff_ot.len()
    );
    eprintln!(
        "  section6 ref-name overlap: scen={} exp={} overlap={}",
        scen_ref_names.len(),
        exp_ref_names.len(),
        scen_ref_names.intersection(&exp_ref_names).count()
    );
    eprintln!(
        "  display OT lists equal={} count={}",
        scen_display == exp_display,
        scen_display.len()
    );
    eprintln!("  first same-OT direct names:");
    for (name, ot) in same_ot.iter().take(40) {
        eprintln!("    OT[{ot:>3}] {name}");
    }
    eprintln!("  first differing-OT direct names:");
    for (name, scen_ot, exp_ot) in diff_ot.iter().take(40) {
        eprintln!("    {name}: scen=OT[{scen_ot}] exp=OT[{exp_ot}]");
    }
}

#[test]
#[ignore]
fn test_debug_econ_base_vs_rk_overlap() {
    let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end().unwrap();

    let base = vdf_file("../../test/bobby/vdf/econ/base.vdf");
    let rk = vdf_file("../../test/bobby/vdf/econ/rk.vdf");

    let base_visible = debug_visible_vdf_names(&base);
    let rk_visible = debug_visible_vdf_names(&rk);
    let base_empirical = debug_empirical_direct_visible_map(&base, &results);
    let rk_empirical = debug_empirical_direct_visible_map(&rk, &results);

    let empirical_overlap: HashSet<String> = base_empirical
        .keys()
        .filter(|name| rk_empirical.contains_key(*name))
        .cloned()
        .collect();

    let mut same_ot = Vec::new();
    let mut diff_ot = Vec::new();
    for name in &empirical_overlap {
        let base_ot = base_empirical[name];
        let rk_ot = rk_empirical[name];
        if base_ot == rk_ot {
            same_ot.push((name.clone(), base_ot));
        } else {
            diff_ot.push((name.clone(), base_ot, rk_ot));
        }
    }
    same_ot.sort_by_key(|(_, ot)| *ot);
    diff_ot.sort_by(|a, b| a.0.cmp(&b.0));

    eprintln!("\n=== econ base vs rk overlap ===");
    eprintln!(
        "  visible names: base={} rk={} overlap={}",
        base_visible.len(),
        rk_visible.len(),
        base_visible.intersection(&rk_visible).count()
    );
    eprintln!(
        "  empirical direct names: base={} rk={} overlap={}",
        base_empirical.len(),
        rk_empirical.len(),
        empirical_overlap.len()
    );
    eprintln!(
        "  empirical overlap same OT={} diff OT={}",
        same_ot.len(),
        diff_ot.len()
    );
    eprintln!("  first same-OT direct names:");
    for (name, ot) in same_ot.iter().take(30) {
        eprintln!("    OT[{ot:>3}] {name}");
    }
    eprintln!("  first differing-OT direct names:");
    for (name, base_ot, rk_ot) in diff_ot.iter().take(30) {
        eprintln!("    {name}: base=OT[{base_ot}] rk=OT[{rk_ot}]");
    }
}

#[test]
#[ignore]
fn test_debug_paired_visible_slot_word_stability() {
    let water_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(
            &std::fs::read_to_string("../../test/bobby/vdf/water/water.mdl").unwrap(),
        )
        .unwrap(),
    ));
    let water_results = crate::interpreter::Simulation::new(&water_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    let pop_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(&std::fs::read_to_string("../../test/bobby/vdf/pop/pop.mdl").unwrap())
            .unwrap(),
    ));
    let pop_results = crate::interpreter::Simulation::new(&pop_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    let econ_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(
            &std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").unwrap(),
        )
        .unwrap(),
    ));
    let econ_results = crate::interpreter::Simulation::new(&econ_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    let wrld3_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(
            &std::fs::read_to_string("../../test/metasd/WRLD3-03/wrld3-03.mdl").unwrap(),
        )
        .unwrap(),
    ));
    let wrld3_results = crate::interpreter::Simulation::new(&wrld3_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    for (label, left_path, right_path, results) in [
        (
            "water",
            "../../test/bobby/vdf/water/Current.vdf",
            "../../test/bobby/vdf/water/water.vdf",
            &water_results,
        ),
        (
            "pop",
            "../../test/bobby/vdf/pop/Current.vdf",
            "../../test/bobby/vdf/pop/pop.vdf",
            &pop_results,
        ),
        (
            "econ",
            "../../test/bobby/vdf/econ/base.vdf",
            "../../test/bobby/vdf/econ/mark2.vdf",
            &econ_results,
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
            "../../test/metasd/WRLD3-03/experiment.vdf",
            &wrld3_results,
        ),
    ] {
        let left = vdf_file(left_path);
        let right = vdf_file(right_path);
        let left_words = debug_visible_slot_words(&left);
        let right_words = debug_visible_slot_words(&right);
        let left_saved = debug_empirical_direct_visible_map(&left, results);
        let right_saved = debug_empirical_direct_visible_map(&right, results);

        let visible_overlap: Vec<String> = left_words
            .keys()
            .filter(|name| right_words.contains_key(*name))
            .cloned()
            .collect();
        let saved_overlap: Vec<String> = left_saved
            .keys()
            .filter(|name| right_saved.contains_key(*name))
            .cloned()
            .collect();

        let mut slot_word_diffs = Vec::new();
        for name in &visible_overlap {
            if left_words[name].words != right_words[name].words {
                slot_word_diffs.push((
                    name.clone(),
                    left_words[name].words,
                    right_words[name].words,
                ));
            }
        }
        slot_word_diffs.sort_by(|a, b| a.0.cmp(&b.0));

        let mut saved_ot_diffs = Vec::new();
        for name in &saved_overlap {
            if left_saved[name] != right_saved[name] {
                saved_ot_diffs.push((name.clone(), left_saved[name], right_saved[name]));
            }
        }
        saved_ot_diffs.sort_by(|a, b| a.0.cmp(&b.0));

        eprintln!("\n=== paired visible slot-word stability: {label} ===");
        eprintln!(
            "  visible overlap={} saved overlap={} slot-word diffs={} saved OT diffs={}",
            visible_overlap.len(),
            saved_overlap.len(),
            slot_word_diffs.len(),
            saved_ot_diffs.len()
        );
        for (name, left_words, right_words) in slot_word_diffs.iter().take(20) {
            eprintln!(
                "    slot words differ {name}: left=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}] right=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}]",
                left_words[0],
                left_words[1],
                left_words[2],
                left_words[3],
                right_words[0],
                right_words[1],
                right_words[2],
                right_words[3]
            );
        }
        for (name, left_ot, right_ot) in saved_ot_diffs.iter().take(20) {
            eprintln!("    saved OT differs {name}: left=OT[{left_ot}] right=OT[{right_ot}]");
        }
    }
}

#[test]
#[ignore]
fn test_debug_visible_slot_tuple_signatures() {
    let water_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(
            &std::fs::read_to_string("../../test/bobby/vdf/water/water.mdl").unwrap(),
        )
        .unwrap(),
    ));
    let water_results = crate::interpreter::Simulation::new(&water_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    let pop_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(&std::fs::read_to_string("../../test/bobby/vdf/pop/pop.mdl").unwrap())
            .unwrap(),
    ));
    let pop_results = crate::interpreter::Simulation::new(&pop_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    let econ_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(
            &std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").unwrap(),
        )
        .unwrap(),
    ));
    let econ_results = crate::interpreter::Simulation::new(&econ_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    let wrld3_project = std::rc::Rc::new(crate::Project::from(
        crate::compat::open_vensim(
            &std::fs::read_to_string("../../test/metasd/WRLD3-03/wrld3-03.mdl").unwrap(),
        )
        .unwrap(),
    ));
    let wrld3_results = crate::interpreter::Simulation::new(&wrld3_project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    for (label, vdf_path, results) in [
        (
            "water-current",
            "../../test/bobby/vdf/water/Current.vdf",
            &water_results,
        ),
        (
            "water-rerun",
            "../../test/bobby/vdf/water/water.vdf",
            &water_results,
        ),
        ("pop-current", "../../test/bobby/vdf/pop/Current.vdf", &pop_results),
        ("pop-rerun", "../../test/bobby/vdf/pop/pop.vdf", &pop_results),
        (
            "econ-base",
            "../../test/bobby/vdf/econ/base.vdf",
            &econ_results,
        ),
        (
            "econ-rerun",
            "../../test/bobby/vdf/econ/mark2.vdf",
            &econ_results,
        ),
        (
            "wrld3-scen",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
            &wrld3_results,
        ),
        (
            "wrld3-exp",
            "../../test/metasd/WRLD3-03/experiment.vdf",
            &wrld3_results,
        ),
    ] {
        let vdf = vdf_file(vdf_path);
        let slot_words = debug_visible_slot_words(&vdf);
        let saved_names: HashSet<String> = debug_empirical_direct_visible_map(&vdf, results)
            .into_keys()
            .collect();

        let mut counts: HashMap<[u32; 4], (usize, usize)> = HashMap::new();
        for (name, words) in &slot_words {
            let entry = counts.entry(words.words).or_default();
            if saved_names.contains(name) {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }

        let mut mixed: Vec<_> = counts
            .iter()
            .filter_map(|(words, (saved, unsaved))| {
                (*saved > 0 && *unsaved > 0).then_some((*words, *saved, *unsaved))
            })
            .collect();
        mixed.sort_by(|a, b| {
            (b.1 + b.2)
                .cmp(&(a.1 + a.2))
                .then_with(|| a.0.cmp(&b.0))
        });

        let mut pure_saved: Vec<_> = counts
            .iter()
            .filter_map(|(words, (saved, unsaved))| {
                (*saved > 0 && *unsaved == 0).then_some((*words, *saved))
            })
            .collect();
        pure_saved.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let mut pure_unsaved: Vec<_> = counts
            .iter()
            .filter_map(|(words, (saved, unsaved))| {
                (*saved == 0 && *unsaved > 0).then_some((*words, *unsaved))
            })
            .collect();
        pure_unsaved.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        eprintln!("\n=== visible slot tuple signatures: {label} ===");
        eprintln!(
            "  visible tuples={} pure_saved={} pure_unsaved={} mixed={}",
            counts.len(),
            pure_saved.len(),
            pure_unsaved.len(),
            mixed.len()
        );
        eprintln!("  top pure-saved tuples:");
        for (words, count) in pure_saved.iter().take(12) {
            eprintln!(
                "    count={count:>3} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}]",
                words[0], words[1], words[2], words[3]
            );
        }
        eprintln!("  top pure-unsaved tuples:");
        for (words, count) in pure_unsaved.iter().take(12) {
            eprintln!(
                "    count={count:>3} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}]",
                words[0], words[1], words[2], words[3]
            );
        }
        eprintln!("  mixed tuples:");
        for (words, saved, unsaved) in mixed.iter().take(24) {
            eprintln!(
                "    saved={saved:>3} unsaved={unsaved:>3} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}]",
                words[0], words[1], words[2], words[3]
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_lookup_table_candidates_vs_vdf_overcount() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let vdf = vdf_file(vdf_path);

        let candidates = debug_visible_filtered_candidates(&vdf);
        let candidate_normalized: HashSet<String> =
            candidates.iter().map(|name| normalize_vdf_name(name)).collect();

        let mut gf_names = Vec::new();
        let mut stdlib_alias_names = Vec::new();
        let mut tableish_names = Vec::new();
        for var in &model.variables {
            match var {
                crate::datamodel::Variable::Aux(a) => {
                    let normalized = normalize_vdf_name(&a.ident);
                    if !candidate_normalized.contains(&normalized) {
                        continue;
                    }
                    if a.gf.is_some() {
                        gf_names.push(a.ident.clone());
                    }
                    if extract_stdlib_call_info(&a.equation).is_some() {
                        stdlib_alias_names.push(a.ident.clone());
                    }
                    if a.ident.to_lowercase().contains("lookup")
                        || a.ident.to_lowercase().contains(" table")
                    {
                        tableish_names.push(a.ident.clone());
                    }
                }
                crate::datamodel::Variable::Flow(f) => {
                    let normalized = normalize_vdf_name(&f.ident);
                    if !candidate_normalized.contains(&normalized) {
                        continue;
                    }
                    if f.gf.is_some() {
                        gf_names.push(f.ident.clone());
                    }
                    if extract_stdlib_call_info(&f.equation).is_some() {
                        stdlib_alias_names.push(f.ident.clone());
                    }
                    if f.ident.to_lowercase().contains("lookup")
                        || f.ident.to_lowercase().contains(" table")
                    {
                        tableish_names.push(f.ident.clone());
                    }
                }
                _ => {}
            }
        }

        gf_names.sort();
        gf_names.dedup();
        stdlib_alias_names.sort();
        stdlib_alias_names.dedup();
        tableish_names.sort();
        tableish_names.dedup();

        let stock_ots = vdf
            .section6_ot_class_codes()
            .unwrap()
            .into_iter()
            .skip(1)
            .filter(|&code| code == VDF_SECTION6_OT_CODE_STOCK)
            .count();

        eprintln!("\n=== lookup/table candidates vs VDF overcount: {label} ===");
        eprintln!(
            "  visible filtered candidates={} stock_ots={} nonstock_capacity={} total_ots={}",
            candidates.len(),
            stock_ots,
            vdf.offset_table_count.saturating_sub(1 + stock_ots),
            vdf.offset_table_count.saturating_sub(1)
        );
        eprintln!("  candidate names backed by graphical functions: {}", gf_names.len());
        for name in gf_names.iter().take(40) {
            eprintln!("    gf {name}");
        }
        eprintln!("  candidate names that are direct stdlib aliases: {}", stdlib_alias_names.len());
        for name in stdlib_alias_names.iter().take(40) {
            eprintln!("    stdlib {name}");
        }
        eprintln!("  candidate names with lookup/table-ish strings: {}", tableish_names.len());
        for name in tableish_names.iter().take(60) {
            eprintln!("    tableish {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_graphical_function_candidate_equations() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let vdf = vdf_file(vdf_path);
        let candidate_normalized: HashSet<String> = debug_visible_filtered_candidates(&vdf)
            .into_iter()
            .map(|name| normalize_vdf_name(&name))
            .collect();

        let mut gf_entries = Vec::new();
        for var in &model.variables {
            match var {
                crate::datamodel::Variable::Aux(a) if a.gf.is_some() => {
                    if candidate_normalized.contains(&normalize_vdf_name(&a.ident)) {
                        let eq = match &a.equation {
                            crate::datamodel::Equation::Scalar(s) => format!("scalar:{s}"),
                            crate::datamodel::Equation::ApplyToAll(_, s) => {
                                format!("apply_to_all:{s}")
                            }
                            crate::datamodel::Equation::Arrayed(_, _, default, except) => {
                                format!("arrayed:default={default:?}:except={except}")
                            }
                        };
                        gf_entries.push((a.ident.clone(), eq));
                    }
                }
                crate::datamodel::Variable::Flow(f) if f.gf.is_some() => {
                    if candidate_normalized.contains(&normalize_vdf_name(&f.ident)) {
                        let eq = match &f.equation {
                            crate::datamodel::Equation::Scalar(s) => format!("scalar:{s}"),
                            crate::datamodel::Equation::ApplyToAll(_, s) => {
                                format!("apply_to_all:{s}")
                            }
                            crate::datamodel::Equation::Arrayed(_, _, default, except) => {
                                format!("arrayed:default={default:?}:except={except}")
                            }
                        };
                        gf_entries.push((f.ident.clone(), eq));
                    }
                }
                _ => {}
            }
        }

        gf_entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut by_eq: HashMap<String, usize> = HashMap::new();
        for (_, eq) in &gf_entries {
            *by_eq.entry(eq.clone()).or_default() += 1;
        }
        let mut by_eq: Vec<_> = by_eq.into_iter().collect();
        by_eq.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        eprintln!("\n=== graphical-function candidate equations: {label} ===");
        eprintln!("  gf candidate count={}", gf_entries.len());
        eprintln!("  equation buckets:");
        for (eq, count) in by_eq.iter().take(20) {
            eprintln!("    {count:>3} {eq}");
        }
        eprintln!("  first entries:");
        for (name, eq) in gf_entries.iter().take(60) {
            eprintln!("    {name} => {eq}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_graphical_function_candidate_refs() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let vdf = vdf_file(vdf_path);
        let sec6_ref_set: HashSet<u32> = vdf
            .parse_section6_ref_stream()
            .unwrap()
            .1
            .into_iter()
            .flat_map(|entry| entry.refs)
            .collect();
        let sec4_ref_set: HashSet<u32> = parse_debug_section4_entries(&vdf)
            .unwrap()
            .1
            .into_iter()
            .flat_map(|entry| entry.refs)
            .collect();

        let mut slot_by_name: HashMap<String, u32> = HashMap::new();
        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            if let Some(name) = vdf.names.get(i) {
                slot_by_name.insert(normalize_vdf_name(name), slot_ref);
            }
        }

        let mut gf_rows = Vec::new();
        for var in &model.variables {
            let (ident, gf) = match var {
                crate::datamodel::Variable::Aux(a) => (&a.ident, a.gf.as_ref()),
                crate::datamodel::Variable::Flow(f) => (&f.ident, f.gf.as_ref()),
                _ => continue,
            };
            if gf.is_none() {
                continue;
            }
            let normalized = normalize_vdf_name(ident);
            let Some(&slot_ref) = slot_by_name.get(&normalized) else {
                continue;
            };
            gf_rows.push((
                ident.clone(),
                slot_ref,
                sec6_ref_set.contains(&slot_ref),
                sec4_ref_set.contains(&slot_ref),
            ));
        }
        gf_rows.sort_by(|a, b| a.0.cmp(&b.0));

        let sec6_hits = gf_rows.iter().filter(|(_, _, has_sec6, _)| *has_sec6).count();
        let sec4_hits = gf_rows.iter().filter(|(_, _, _, has_sec4)| *has_sec4).count();

        eprintln!("\n=== graphical-function candidate refs: {label} ===");
        eprintln!(
            "  gf candidates={} sec6_hits={} sec4_hits={}",
            gf_rows.len(),
            sec6_hits,
            sec4_hits
        );
        for (name, slot_ref, has_sec6, has_sec4) in gf_rows.iter().take(80) {
            eprintln!(
                "    slot={slot_ref:>5} sec6={} sec4={} {name}",
                has_sec6,
                has_sec4
            );
        }
    }
}

#[test]
#[ignore]
fn test_debug_candidate_count_after_gf_no_sec6_filter() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let vdf = vdf_file(vdf_path);

        let sec6_ref_set: HashSet<u32> = vdf
            .parse_section6_ref_stream()
            .unwrap()
            .1
            .into_iter()
            .flat_map(|entry| entry.refs)
            .collect();

        let mut slot_name: HashMap<u32, String> = HashMap::new();
        for (i, &slot) in vdf.slot_table.iter().enumerate() {
            if let Some(name) = vdf.names.get(i) {
                slot_name.insert(slot, name.clone());
            }
        }

        let mut alias_names_normalized: HashSet<String> = HashSet::new();
        let mut model_stock_set: HashSet<String> = HashSet::new();
        let mut model_sig_stock_names: HashSet<String> = HashSet::new();
        for var in &model.variables {
            let (ident, equation) = match var {
                crate::datamodel::Variable::Stock(s) => {
                    model_stock_set.insert(normalize_vdf_name(&s.ident));
                    continue;
                }
                crate::datamodel::Variable::Aux(a) => (&a.ident, &a.equation),
                crate::datamodel::Variable::Flow(f) => (&f.ident, &f.equation),
                crate::datamodel::Variable::Module(_) => continue,
            };
            if let Some(info) = extract_stdlib_call_info(equation) {
                alias_names_normalized.insert(normalize_vdf_name(ident));
                for (sig, is_stock) in info.vensim_signatures() {
                    if is_stock {
                        model_sig_stock_names.insert(sig);
                    }
                }
            }
        }

        let mut candidates = debug_visible_filtered_candidates(&vdf);
        candidates.retain(|n| !alias_names_normalized.contains(&normalize_vdf_name(n)));
        let current_visible = candidates.len();

        let name_to_slot: HashMap<String, u32> = slot_name
            .into_iter()
            .map(|(slot, name)| (normalize_vdf_name(&name), slot))
            .collect();

        let gf_no_sec6_normalized: HashSet<String> = model
            .variables
            .iter()
            .filter_map(|var| match var {
                crate::datamodel::Variable::Aux(a) if a.gf.is_some() => Some(a.ident.clone()),
                crate::datamodel::Variable::Flow(f) if f.gf.is_some() => Some(f.ident.clone()),
                _ => None,
            })
            .filter(|name| {
                let normalized = normalize_vdf_name(name);
                let Some(&slot) = name_to_slot.get(&normalized) else {
                    return false;
                };
                !sec6_ref_set.contains(&slot)
            })
            .map(|name| normalize_vdf_name(&name))
            .collect();

        candidates.retain(|n| !gf_no_sec6_normalized.contains(&normalize_vdf_name(n)));
        let filtered_visible = candidates.len();

        eprintln!("\n=== candidate count after gf-no-sec6 filter: {label} ===");
        eprintln!(
            "  visible_current={} visible_filtered={} gf_no_sec6={} hidden_stock_names={} ot_capacity={}",
            current_visible,
            filtered_visible,
            gf_no_sec6_normalized.len(),
            model_sig_stock_names.len(),
            vdf.offset_table_count.saturating_sub(1)
        );
        eprintln!(
            "  filtered_visible_plus_hidden_stock={}",
            filtered_visible + model_sig_stock_names.len()
        );
        let mut extras: Vec<_> = gf_no_sec6_normalized.into_iter().collect();
        extras.sort();
        for name in extras.iter().take(40) {
            eprintln!("    excluded {name}");
        }
    }
}

#[test]
#[ignore]
fn test_debug_remaining_nonstock_candidates_without_section6_refs() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let model = datamodel_project
        .models
        .iter()
        .find(|m| m.name == "main")
        .unwrap();
    let vdf = vdf_file(vdf_path);

    let sec6_ref_set: HashSet<u32> = vdf
        .parse_section6_ref_stream()
        .unwrap()
        .1
        .into_iter()
        .flat_map(|entry| entry.refs)
        .collect();

    let mut alias_names_normalized: HashSet<String> = HashSet::new();
    let mut stock_names_normalized: HashSet<String> = HashSet::new();
    let mut gf_names_normalized: HashSet<String> = HashSet::new();
    for var in &model.variables {
        match var {
            crate::datamodel::Variable::Stock(s) => {
                stock_names_normalized.insert(normalize_vdf_name(&s.ident));
            }
            crate::datamodel::Variable::Aux(a) => {
                if a.gf.is_some() {
                    gf_names_normalized.insert(normalize_vdf_name(&a.ident));
                }
                if extract_stdlib_call_info(&a.equation).is_some() {
                    alias_names_normalized.insert(normalize_vdf_name(&a.ident));
                }
            }
            crate::datamodel::Variable::Flow(f) => {
                if f.gf.is_some() {
                    gf_names_normalized.insert(normalize_vdf_name(&f.ident));
                }
                if extract_stdlib_call_info(&f.equation).is_some() {
                    alias_names_normalized.insert(normalize_vdf_name(&f.ident));
                }
            }
            crate::datamodel::Variable::Module(_) => {}
        }
    }

    let mut remaining = Vec::new();
    for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
        let Some(name) = vdf.names.get(i) else {
            continue;
        };
        if is_vdf_metadata_entry(name) || name.starts_with('#') {
            continue;
        }
        let normalized = normalize_vdf_name(name);
        if alias_names_normalized.contains(&normalized) || stock_names_normalized.contains(&normalized) {
            continue;
        }
        if is_probable_lookup_table_name(name)
            && gf_names_normalized.contains(&normalized)
            && !sec6_ref_set.contains(&slot_ref)
        {
            continue;
        }
        if sec6_ref_set.contains(&slot_ref) {
            continue;
        }
        remaining.push((name.clone(), slot_ref, gf_names_normalized.contains(&normalized)));
    }

    remaining.sort_by(|a, b| a.0.cmp(&b.0));

    eprintln!("\n=== remaining WRLD3 nonstock candidates without section6 refs ===");
    eprintln!("  count={}", remaining.len());
    for (name, slot_ref, is_gf) in remaining.iter().take(80) {
        eprintln!("    slot={slot_ref:>5} gf={} {name}", is_gf);
    }
}

#[test]
#[ignore]
fn test_debug_wrld3_gf_candidates_vs_direct_empirical() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let model = datamodel_project
        .models
        .iter()
        .find(|m| m.name == "main")
        .cloned()
        .unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let results = crate::interpreter::Simulation::new(&project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();
    let vdf = vdf_file(vdf_path);

    let sec6_ref_set: HashSet<u32> = vdf
        .parse_section6_ref_stream()
        .unwrap()
        .1
        .into_iter()
        .flat_map(|entry| entry.refs)
        .collect();
    let direct_empirical = debug_empirical_direct_visible_map(&vdf, &results);
    let direct_empirical_names: HashSet<String> = direct_empirical.into_keys().collect();

    let mut slot_by_name: HashMap<String, u32> = HashMap::new();
    for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
        if let Some(name) = vdf.names.get(i) {
            slot_by_name.insert(normalize_vdf_name(name), slot_ref);
        }
    }

    let mut rows = Vec::new();
    for var in &model.variables {
        let ident = match var {
            crate::datamodel::Variable::Aux(a) if a.gf.is_some() => &a.ident,
            crate::datamodel::Variable::Flow(f) if f.gf.is_some() => &f.ident,
            _ => continue,
        };
        let normalized = normalize_vdf_name(ident);
        let Some(&slot_ref) = slot_by_name.get(&normalized) else {
            continue;
        };
        rows.push((
            ident.clone(),
            sec6_ref_set.contains(&slot_ref),
            direct_empirical_names.contains(&normalized),
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let sec6_and_direct = rows
        .iter()
        .filter(|(_, has_sec6, is_direct)| *has_sec6 && *is_direct)
        .count();
    let sec6_only = rows
        .iter()
        .filter(|(_, has_sec6, is_direct)| *has_sec6 && !*is_direct)
        .count();
    let direct_only = rows
        .iter()
        .filter(|(_, has_sec6, is_direct)| !*has_sec6 && *is_direct)
        .count();
    let neither = rows
        .iter()
        .filter(|(_, has_sec6, is_direct)| !*has_sec6 && !*is_direct)
        .count();

    eprintln!("\n=== WRLD3 gf candidates vs direct empirical ===");
    eprintln!(
        "  total={} sec6+direct={} sec6_only={} direct_only={} neither={}",
        rows.len(),
        sec6_and_direct,
        sec6_only,
        direct_only,
        neither
    );
    for (name, has_sec6, is_direct) in rows.iter().take(80) {
        eprintln!("    sec6={} direct={} {name}", has_sec6, is_direct);
    }
}

#[test]
#[ignore]
fn test_debug_wrld3_gf_nonsec6_slot_words() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let model = datamodel_project
        .models
        .iter()
        .find(|m| m.name == "main")
        .cloned()
        .unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let results = crate::interpreter::Simulation::new(&project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();
    let vdf = vdf_file(vdf_path);

    let sec6_ref_set: HashSet<u32> = vdf
        .parse_section6_ref_stream()
        .unwrap()
        .1
        .into_iter()
        .flat_map(|entry| entry.refs)
        .collect();
    let direct_empirical_names: HashSet<String> =
        debug_empirical_direct_visible_map(&vdf, &results).into_keys().collect();
    let slot_words = debug_visible_slot_words(&vdf);

    let mut direct_only = Vec::new();
    let mut neither = Vec::new();
    for var in &model.variables {
        let ident = match var {
            crate::datamodel::Variable::Aux(a) if a.gf.is_some() => &a.ident,
            crate::datamodel::Variable::Flow(f) if f.gf.is_some() => &f.ident,
            _ => continue,
        };
        let normalized = normalize_vdf_name(ident);
        let Some(words) = slot_words.get(&normalized).copied() else {
            continue;
        };
        if sec6_ref_set.contains(&words.slot_ref) {
            continue;
        }
        let row = (ident.clone(), words.slot_ref, words.words);
        if direct_empirical_names.contains(&normalized) {
            direct_only.push(row);
        } else {
            neither.push(row);
        }
    }

    direct_only.sort_by(|a, b| a.0.cmp(&b.0));
    neither.sort_by(|a, b| a.0.cmp(&b.0));

    eprintln!("\n=== WRLD3 gf non-sec6 slot words ===");
    eprintln!("  direct-only={}", direct_only.len());
    for (name, slot_ref, words) in &direct_only {
        eprintln!(
            "    direct slot={slot_ref:>5} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}] {name}",
            words[0], words[1], words[2], words[3]
        );
    }
    eprintln!("  neither={}", neither.len());
    for (name, slot_ref, words) in neither.iter().take(40) {
        eprintln!(
            "    neither slot={slot_ref:>5} words=[0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}] {name}",
            words[0], words[1], words[2], words[3]
        );
    }
}

#[test]
#[ignore]
fn test_debug_wrld3_nonsec6_table_parent_aliases() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let results = crate::interpreter::Simulation::new(&project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();
    let vdf = vdf_file(vdf_path);

    let sec6_ref_set: HashSet<u32> = vdf
        .parse_section6_ref_stream()
        .unwrap()
        .1
        .into_iter()
        .flat_map(|entry| entry.refs)
        .collect();
    let mut visible_names = Vec::new();
    let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
    for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
        let Some(name) = vdf.names.get(i) else {
            continue;
        };
        if name.is_empty()
            || name.starts_with('.')
            || name.starts_with('-')
            || name.starts_with('#')
            || name.starts_with(':')
            || name.starts_with('"')
            || system_names.contains(name.as_str())
            || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
            || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
        {
            continue;
        }
        visible_names.push((name.clone(), slot_ref));
    }
    let direct_empirical = debug_empirical_direct_visible_map(&vdf, &results);

    let mut rows = Vec::new();
    let visible_name_set: HashSet<String> = visible_names
        .iter()
        .map(|(name, _)| normalize_vdf_name(name))
        .collect();
    for (name, slot_ref) in &visible_names {
        if !is_probable_lookup_table_name(name) || sec6_ref_set.contains(slot_ref) {
            continue;
        }
        let parent = name
            .replace(" LOOKUP", "")
            .replace(" lookup", "")
            .replace(" table", "")
            .replace(" Table", "")
            .replace("  ", " ")
            .trim()
            .to_string();
        rows.push((
            name.clone(),
            parent.clone(),
            visible_name_set.contains(&normalize_vdf_name(&parent)),
            direct_empirical.get(&normalize_vdf_name(name)).copied(),
            direct_empirical.get(&normalize_vdf_name(&parent)).copied(),
        ));
    }

    rows.sort_by(|a, b| a.0.cmp(&b.0));

    eprintln!("\n=== WRLD3 non-sec6 table parent aliases ===");
    for (name, parent, parent_exists, name_ot, parent_ot) in rows.iter().take(80) {
        eprintln!(
            "    parent_exists={} name_ot={name_ot:?} parent_ot={parent_ot:?} {name} -> {parent}",
            parent_exists
        );
    }
}

#[test]
#[ignore]
fn test_debug_stocks_first_vs_direct_visible_empirical() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = crate::Project::from(datamodel_project.clone());
        let sim_project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&sim_project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let predicted = vdf
            .build_stocks_first_ot_map_for_project(&project, "main")
            .unwrap();
        let empirical = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();

        let mut correct = 0usize;
        let mut wrong = 0usize;
        let mut missing = 0usize;
        for (name, &emp_ot) in &empirical {
            let raw = name.as_str();
            if raw == "time" || raw.starts_with("$⁚") {
                continue;
            }
            match predicted.get(name) {
                Some(&pred_ot) if pred_ot == emp_ot => correct += 1,
                Some(_) => wrong += 1,
                None => missing += 1,
            }
        }

        eprintln!("\n=== stocks-first vs direct visible empirical: {label} ===");
        eprintln!("  correct={correct} wrong={wrong} missing={missing}");
        if correct + wrong > 0 {
            eprintln!(
                "  accuracy={:.1}%",
                100.0 * correct as f64 / (correct + wrong) as f64
            );
        }
    }
}

/// Investigate the WRLD3 participant gap and lookup-table occupancy.
///
/// For each VDF file, build the VDF-only candidate list and compare against
/// empirical ground truth. This identifies which names are empirically saved
/// but excluded by our candidate filter, and which names we include but
/// empirically don't exist.
#[test]
#[ignore]
fn test_debug_vdf_only_participant_gap() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();

        // Build the set of empirically saved names (normalized).
        let mut empirical_all: HashMap<String, usize> = HashMap::new();
        for (id, &ot) in &empirical_map {
            empirical_all.insert(normalize_vdf_name(id.as_str()), ot);
        }

        // Build VDF-only candidate set using current filtering rules.
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let participant_helpers: HashSet<&str> =
            ["DEL", "LV1", "LV2", "LV3", "ST", "RT1", "RT2", "DL"]
                .into_iter()
                .collect();
        let nonparticipant_helpers: HashSet<&str> = [
            "IN", "INI", "OUTPUT", "SMOOTH", "SMOOTHI", "SMOOTH3", "SMOOTH3I",
            "DELAY1", "DELAY1I", "DELAY3", "DELAY3I", "TREND", "NPV",
        ]
        .into_iter()
        .collect();

        let mut vdf_candidates: HashSet<String> = HashSet::new();
        let mut excluded_lookupish: Vec<String> = Vec::new();

        for name in vdf.names.iter().take(vdf.slot_table.len()) {
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with(':')
                || name.starts_with('"')
                || name.starts_with('#')
                || system_names.contains(name.as_str())
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
            {
                continue;
            }
            if VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
                continue;
            }
            if nonparticipant_helpers.contains(name.as_str()) {
                continue;
            }
            if is_probable_lookup_table_name(name) {
                excluded_lookupish.push(name.clone());
                continue;
            }
            if is_vdf_metadata_entry(name) {
                continue;
            }
            vdf_candidates.insert(normalize_vdf_name(name));
        }

        // Add participant helpers
        for name in vdf.names.iter().take(vdf.slot_table.len()) {
            if participant_helpers.contains(name.as_str()) {
                vdf_candidates.insert(normalize_vdf_name(name));
            }
        }

        // Add #-prefixed names from unslotted tail
        if vdf.names.len() > vdf.slot_table.len() {
            for name in &vdf.names[vdf.slot_table.len()..] {
                if name.starts_with('#') {
                    vdf_candidates.insert(normalize_vdf_name(name));
                }
            }
        }

        let ot_capacity = vdf.offset_table_count.saturating_sub(1);

        // Find empirically-saved names we excluded
        let mut false_negatives: Vec<(String, usize)> = Vec::new();
        for (emp_name, &emp_ot) in &empirical_all {
            if emp_name == "time" || emp_name.starts_with("$") {
                continue;
            }
            if !vdf_candidates.contains(emp_name) {
                false_negatives.push((emp_name.clone(), emp_ot));
            }
        }
        false_negatives.sort();

        // Find candidate names that are NOT empirically saved
        let empirical_norm: HashSet<String> = empirical_all.keys().cloned().collect();
        let mut false_positives: Vec<String> = vdf_candidates
            .iter()
            .filter(|name| !empirical_norm.contains(*name))
            .cloned()
            .collect();
        false_positives.sort();

        // Check lookupish names against empirical
        let mut lookupish_saved: Vec<(String, usize)> = Vec::new();
        let mut lookupish_not_saved: Vec<String> = Vec::new();
        for name in &excluded_lookupish {
            let normalized = normalize_vdf_name(name);
            if let Some(&ot) = empirical_all.get(&normalized) {
                lookupish_saved.push((name.clone(), ot));
            } else {
                lookupish_not_saved.push(name.clone());
            }
        }
        lookupish_saved.sort();

        eprintln!("\n=== VDF-only participant gap analysis: {label} ===");
        eprintln!("  OT capacity (excl time) = {ot_capacity}");
        eprintln!("  VDF-only candidates     = {}", vdf_candidates.len());
        eprintln!("  gap                     = {}", ot_capacity as isize - vdf_candidates.len() as isize);
        eprintln!("  empirical saved (excl time, $-prefixed) = {}",
            empirical_all.iter().filter(|(k, _)| *k != "time" && !k.starts_with("$")).count());
        eprintln!("\n  false negatives (empirically saved but excluded): {}", false_negatives.len());
        for (name, ot) in &false_negatives {
            eprintln!("    OT[{ot:>3}] {name}");
        }
        eprintln!("\n  false positives (candidate but NOT empirically saved): {}", false_positives.len());
        for name in &false_positives {
            eprintln!("    {name}");
        }
        eprintln!("\n  lookupish names with empirical OT: {}", lookupish_saved.len());
        for (name, ot) in &lookupish_saved {
            eprintln!("    OT[{ot:>3}] {name}");
        }
        eprintln!("  lookupish names WITHOUT empirical OT: {}", lookupish_not_saved.len());
    }
}

/// Classify all VDF name table entries into categories and compare against
/// the OT capacity to find the exact participant formula.
#[test]
#[ignore]
fn test_debug_name_table_category_audit() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let empirical_norm: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut lookup_only = Vec::new();
        let mut table_only = Vec::new();
        let mut quoted_names = Vec::new();

        // Break down lookupish names into " lookup" vs " table"
        for (i, name) in vdf.names.iter().enumerate().take(vdf.slot_table.len()) {
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with(':')
                || system_names.contains(name.as_str())
            {
                continue;
            }
            if name.starts_with('"') {
                let normalized = normalize_vdf_name(name);
                let is_saved = empirical_norm.contains(&normalized);
                quoted_names.push((name.clone(), i, is_saved));
                continue;
            }

            let lower = name.to_lowercase();
            if lower.contains(" lookup") && !lower.contains(" table") {
                let normalized = normalize_vdf_name(name);
                let is_saved = empirical_norm.contains(&normalized);
                lookup_only.push((name.clone(), is_saved));
            } else if lower.contains(" table") && !lower.contains(" lookup") {
                let normalized = normalize_vdf_name(name);
                let is_saved = empirical_norm.contains(&normalized);
                table_only.push((name.clone(), is_saved));
            }
        }

        let lookup_saved = lookup_only.iter().filter(|(_, s)| *s).count();
        let table_saved = table_only.iter().filter(|(_, s)| *s).count();

        eprintln!("\n=== name table category audit: {label} ===");
        eprintln!("  OT capacity = {}", vdf.offset_table_count);
        eprintln!("  total names = {}, slotted = {}", vdf.names.len(), vdf.slot_table.len());

        eprintln!("\n  LOOKUP-only names (contain ' lookup', not ' table'): {}", lookup_only.len());
        eprintln!("    saved: {lookup_saved}, not saved: {}", lookup_only.len() - lookup_saved);
        for (name, is_saved) in &lookup_only {
            let marker = if *is_saved { "SAVED" } else { "     " };
            eprintln!("    {marker} {name}");
        }

        eprintln!("\n  TABLE-only names (contain ' table', not ' lookup'): {}", table_only.len());
        eprintln!("    saved: {table_saved}, not saved: {}", table_only.len() - table_saved);
        for (name, is_saved) in &table_only {
            let marker = if *is_saved { "SAVED" } else { "     " };
            eprintln!("    {marker} {name}");
        }

        eprintln!("\n  Quoted names (start with '\"'): {}", quoted_names.len());
        for (name, idx, is_saved) in &quoted_names {
            let marker = if *is_saved { "SAVED" } else { "     " };
            eprintln!("    {marker} idx={idx} {name}");
        }

        // Count with different filtering strategies
        let mut _count_no_lookup = 0usize;
        let mut count_no_lookup_no_table = 0usize;
        let mut count_no_lookup_keep_table = 0usize;

        for name in vdf.names.iter().take(vdf.slot_table.len()) {
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with(':')
                || name.starts_with('"')
                || system_names.contains(name.as_str())
                || VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name))
                || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
                || is_vdf_metadata_entry(name)
            {
                continue;
            }

            let lower = name.to_lowercase();
            count_no_lookup_keep_table += if lower.contains(" lookup") { 0 } else { 1 };
            count_no_lookup_no_table += if lower.contains(" lookup") || lower.contains(" table") { 0 } else { 1 };
            _count_no_lookup += if lower.contains(" lookup") { 0 } else { 1 };
        }

        // Hash names from unslotted tail
        let hash_count = if vdf.names.len() > vdf.slot_table.len() {
            vdf.names[vdf.slot_table.len()..].iter().filter(|n| n.starts_with('#')).count()
        } else {
            0
        };

        eprintln!("\n  Counting strategies (excl hash={hash_count}):");
        eprintln!("    no lookup filter:             {} + hash = {}", count_no_lookup_keep_table, count_no_lookup_keep_table + hash_count);
        eprintln!("    no lookup, no table:           {} + hash = {}", count_no_lookup_no_table, count_no_lookup_no_table + hash_count);
        eprintln!("    OT capacity (incl time):       {}", vdf.offset_table_count);
    }
}

/// Dump section-6 class codes to see if stocks form a contiguous block
/// and where system variables sit.
#[test]
#[ignore]
fn test_debug_section6_code_layout() {
    for (label, vdf_path) in [
        ("water", "../../test/bobby/vdf/water/base.vdf"),
        ("pop", "../../test/bobby/vdf/pop/Current.vdf"),
        ("econ", "../../test/bobby/vdf/econ/base.vdf"),
        ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
    ] {
        let vdf = vdf_file(vdf_path);
        let Some(codes) = vdf.section6_ot_class_codes() else {
            eprintln!("\n=== section6 code layout: {label} === (no codes)");
            continue;
        };

        let stock_count = codes.iter().skip(1).filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK).count();
        let time_code = codes.first().copied().unwrap_or(0);

        // Find transition points
        let mut last_is_stock = None;
        let mut transitions = Vec::new();
        for (ot, &code) in codes.iter().enumerate() {
            let is_stock = code == VDF_SECTION6_OT_CODE_STOCK;
            if ot == 0 { continue; } // skip Time
            if last_is_stock != Some(is_stock) {
                transitions.push((ot, code, is_stock));
            }
            last_is_stock = Some(is_stock);
        }

        // Check contiguity: are stocks in one block?
        let first_nonstock_ot = codes.iter().enumerate().skip(1)
            .find(|&(_, &c)| c != VDF_SECTION6_OT_CODE_STOCK)
            .map(|(i, _)| i);
        let last_stock_ot = codes.iter().enumerate().skip(1)
            .rfind(|&(_, &c)| c == VDF_SECTION6_OT_CODE_STOCK)
            .map(|(i, _)| i);
        let contiguous = match (first_nonstock_ot, last_stock_ot) {
            (Some(first_ns), Some(last_s)) => last_s < first_ns,
            _ => true,
        };

        // Count distinct non-stock codes
        let mut nonstock_code_counts = std::collections::BTreeMap::<u8, usize>::new();
        for &code in codes.iter().skip(1) {
            if code != VDF_SECTION6_OT_CODE_STOCK {
                *nonstock_code_counts.entry(code).or_default() += 1;
            }
        }

        eprintln!("\n=== section6 code layout: {label} ===");
        eprintln!("  OT count = {}, stock count = {stock_count}", codes.len());
        eprintln!("  time code = 0x{time_code:02x}");
        eprintln!("  stocks contiguous = {contiguous}");
        eprintln!("  first non-stock OT = {first_nonstock_ot:?}, last stock OT = {last_stock_ot:?}");
        eprintln!("  non-stock code distribution:");
        for (&code, &count) in &nonstock_code_counts {
            eprintln!("    0x{code:02x}: {count}");
        }
        eprintln!("  transitions: {:?}", transitions.iter().take(8).collect::<Vec<_>>());
    }
}

/// Investigate the three-way OT partition (stock / dynamic / constant)
/// and whether inline constant values help narrow the name-to-OT mapping.
#[test]
#[ignore]
fn test_debug_three_way_ot_partition() {
    for (label, mdl_path, vdf_path) in [
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let codes = vdf.section6_ot_class_codes().unwrap();

        // Classify empirically matched names by their OT's section-6 code
        let mut stock_names = Vec::new();
        let mut dynamic_names = Vec::new();
        let mut constant_names = Vec::new();

        for (id, &ot) in &empirical_map {
            if id.as_str() == "time" || id.as_str().starts_with("$") {
                continue;
            }
            let code = codes.get(ot).copied().unwrap_or(0);
            let name = id.as_str().to_string();
            match code {
                VDF_SECTION6_OT_CODE_STOCK => stock_names.push((name, ot)),
                0x11 => dynamic_names.push((name, ot)),
                0x17 => {
                    // Read the inline constant value from the offset table
                    let const_val = vdf.offset_table_entry(ot)
                        .map(|raw| f32::from_le_bytes(raw.to_le_bytes()));
                    constant_names.push((name, ot, const_val));
                }
                _ => {}
            }
        }

        stock_names.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        dynamic_names.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        constant_names.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        // Check if stocks are alphabetically contiguous at lowest OTs
        let stock_ots: Vec<usize> = stock_names.iter().map(|(_, ot)| *ot).collect();
        let stock_alpha_monotonic = stock_ots.windows(2).all(|w| w[0] < w[1]);

        let dynamic_ots: Vec<usize> = dynamic_names.iter().map(|(_, ot)| *ot).collect();
        let dynamic_alpha_monotonic = dynamic_ots.windows(2).all(|w| w[0] < w[1]);

        let constant_ots: Vec<usize> = constant_names.iter().map(|(_, ot, _)| *ot).collect();
        let constant_alpha_monotonic = constant_ots.windows(2).all(|w| w[0] < w[1]);

        let stock_count_s6 = codes.iter().skip(1).filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK).count();
        let dynamic_count_s6 = codes.iter().skip(1).filter(|&&c| c == 0x11).count();
        let constant_count_s6 = codes.iter().skip(1).filter(|&&c| c == 0x17).count();

        eprintln!("\n=== three-way OT partition: {label} ===");
        eprintln!("  section-6 counts: stock={stock_count_s6} dynamic={dynamic_count_s6} constant={constant_count_s6}");
        eprintln!("  empirical matched: stock={} dynamic={} constant={}", stock_names.len(), dynamic_names.len(), constant_names.len());
        eprintln!("  stock alpha-monotonic (name order = OT order): {stock_alpha_monotonic}");
        eprintln!("  dynamic alpha-monotonic: {dynamic_alpha_monotonic}");
        eprintln!("  constant alpha-monotonic: {constant_alpha_monotonic}");

        // Show constant values to check if they're recognizable
        eprintln!("\n  constant non-stock entries (first 30):");
        for (name, ot, val) in constant_names.iter().take(30) {
            eprintln!("    OT[{ot:>3}] val={:>12.4} {name}", val.unwrap_or(f32::NAN));
        }
    }
}

/// Check if non-stock model variables (excluding system vars) are
/// alpha-monotonic when combined. Also check the stock partition.
#[test]
#[ignore]
fn test_debug_combined_nonstock_alpha_monotonic() {
    let system_idents: HashSet<&str> = [
        "time", "dt", "initial_time", "final_time", "saveper", "timestep",
        "initialtime", "finaltime",
    ].into_iter().collect();

    for (label, mdl_path, vdf_path) in [
        ("water", "../../test/bobby/vdf/water/water.mdl", "../../test/bobby/vdf/water/base.vdf"),
        ("pop", "../../test/bobby/vdf/pop/pop.mdl", "../../test/bobby/vdf/pop/Current.vdf"),
        ("econ", "../../test/bobby/vdf/econ/mark2.mdl", "../../test/bobby/vdf/econ/base.vdf"),
        ("wrld3", "../../test/metasd/WRLD3-03/wrld3-03.mdl", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let codes = vdf.section6_ot_class_codes().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();

        // Separate into stocks and non-stocks (excluding system/internal)
        let mut stocks: Vec<(String, usize)> = Vec::new();
        let mut nonstocks: Vec<(String, usize)> = Vec::new();

        for (id, &ot) in &empirical_map {
            let name = id.as_str();
            if system_idents.contains(name) || name.starts_with("$") || name.starts_with("#") {
                continue;
            }
            let code = codes.get(ot).copied().unwrap_or(0);
            if code == VDF_SECTION6_OT_CODE_STOCK {
                stocks.push((name.to_string(), ot));
            } else if code == 0x0f {
                continue; // Time
            } else {
                nonstocks.push((name.to_string(), ot));
            }
        }

        stocks.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        nonstocks.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        let stock_monotonic = stocks.windows(2).all(|w| w[0].1 < w[1].1);
        let nonstock_monotonic = nonstocks.windows(2).all(|w| w[0].1 < w[1].1);

        // Find inversions in non-stock ordering
        let mut inversions = Vec::new();
        for w in nonstocks.windows(2) {
            if w[0].1 >= w[1].1 {
                inversions.push((w[0].clone(), w[1].clone()));
            }
        }

        let stock_count_s6 = codes.iter().skip(1).filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK).count();

        eprintln!("\n=== combined non-stock alpha-monotonic: {label} ===");
        eprintln!("  stocks: {} empirical, {} section-6, alpha-mono={stock_monotonic}", stocks.len(), stock_count_s6);
        eprintln!("  non-stocks (excl system): {} empirical, alpha-mono={nonstock_monotonic}", nonstocks.len());
        eprintln!("  inversions: {}", inversions.len());
        for ((n1, ot1), (n2, ot2)) in inversions.iter().take(10) {
            eprintln!("    {n1} (OT[{ot1}]) >= {n2} (OT[{ot2}])");
        }
    }
}

/// Validate the section-6-guided mapper against empirical ground truth.
#[test]
#[ignore]
fn test_debug_section6_guided_vs_empirical() {
    for (label, mdl_path, vdf_path) in [
        ("water", "../../test/bobby/vdf/water/water.mdl", "../../test/bobby/vdf/water/base.vdf"),
        ("pop", "../../test/bobby/vdf/pop/pop.mdl", "../../test/bobby/vdf/pop/Current.vdf"),
        ("econ", "../../test/bobby/vdf/econ/mark2.mdl", "../../test/bobby/vdf/econ/base.vdf"),
        ("wrld3", "../../test/metasd/WRLD3-03/wrld3-03.mdl", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let model = datamodel_project
            .models
            .iter()
            .find(|m| m.name == "main")
            .unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project.clone()));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        // Debug: check model stock count and #-prefixed stock classification
        let model_stocks: Vec<&str> = model.variables.iter().filter_map(|v| {
            if let crate::datamodel::Variable::Stock(s) = v { Some(s.ident.as_str()) } else { None }
        }).collect();
        let hash_names: Vec<&str> = vdf.names.iter().filter(|n| n.starts_with('#')).map(|s| s.as_str()).collect();

        // Classify #-prefixed names as stock/non-stock based on signature
        let mut hash_stocks = 0usize;
        let mut hash_nonstocks = 0usize;
        for name in &hash_names {
            let is_stock = name.starts_with("#SMOOTH(") || name.starts_with("#SMOOTHI(")
                || name.starts_with("#SMOOTH3(") || name.starts_with("#SMOOTH3I(")
                || name.starts_with("#LV1<") || name.starts_with("#LV2<") || name.starts_with("#LV3<");
            if is_stock { hash_stocks += 1; } else { hash_nonstocks += 1; }
        }
        let codes = vdf.section6_ot_class_codes().unwrap();
        let s6_stocks = codes.iter().skip(1).filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK).count();
        eprintln!("\n  model stocks={} hash_stocks={} hash_nonstocks={} section6_stocks={s6_stocks}",
            model_stocks.len(), hash_stocks, hash_nonstocks);
        eprintln!("  model+hash stocks = {} vs section6 = {s6_stocks}, gap = {}",
            model_stocks.len() + hash_stocks, s6_stocks as isize - (model_stocks.len() + hash_stocks) as isize);
        if hash_stocks + model_stocks.len() != s6_stocks {
            eprintln!("  hash names:");
            for name in &hash_names {
                let is_stock = name.starts_with("#SMOOTH(") || name.starts_with("#SMOOTHI(")
                    || name.starts_with("#SMOOTH3(") || name.starts_with("#SMOOTH3I(")
                    || name.starts_with("#LV1<") || name.starts_with("#LV2<") || name.starts_with("#LV3<");
                eprintln!("    {} {name}", if is_stock { "STOCK" } else { "     " });
            }
        }

        let predicted = match vdf.build_section6_guided_ot_map(model) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("\n=== section6-guided vs empirical: {label} === ERROR: {e}");
                continue;
            }
        };
        let empirical = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();

        let mut correct = 0usize;
        let mut wrong = 0usize;
        let mut missing = 0usize;
        let mut wrong_examples = Vec::new();
        for (name, &emp_ot) in &empirical {
            let raw = name.as_str();
            if raw == "time" || raw.starts_with("$⁚") {
                continue;
            }
            match predicted.get(name) {
                Some(&pred_ot) if pred_ot == emp_ot => correct += 1,
                Some(&pred_ot) => {
                    wrong += 1;
                    if wrong_examples.len() < 20 {
                        wrong_examples.push((raw.to_string(), emp_ot, pred_ot));
                    }
                }
                None => missing += 1,
            }
        }

        wrong_examples.sort();

        eprintln!("\n=== section6-guided vs empirical: {label} ===");
        eprintln!("  predicted entries: {}", predicted.len());
        eprintln!("  empirical entries: {}", empirical.len());
        eprintln!("  correct={correct} wrong={wrong} missing={missing}");
        if correct + wrong > 0 {
            eprintln!(
                "  accuracy={:.1}%",
                100.0 * correct as f64 / (correct + wrong) as f64
            );
        }
        if !wrong_examples.is_empty() {
            eprintln!("  wrong mappings (first 20):");
            for (name, emp_ot, pred_ot) in &wrong_examples {
                eprintln!("    {name}: empirical=OT[{emp_ot}] predicted=OT[{pred_ot}]");
            }
        }
    }
}

/// Compare slot data between saved and unsaved TABLE names to find
/// a VDF-structural save/no-save signal.
#[test]
#[ignore]
fn test_debug_table_name_slot_signal() {
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end().unwrap();

    let vdf = vdf_file(vdf_path);
    let empirical_map = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
    let empirical_norm: HashSet<String> = empirical_map
        .keys()
        .map(|id| normalize_vdf_name(id.as_str()))
        .collect();

    let sec1 = vdf.slot_section().unwrap();
    let sec1_start = sec1.data_offset();
    let sec1_end = sec1.region_end.min(vdf.data.len());

    // Build sorted slot offsets to compute blob sizes
    let mut sorted_offsets: Vec<u32> = vdf.slot_table.clone();
    sorted_offsets.sort_unstable();
    sorted_offsets.dedup();
    let next_by_offset: HashMap<u32, u32> = sorted_offsets
        .windows(2)
        .map(|w| (w[0], w[1]))
        .collect();

    eprintln!("\n=== TABLE name slot data signal (WRLD3) ===");
    for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
        let Some(name) = vdf.names.get(i) else { continue };
        let lower = name.to_lowercase();
        if !lower.contains(" table") { continue; }

        let normalized = normalize_vdf_name(name);
        let is_saved = empirical_norm.contains(&normalized);

        let abs_start = sec1_start + slot_ref as usize;
        let blob_end = next_by_offset
            .get(&slot_ref)
            .map(|&next| sec1_start + next as usize)
            .unwrap_or(sec1_end);
        let blob_len = blob_end.saturating_sub(abs_start);

        // Read the slot blob words
        let word_count = blob_len / 4;
        let words: Vec<u32> = (0..word_count.min(8))
            .map(|j| read_u32(&vdf.data, abs_start + j * 4))
            .collect();

        let marker = if is_saved { "SAVED" } else { "     " };
        eprintln!(
            "  {marker} slot={slot_ref:>5} blob_len={blob_len:>3} words={:?} {name}",
            words.iter().map(|w| format!("0x{w:08x}")).collect::<Vec<_>>()
        );
    }
}

/// Test whether record presence (via slot_ref matching f[12]) predicts
/// OT participation, and explore using record count as the sole VDF-only
/// discriminator for participant filtering.
#[test]
#[ignore]
fn test_debug_record_presence_as_participant_signal() {
    for (label, mdl_path, vdf_path) in [
        (
            "water",
            "../../test/bobby/vdf/water/water.mdl",
            "../../test/bobby/vdf/water/base.vdf",
        ),
        (
            "pop",
            "../../test/bobby/vdf/pop/pop.mdl",
            "../../test/bobby/vdf/pop/Current.vdf",
        ),
        (
            "econ",
            "../../test/bobby/vdf/econ/mark2.mdl",
            "../../test/bobby/vdf/econ/base.vdf",
        ),
        (
            "wrld3",
            "../../test/metasd/WRLD3-03/wrld3-03.mdl",
            "../../test/metasd/WRLD3-03/SCEN01.VDF",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
        let results = sim.run_to_end().unwrap();

        let vdf = vdf_file(vdf_path);
        let vdf_data = vdf.extract_data().unwrap();
        let empirical_map = build_empirical_ot_map(&vdf_data, &results).unwrap();
        let empirical_norm: HashSet<String> = empirical_map
            .keys()
            .map(|id| normalize_vdf_name(id.as_str()))
            .collect();

        // Build record presence map: slot_ref -> count of model-variable records
        let mut records_by_slot: HashMap<u32, usize> = HashMap::new();
        for rec in &vdf.records {
            let ot = rec.fields[11] as usize;
            if rec.fields[0] != 0
                && rec.fields[1] != RECORD_F1_SYSTEM
                && rec.fields[1] != RECORD_F1_INITIAL_TIME_CONST
                && rec.fields[10] > 0
                && ot > 0
                && ot < vdf.offset_table_count
            {
                *records_by_slot.entry(rec.fields[12]).or_default() += 1;
            }
        }

        // Also count system-variable records (f[1]==23 or f[1]==15)
        let mut system_records_by_slot: HashMap<u32, usize> = HashMap::new();
        for rec in &vdf.records {
            if rec.fields[1] == RECORD_F1_SYSTEM || rec.fields[1] == RECORD_F1_INITIAL_TIME_CONST {
                *system_records_by_slot.entry(rec.fields[12]).or_default() += 1;
            }
        }

        // Minimal name filter: only remove structural metadata
        let _system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut has_record_and_saved = 0usize;
        let mut has_record_not_saved = 0usize;
        let mut no_record_but_saved = 0usize;
        let mut no_record_not_saved = 0usize;
        let mut has_record_names = Vec::new();
        let mut no_record_saved_names = Vec::new();

        for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
            let Some(name) = vdf.names.get(i) else {
                continue;
            };
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.starts_with(':')
            {
                continue;
            }

            let normalized = normalize_vdf_name(name);
            let has_model_record = records_by_slot.contains_key(&slot_ref);
            let has_system_record = system_records_by_slot.contains_key(&slot_ref);
            let is_saved = empirical_norm.contains(&normalized);

            if has_model_record || has_system_record {
                if is_saved {
                    has_record_and_saved += 1;
                } else {
                    has_record_not_saved += 1;
                }
                has_record_names.push((name.clone(), slot_ref, has_model_record, has_system_record));
            } else if is_saved {
                no_record_but_saved += 1;
                no_record_saved_names.push((name.clone(), slot_ref));
            } else {
                no_record_not_saved += 1;
            }
        }

        no_record_saved_names.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        let model_record_ots = vdf.model_record_ot_indices();

        eprintln!("\n=== record presence as participant signal: {label} ===");
        eprintln!("  OT capacity (excl time) = {}", vdf.offset_table_count - 1);
        eprintln!("  model-variable record OTs = {}", model_record_ots.len());
        eprintln!("  distinct slot_refs with records = {}", records_by_slot.len());
        eprintln!("  names with ANY record (model or system) = {}", has_record_names.len());
        eprintln!("  has record AND empirically saved   = {has_record_and_saved}");
        eprintln!("  has record but NOT empirically saved = {has_record_not_saved}");
        eprintln!("  NO record but empirically saved    = {no_record_but_saved}");
        eprintln!("  NO record and NOT saved            = {no_record_not_saved}");
        eprintln!("\n  names saved but WITHOUT any record:");
        for (name, slot_ref) in &no_record_saved_names {
            eprintln!("    slot={slot_ref:>5} {name}");
        }
    }
}

/// Check ALL per-name metadata for saved vs unsaved TABLE names:
/// records (via slot_ref matching f[12]), section-6 refs, section-4 refs.
#[test]
#[ignore]
fn test_debug_table_name_all_metadata() {
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end().unwrap();

    let vdf = vdf_file(vdf_path);
    let empirical_map = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
    let empirical_norm: HashSet<String> = empirical_map
        .keys()
        .map(|id| normalize_vdf_name(id.as_str()))
        .collect();

    // Build record lookup: f[12] -> list of records
    let mut records_by_slot: HashMap<u32, Vec<&VdfRecord>> = HashMap::new();
    for rec in &vdf.records {
        records_by_slot.entry(rec.fields[12]).or_default().push(rec);
    }

    // Build section-6 ref set: which slot_refs appear in section-6 entries
    let sec6_ref_set: HashSet<u32> = vdf
        .parse_section6_ref_stream()
        .map(|(_, entries, _)| entries.iter().flat_map(|e| e.refs.iter().copied()).collect())
        .unwrap_or_default();

    // Build section-4 ref set
    let sec4_refs: HashSet<u32> = if let Some((_, entries, _)) = parse_debug_section4_entries(&vdf) {
        entries.iter().flat_map(|e| e.refs.iter().copied()).collect()
    } else {
        HashSet::new()
    };

    let sec1 = vdf.slot_section().unwrap();
    let sec1_start = sec1.data_offset();

    eprintln!("\n=== ALL metadata for TABLE/LOOKUP names (WRLD3) ===");

    for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
        let Some(name) = vdf.names.get(i) else { continue };
        let lower = name.to_lowercase();
        if !lower.contains(" table") && !lower.contains(" lookup") {
            continue;
        }

        let normalized = normalize_vdf_name(name);
        let is_saved = empirical_norm.contains(&normalized);
        let has_records = records_by_slot.contains_key(&slot_ref);
        let in_sec6 = sec6_ref_set.contains(&slot_ref);
        let in_sec4 = sec4_refs.contains(&slot_ref);

        // Read slot words
        let abs_off = sec1_start + slot_ref as usize;
        let words = if abs_off + 16 <= vdf.data.len() {
            [
                read_u32(&vdf.data, abs_off),
                read_u32(&vdf.data, abs_off + 4),
                read_u32(&vdf.data, abs_off + 8),
                read_u32(&vdf.data, abs_off + 12),
            ]
        } else {
            [0; 4]
        };

        // Check record fields if present
        let rec_info = if let Some(recs) = records_by_slot.get(&slot_ref) {
            recs.iter()
                .map(|r| format!("f0={} f1={} f10={} f11={}", r.fields[0], r.fields[1], r.fields[10], r.fields[11]))
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            String::new()
        };

        let tag = if is_saved { "SAVED" } else { "     " };
        let rec_tag = if has_records { "REC" } else { "   " };
        let s6_tag = if in_sec6 { "S6" } else { "  " };
        let s4_tag = if in_sec4 { "S4" } else { "  " };

        eprintln!("  {tag} {rec_tag} {s6_tag} {s4_tag} slot={slot_ref:>5} w=[{:08x} {:08x} {:08x} {:08x}] {name}",
            words[0], words[1], words[2], words[3]);
        if !rec_info.is_empty() {
            eprintln!("                          records: {rec_info}");
        }
    }
}

/// Now that we know ALL lookupish names have OTs (via display records),
/// figure out which non-lookupish names are the remaining participants.
/// Count every category of VDF name and reconcile against OT capacity.
#[test]
#[ignore]
fn test_debug_full_participant_accounting() {
    for (label, vdf_path) in [
        ("econ", "../../test/bobby/vdf/econ/base.vdf"),
        ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
    ] {
        let vdf = vdf_file(vdf_path);
        let display_records = vdf.section6_display_records().unwrap_or_default();
        let codes = vdf.section6_ot_class_codes().unwrap();

        let ot_capacity = vdf.offset_table_count - 1; // excluding Time
        let stock_count = codes.iter().skip(1).filter(|&&c| c == VDF_SECTION6_OT_CODE_STOCK).count();
        let dynamic_count = codes.iter().skip(1).filter(|&&c| c == 0x11).count();
        let constant_count = codes.iter().skip(1).filter(|&&c| c == 0x17).count();
        let display_ot_count = display_records.len(); // = lookupish OTs

        // Categorize every name in the slotted prefix
        let system_names: HashSet<&str> = SYSTEM_NAMES.into_iter().collect();
        let mut cat_structural = 0usize; // starts with . - : "
        let mut cat_system = 0usize;     // Time, INITIAL TIME, etc.
        let mut cat_builtin = 0usize;    // abs, cos, step, etc.
        let mut cat_placeholder = 0usize; // single-char non-alpha
        let mut cat_metadata = 0usize;   // is_vdf_metadata_entry (module IO names)
        let mut cat_lookupish = 0usize;  // contains " lookup" or " table"
        let mut cat_hash = 0usize;       // starts with #
        let mut cat_helper = 0usize;     // participant helper (DEL, LV1, etc.)
        let mut cat_regular = 0usize;    // everything else
        let mut cat_regular_names: Vec<String> = Vec::new();

        let mut seen: HashSet<String> = HashSet::new();

        for name in vdf.names.iter().take(vdf.slot_table.len()) {
            let normalized = normalize_vdf_name(name);
            if !seen.insert(normalized.clone()) {
                continue; // skip duplicates
            }

            if name.is_empty() || name.starts_with('.') || name.starts_with('-')
                || name.starts_with(':') || name.starts_with('"') {
                cat_structural += 1;
            } else if name == "Time" {
                // Time = OT[0], handled separately
            } else if system_names.contains(name.as_str()) {
                cat_system += 1;
            } else if name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()) {
                cat_placeholder += 1;
            } else if VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
                cat_builtin += 1;
            } else if is_vdf_metadata_entry(name) {
                cat_metadata += 1; // IN, INI, OUTPUT, SMOOTH, DELAY1, etc.
            } else if name.starts_with('#') {
                cat_hash += 1;
            } else if STDLIB_PARTICIPANT_HELPERS.contains(&name.as_str()) {
                cat_helper += 1;
            } else {
                let lower = name.to_lowercase();
                if lower.contains(" lookup") || lower.contains(" table") {
                    cat_lookupish += 1;
                } else {
                    cat_regular += 1;
                    cat_regular_names.push(name.clone());
                }
            }
        }

        let participant_total = cat_system + cat_hash + cat_helper + cat_lookupish + cat_regular;

        eprintln!("\n=== full participant accounting: {label} ===");
        eprintln!("  OT capacity (excl Time) = {ot_capacity}");
        eprintln!("  section-6: stock={stock_count} dynamic={dynamic_count} constant={constant_count}");
        eprintln!("  display records (lookupish OTs) = {display_ot_count}");
        eprintln!("  non-lookupish OTs = {}", ot_capacity - display_ot_count);
        eprintln!();
        eprintln!("  Name table categories (unique normalized names):");
        eprintln!("    structural (., -, :, \")  = {cat_structural}");
        eprintln!("    Time                     = 1 (OT[0])");
        eprintln!("    system (excl Time)       = {cat_system}");
        eprintln!("    builtins                 = {cat_builtin}");
        eprintln!("    placeholders             = {cat_placeholder}");
        eprintln!("    metadata (mod IO/names)  = {cat_metadata}");
        eprintln!("    #-prefixed               = {cat_hash}");
        eprintln!("    participant helpers       = {cat_helper}");
        eprintln!("    lookupish                = {cat_lookupish}");
        eprintln!("    regular model vars        = {cat_regular}");
        eprintln!();
        eprintln!("  Candidate participants = system + hash + helper + lookupish + regular");
        eprintln!("    = {cat_system} + {cat_hash} + {cat_helper} + {cat_lookupish} + {cat_regular} = {participant_total}");
        eprintln!("  OT capacity = {ot_capacity}");
        eprintln!("  excess = {}", participant_total as isize - ot_capacity as isize);

        if participant_total > ot_capacity && label == "wrld3" {
            // Cross-reference VDF regular names against model variable idents
            // to find which VDF names are NOT actual model variables.
            let contents = std::fs::read_to_string("../../test/metasd/WRLD3-03/wrld3-03.mdl").unwrap();
            let datamodel = crate::compat::open_vensim(&contents).unwrap();
            let model = datamodel.models.iter().find(|m| m.name == "main").unwrap();

            let model_idents: HashSet<String> = model.variables.iter()
                .map(|v| normalize_vdf_name(v.get_ident()))
                .collect();

            let mut in_model = Vec::new();
            let mut not_in_model = Vec::new();
            for name in &cat_regular_names {
                let normalized = normalize_vdf_name(name);
                if model_idents.contains(&normalized) {
                    in_model.push(name.clone());
                } else {
                    not_in_model.push(name.clone());
                }
            }

            not_in_model.sort_by_key(|a| a.to_lowercase());
            eprintln!("\n  regular names NOT in model ({}):", not_in_model.len());
            for name in &not_in_model {
                eprintln!("    {name}");
            }
            eprintln!("  regular names IN model: {}", in_model.len());
            eprintln!("  model variables total: {}", model_idents.len());

            // Check which "builtin" filtered names are actually model variables
            let mut builtin_in_model = Vec::new();
            for name in vdf.names.iter().take(vdf.slot_table.len()) {
                if VENSIM_BUILTINS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
                    let normalized = normalize_vdf_name(name);
                    let in_mdl = model_idents.contains(&normalized);
                    builtin_in_model.push((name.clone(), in_mdl));
                }
            }
            eprintln!("\n  'builtin' names found in VDF name table:");
            for (name, in_mdl) in &builtin_in_model {
                let tag = if *in_mdl { "IN MODEL" } else { "not model" };
                eprintln!("    {tag}: {name}");
            }

            // Check which "metadata" filtered names are model variables
            let mut metadata_in_model = Vec::new();
            for name in vdf.names.iter().take(vdf.slot_table.len()) {
                if is_vdf_metadata_entry(name) && !name.starts_with('.') && !name.starts_with('-')
                    && !name.starts_with(':') && !name.starts_with('"') {
                    let normalized = normalize_vdf_name(name);
                    let in_mdl = model_idents.contains(&normalized);
                    metadata_in_model.push((name.clone(), in_mdl));
                }
            }
            eprintln!("\n  'metadata' names found in VDF name table:");
            for (name, in_mdl) in &metadata_in_model {
                let tag = if *in_mdl { "IN MODEL" } else { "not model" };
                eprintln!("    {tag}: {name}");
            }
        }
    }
}

/// Check if section-6 display records correspond to lookupish names.
/// The display record count (55 for WRLD3) matches the lookupish name
/// count. If each display record encodes a lookup table entry, the
/// OT index field (word[10]) might distinguish saved (valid OT) from
/// unsaved (invalid/zero OT).
#[test]
#[ignore]
fn test_debug_display_records_vs_lookupish_names() {
    for (label, vdf_path) in [
        ("econ", "../../test/bobby/vdf/econ/base.vdf"),
        ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
    ] {
        let vdf = vdf_file(vdf_path);

        let display_records = vdf.section6_display_records().unwrap_or_default();
        let codes = vdf.section6_ot_class_codes();

        // Count lookupish names
        let lookupish_count = vdf.names.iter().take(vdf.slot_table.len())
            .filter(|n| {
                let lower = n.to_lowercase();
                lower.contains(" table") || lower.contains(" lookup")
            })
            .count();

        eprintln!("\n=== display records vs lookupish names: {label} ===");
        eprintln!("  display records: {}", display_records.len());
        eprintln!("  lookupish names: {lookupish_count}");

        // Show all display records with their OT index
        for (i, rec) in display_records.iter().enumerate() {
            let ot = rec.ot_index();
            let code_at_ot = codes.as_ref().and_then(|c| c.get(ot).copied());
            let has_valid_ot = ot > 0 && ot < vdf.offset_table_count;

            // Check if the OT entry is an inline constant or data block
            let ot_val = vdf.offset_table_entry(ot);
            let is_data_block = ot_val.map(|v| vdf.is_data_block_offset(v)).unwrap_or(false);

            eprintln!("  display[{i:>2}]: ot={ot:>3} valid={has_valid_ot} code={:?} data_block={is_data_block} words[0..3]=[{:08x} {:08x} {:08x}]",
                code_at_ot,
                rec.words[0], rec.words[1], rec.words[2]);
        }
    }
}

/// Compare companion variable slot data with TABLE name slot data.
/// Check if the companion's slot words contain the TABLE's slot_ref
/// (a cross-reference that might encode the attached/standalone distinction).
#[test]
#[ignore]
fn test_debug_companion_vs_table_slot_crossref() {
    let vdf = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

    let contents = std::fs::read_to_string("../../test/metasd/WRLD3-03/wrld3-03.mdl").unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end().unwrap();
    let empirical_map = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
    let empirical_norm: HashSet<String> = empirical_map
        .keys()
        .map(|id| normalize_vdf_name(id.as_str()))
        .collect();

    let sec1 = vdf.slot_section().unwrap();
    let sec1_start = sec1.data_offset();

    // Build name -> (index, slot_ref) lookup
    let mut name_to_slot: HashMap<String, (usize, u32)> = HashMap::new();
    for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
        if let Some(name) = vdf.names.get(i) {
            name_to_slot.insert(normalize_vdf_name(name), (i, slot_ref));
        }
    }

    // Known companion pairs (from the MDL analysis above)
    let pairs: Vec<(&str, &str)> = vec![
        ("capacity utilization fraction table", "capacity utilization fraction"),
        ("assimilation half life mult table", "assimilation half life multiplier"),
        ("crowding multiplier from industry table", "crowding multiplier from industry"),
        ("development cost per hectare table", "development cost per hectare"),
        ("mortality 0 to 14 table", "mortality 0 to 14"),
        ("indicated food per capita table 1", "indicated food per capita 1"),
        ("indicated food per capita table 2", "indicated food per capita 2"),
        ("land yield multiplier from capital table", "land yield multiplier from capital"),
        ("fraction of industrial output allocated to services table 1", "fraction of industrial output allocated to services 1"),
    ];

    eprintln!("\n=== companion vs TABLE slot cross-references (WRLD3) ===");
    for (table_name, companion_name) in &pairs {
        let tn = normalize_vdf_name(table_name);
        let cn = normalize_vdf_name(companion_name);
        let is_saved = empirical_norm.contains(&tn);

        let Some(&(_, t_slot)) = name_to_slot.get(&tn) else { continue };
        let Some(&(_, c_slot)) = name_to_slot.get(&cn) else {
            eprintln!("  companion not found: {companion_name}");
            continue;
        };

        // Read both slot blobs (first 16 bytes)
        let t_abs = sec1_start + t_slot as usize;
        let c_abs = sec1_start + c_slot as usize;
        if t_abs + 16 > vdf.data.len() || c_abs + 16 > vdf.data.len() { continue; }

        let t_words: Vec<u32> = (0..4).map(|j| read_u32(&vdf.data, t_abs + j*4)).collect();
        let c_words: Vec<u32> = (0..4).map(|j| read_u32(&vdf.data, c_abs + j*4)).collect();

        // Check if any word in the companion contains the TABLE's slot_ref or vice versa
        let c_refs_t = c_words.contains(&t_slot);
        let t_refs_c = t_words.contains(&c_slot);

        let tag = if is_saved { "SAVED" } else { "     " };
        eprintln!("  {tag} {table_name}");
        eprintln!("    TABLE  slot={t_slot:>5} words=[{:08x} {:08x} {:08x} {:08x}]",
            t_words[0], t_words[1], t_words[2], t_words[3]);
        eprintln!("    COMPAN slot={c_slot:>5} words=[{:08x} {:08x} {:08x} {:08x}]{}{}",
            c_words[0], c_words[1], c_words[2], c_words[3],
            if c_refs_t { " **REFS TABLE**" } else { "" },
            if t_refs_c { " **TABLE REFS COMPANION**" } else { "" });
    }
}

/// Bit-level comparison of slot data between saved and unsaved TABLE names
/// to find a VDF-structural flag encoding whether a lookup has an OT entry.
#[test]
#[ignore]
fn test_debug_table_slot_bitwise_comparison() {
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end().unwrap();

    let vdf = vdf_file(vdf_path);
    let empirical_map = build_empirical_ot_map(&vdf.extract_data().unwrap(), &results).unwrap();
    let empirical_norm: HashSet<String> = empirical_map
        .keys()
        .map(|id| normalize_vdf_name(id.as_str()))
        .collect();

    let sec1 = vdf.slot_section().unwrap();
    let sec1_start = sec1.data_offset();

    // For each TABLE name, read the full slot blob and compare
    // saved vs unsaved at the bit level
    let mut saved_words: Vec<([u32; 4], String)> = Vec::new();
    let mut unsaved_words: Vec<([u32; 4], String)> = Vec::new();

    for (i, &slot_ref) in vdf.slot_table.iter().enumerate() {
        let Some(name) = vdf.names.get(i) else { continue };
        let lower = name.to_lowercase();
        if !lower.contains(" table") && !lower.contains(" lookup") {
            continue;
        }

        let abs_off = sec1_start + slot_ref as usize;
        if abs_off + 16 > vdf.data.len() { continue; }

        let words = [
            read_u32(&vdf.data, abs_off),
            read_u32(&vdf.data, abs_off + 4),
            read_u32(&vdf.data, abs_off + 8),
            read_u32(&vdf.data, abs_off + 12),
        ];

        let normalized = normalize_vdf_name(name);
        if empirical_norm.contains(&normalized) {
            saved_words.push((words, name.clone()));
        } else {
            unsaved_words.push((words, name.clone()));
        }
    }

    // Find bits that are consistently 1 for saved and 0 for unsaved (or vice versa)
    // across all 4 words (128 bits total)
    eprintln!("\n=== TABLE name slot bitwise comparison (WRLD3) ===");
    eprintln!("  saved: {} names, unsaved: {} names", saved_words.len(), unsaved_words.len());

    for word_idx in 0..4 {
        // For each bit position, count how many saved/unsaved have it set
        for bit in 0..32 {
            let mask = 1u32 << bit;
            let saved_set = saved_words.iter().filter(|(w, _)| w[word_idx] & mask != 0).count();
            let unsaved_set = unsaved_words.iter().filter(|(w, _)| w[word_idx] & mask != 0).count();

            // Report bits that are highly discriminating
            let saved_pct = if saved_words.is_empty() { 0.0 } else { saved_set as f64 / saved_words.len() as f64 };
            let unsaved_pct = if unsaved_words.is_empty() { 0.0 } else { unsaved_set as f64 / unsaved_words.len() as f64 };

            if (saved_pct - unsaved_pct).abs() > 0.4 {
                eprintln!("  word[{word_idx}] bit {bit:>2}: saved={saved_set}/{} ({:.0}%)  unsaved={unsaved_set}/{} ({:.0}%)  delta={:.0}%",
                    saved_words.len(), saved_pct*100.0,
                    unsaved_words.len(), unsaved_pct*100.0,
                    (saved_pct - unsaved_pct)*100.0);
            }
        }
    }

    // Also show the raw words for all entries
    eprintln!("\n  SAVED entries:");
    for (words, name) in &saved_words {
        eprintln!("    [{:08x} {:08x} {:08x} {:08x}] {name}", words[0], words[1], words[2], words[3]);
    }
    eprintln!("\n  UNSAVED entries (first 20):");
    for (words, name) in unsaved_words.iter().take(20) {
        eprintln!("    [{:08x} {:08x} {:08x} {:08x}] {name}", words[0], words[1], words[2], words[3]);
    }
}

// ---- Lookup table location tests ----

/// Search for known f32 lookup table data points in VDF binary data.
/// Returns all byte offsets where a contiguous sequence of the given
/// f32 values appears.
fn find_f32_sequence(data: &[u8], values: &[f32]) -> Vec<usize> {
    if values.is_empty() || data.len() < values.len() * 4 {
        return Vec::new();
    }
    let needle: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut hits = Vec::new();
    for pos in 0..=data.len() - needle.len() {
        if data[pos..pos + needle.len()] == needle[..] {
            hits.push(pos);
        }
    }
    hits
}

/// Same as find_f32_sequence but for f64.
fn find_f64_sequence(data: &[u8], values: &[f64]) -> Vec<usize> {
    if values.is_empty() || data.len() < values.len() * 8 {
        return Vec::new();
    }
    let needle: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut hits = Vec::new();
    for pos in 0..=data.len() - needle.len() {
        if data[pos..pos + needle.len()] == needle[..] {
            hits.push(pos);
        }
    }
    hits
}

/// Search for known lookup table data points in VDF files to locate
/// where graphical function definitions are stored.
///
/// Strategy: take the (x,y) pairs from an MDL lookup definition,
/// search for their f32 representations as contiguous sequences in
/// the VDF binary, and report all matches with surrounding context.
#[test]
#[ignore]
fn test_debug_find_lookup_table_data_in_vdf() {
    // econ: "hud policy lookup" has distinctive data points
    // [(108,0)-(800,1)],(0,1),(108,1),(112.771,0.973684),(116.147,0.916667),
    // (118.128,0.811404),(119.817,0.701754),(122.752,0.587719),(127.229,0.528509),
    // (132,0.5),(199,0.5),(200,1),(600,1)
    let hud_x: Vec<f32> = vec![
        0.0, 108.0, 112.771, 116.147, 118.128, 119.817, 122.752,
        127.229, 132.0, 199.0, 200.0, 600.0,
    ];
    let hud_y: Vec<f32> = vec![
        1.0, 1.0, 0.973684, 0.916667, 0.811404, 0.701754, 0.587719,
        0.528509, 0.5, 0.5, 1.0, 1.0,
    ];

    // econ: "loan standards impact on insolvency table"
    // [(0,-0.01)-(1,0.1)],(0,0.1),(0.412844,0.0884211),
    // (0.587156,0.0526316),(0.761468,-0.00263158),(1,-0.01)
    let loan_x: Vec<f32> = vec![0.0, 0.412844, 0.587156, 0.761468, 1.0];
    let loan_y: Vec<f32> = vec![0.1, 0.0884211, 0.0526316, -0.00263158, -0.01];

    let vdf_path = "../../test/bobby/vdf/econ/base.vdf";
    let data = std::fs::read(vdf_path).unwrap();
    let vdf = vdf_file(vdf_path);

    eprintln!("\n=== searching for lookup table data in econ VDF ({} bytes) ===", data.len());

    // Search for x-values as contiguous f32 sequences
    // Try different subsequence lengths (3+ consecutive values)
    for (label, values) in [
        ("hud_x (first 4)", &hud_x[..4]),
        ("hud_x (last 4)", &hud_x[hud_x.len()-4..]),
        ("hud_y (first 4)", &hud_y[..4]),
        ("hud_y (last 4)", &hud_y[hud_y.len()-4..]),
        ("hud_x (all 12)", &hud_x[..]),
        ("hud_y (all 12)", &hud_y[..]),
        ("loan_x (all 5)", &loan_x[..]),
        ("loan_y (all 5)", &loan_y[..]),
    ] {
        let hits = find_f32_sequence(&data, values);
        eprintln!("  f32 {label}: {} hit(s)", hits.len());
        for &offset in hits.iter().take(3) {
            eprintln!("    offset=0x{offset:06x} ({offset})");
            // Show which section this falls in
            for (i, sec) in vdf.sections.iter().enumerate() {
                if offset >= sec.file_offset && offset < sec.region_end {
                    eprintln!("      in section[{i}] (offset +{} into region, field4={})",
                        offset - sec.file_offset, sec.field4);
                    break;
                }
            }
        }
    }

    // Also try interleaved x,y pairs: (x0,y0,x1,y1,...)
    let hud_interleaved: Vec<f32> = hud_x.iter().zip(hud_y.iter())
        .flat_map(|(&x, &y)| [x, y])
        .collect();
    let loan_interleaved: Vec<f32> = loan_x.iter().zip(loan_y.iter())
        .flat_map(|(&x, &y)| [x, y])
        .collect();

    for (label, values) in [
        ("hud_xy interleaved (first 6 vals)", &hud_interleaved[..6]),
        ("hud_xy interleaved (all)", &hud_interleaved[..]),
        ("loan_xy interleaved (all)", &loan_interleaved[..]),
    ] {
        let hits = find_f32_sequence(&data, values);
        eprintln!("  f32 {label}: {} hit(s)", hits.len());
        for &offset in hits.iter().take(3) {
            eprintln!("    offset=0x{offset:06x} ({offset})");
            for (i, sec) in vdf.sections.iter().enumerate() {
                if offset >= sec.file_offset && offset < sec.region_end {
                    eprintln!("      in section[{i}] (offset +{} into region, field4={})",
                        offset - sec.file_offset, sec.field4);
                    break;
                }
            }
        }
    }

    // Try f64 as well (unlikely but worth checking)
    let hud_x_f64: Vec<f64> = hud_x.iter().map(|&v| v as f64).collect();
    let hits_f64 = find_f64_sequence(&data, &hud_x_f64[..4]);
    eprintln!("  f64 hud_x (first 4): {} hit(s)", hits_f64.len());

    // Now that we found the hud table: dump surrounding context to understand
    // the framing structure. x-values at 0x380a, y-values at 0x383a.
    // x has 12 values (48 bytes), y has 12 values (48 bytes).
    // Check what comes before and after.
    let hud_x_offset = 0x380a_usize;
    let hud_y_offset = 0x383a_usize;
    let hud_y_end = hud_y_offset + 12 * 4; // 0x386a

    // Dump 32 bytes before x-values as u32/f32
    eprintln!("\n  context around hud policy lookup table:");
    let pre_start = hud_x_offset.saturating_sub(32);
    eprintln!("  bytes before x-values (from 0x{pre_start:06x}):");
    for off in (pre_start..hud_x_offset).step_by(4) {
        let u = read_u32(&data, off);
        let f = read_f32(&data, off);
        eprintln!("    0x{off:06x}: u32={u:>10} (0x{u:08x})  f32={f:>12.4}");
    }

    // Check if there's a count or header between x and y arrays
    eprintln!("  gap between x-end (0x{:06x}) and y-start (0x{hud_y_offset:06x}): {} bytes",
        hud_x_offset + 48, hud_y_offset - (hud_x_offset + 48));

    // Dump 32 bytes after y-values
    eprintln!("  bytes after y-values (from 0x{hud_y_end:06x}):");
    for off in (hud_y_end..hud_y_end + 32).step_by(4) {
        if off + 4 > data.len() { break; }
        let u = read_u32(&data, off);
        let f = read_f32(&data, off);
        eprintln!("    0x{off:06x}: u32={u:>10} (0x{u:08x})  f32={f:>12.4}");
    }

    // Try searching for loan table values with slight rounding
    // The MDL has values like 0.412844 -- check what f32 representation gives us
    eprintln!("\n  loan table f32 precision check:");
    for &v in &loan_x {
        eprintln!("    {v} -> bytes = {:?}", v.to_le_bytes());
    }
    for &v in &loan_y {
        eprintln!("    {v} -> bytes = {:?}", v.to_le_bytes());
    }

    // Try searching for just the distinctive loan y-values as f32
    let loan_y_distinctive: Vec<f32> = vec![0.0884211_f32, 0.0526316_f32];
    let hits = find_f32_sequence(&data, &loan_y_distinctive);
    eprintln!("  f32 loan_y distinctive pair: {} hit(s)", hits.len());
    for &offset in hits.iter().take(3) {
        eprintln!("    offset=0x{offset:06x}");
    }

    // Dump the full section-7 region leading up to the hud table to find
    // the framing structure (headers, counts, table boundaries).
    let sec7 = &vdf.sections[7];
    let sec7_data_start = sec7.data_offset();
    eprintln!("\n  section 7: file_offset=0x{:06x} data_start=0x{sec7_data_start:06x} region_end=0x{:06x} field1={} field4={}",
        sec7.file_offset, sec7.region_end, sec7.field1, sec7.field4);

    // Dump just the first 80 words of section 7 to see the header structure
    eprintln!("\n  section 7 data dump (first 80 words):");
    let dump_end = (sec7_data_start + 320).min(sec7.region_end).min(data.len());
    for off in (sec7_data_start..dump_end).step_by(4) {
        let f = read_f32(&data, off);
        let u = read_u32(&data, off);
        let rel = off - sec7_data_start;
        // Annotate known positions
        let annotation = if off == hud_x_offset {
            " <-- hud x-values start"
        } else if off == hud_y_offset {
            " <-- hud y-values start"
        } else if off == hud_y_end {
            " <-- hud y-values end"
        } else {
            ""
        };
        eprintln!("    +{rel:>4} 0x{off:06x}: f32={f:>14.6}  u32=0x{u:08x}{annotation}");
    }
}

/// Same search on WRLD3 -- use a distinctive table with unique values.
#[test]
#[ignore]
fn test_debug_find_lookup_table_data_in_wrld3() {
    // WRLD3: "capacity utilization fraction table" (SAVED, has OT entry)
    // (1,1),(3,0.9),(5,0.7),(7,0.3),(9,0.1),(11,0.1)
    let cuf_x: Vec<f32> = vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let cuf_y: Vec<f32> = vec![1.0, 0.9, 0.7, 0.3, 0.1, 0.1];

    // WRLD3: "development cost per hectare table" (NOT saved)
    // (0,100000),(0.1,7400),(0.2,5200),(0.3,3500),(0.4,2400),(0.5,1500),
    // (0.6,750),(0.7,300),(0.8,150),(0.9,75),(1,50)
    let dev_x: Vec<f32> = vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
    let dev_y: Vec<f32> = vec![100000.0, 7400.0, 5200.0, 3500.0, 2400.0, 1500.0,
                               750.0, 300.0, 150.0, 75.0, 50.0];

    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let data = std::fs::read(vdf_path).unwrap();
    let vdf = vdf_file(vdf_path);

    eprintln!("\n=== searching for lookup table data in WRLD3 VDF ({} bytes) ===", data.len());

    // Try contiguous x, contiguous y, and interleaved x/y
    let cuf_interleaved: Vec<f32> = cuf_x.iter().zip(cuf_y.iter())
        .flat_map(|(&x, &y)| [x, y])
        .collect();
    let dev_interleaved: Vec<f32> = dev_x.iter().zip(dev_y.iter())
        .flat_map(|(&x, &y)| [x, y])
        .collect();

    for (label, values) in [
        ("cuf_x (all 6)", &cuf_x[..]),
        ("cuf_y (all 6)", &cuf_y[..]),
        ("cuf_xy interleaved (all 12)", &cuf_interleaved[..]),
        ("cuf_xy interleaved (first 6)", &cuf_interleaved[..6]),
        ("dev_x (all 11)", &dev_x[..]),
        ("dev_y (all 11)", &dev_y[..]),
        ("dev_y (first 5)", &dev_y[..5]),
        ("dev_xy interleaved (first 6)", &dev_interleaved[..6]),
        ("dev_xy interleaved (all 22)", &dev_interleaved[..]),
    ] {
        let hits = find_f32_sequence(&data, values);
        eprintln!("  f32 {label}: {} hit(s)", hits.len());
        for &offset in hits.iter().take(3) {
            eprintln!("    offset=0x{offset:06x} ({offset})");
            for (i, sec) in vdf.sections.iter().enumerate() {
                if offset >= sec.file_offset && offset < sec.region_end {
                    eprintln!("      in section[{i}] (offset +{} into section, field4={})",
                        offset - sec.file_offset, sec.field4);
                    break;
                }
            }
        }
    }
}
