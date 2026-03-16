// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;
use std::error::Error;

use simlin_engine::vdf::{
    FILE_HEADER_SIZE, RECORD_SIZE, SYSTEM_NAMES, Section, VDF_SECTION6_OT_CODE_STOCK,
    VDF_SECTION6_OT_CODE_TIME, VDF_SENTINEL, VENSIM_BUILTINS, VdfFile, read_f32, read_u16,
    read_u32,
};

const SECTION_ROLES: [&str; 8] = [
    "model info",
    "string table",
    "name table",
    "unknown (zeros)",
    "unknown metadata",
    "degenerate/marker",
    "unknown metadata",
    "lookup + OT + data",
];

/// Max bytes of section data to hexdump.
const MAX_HEXDUMP: usize = 256;

pub fn dump_vdf(path: &str) -> Result<(), Box<dyn Error>> {
    let data = std::fs::read(path)?;
    let file_size = data.len();
    let vdf = VdfFile::parse(data)?;

    print_header(&vdf, file_size, path);
    print_layout(&vdf);
    print_sections(&vdf);
    print_names(&vdf);
    print_slots(&vdf);
    print_records(&vdf);
    print_record_ot_ranges(&vdf);
    print_ref_streams(&vdf);
    print_section6_ot_codes(&vdf);
    print_section6_tail(&vdf);
    print_offset_table(&vdf);
    print_data_blocks(&vdf);
    print_gaps(&vdf, file_size);
    print_summary(&vdf, file_size);

    Ok(())
}

fn print_header(vdf: &VdfFile, file_size: usize, path: &str) {
    println!("=== VDF File: {} ===", path);
    println!("File size:    {} bytes", file_size);

    let ts_bytes = &vdf.data[4..0x78];
    let ts_end = ts_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(ts_bytes.len());
    let timestamp = std::str::from_utf8(&ts_bytes[..ts_end]).unwrap_or("(unparseable)");
    println!("Timestamp:    {}", timestamp);
    println!("Time points:  {}", vdf.time_point_count);
    println!("Bitmap size:  {} bytes", vdf.bitmap_size);
    println!();
}

fn print_layout(vdf: &VdfFile) {
    println!("=== File Layout ===");

    let mut entries: Vec<(usize, String)> = Vec::new();

    entries.push((0, format!("File header ({} bytes)", FILE_HEADER_SIZE)));

    for (i, sec) in vdf.sections.iter().enumerate() {
        let role = SECTION_ROLES.get(i).copied().unwrap_or("unknown");
        let region_size = sec.region_end - sec.file_offset;
        entries.push((
            sec.file_offset,
            format!("Section {}: {} (region {}B)", i, role, region_size),
        ));
    }

    if !vdf.records.is_empty() {
        let start = vdf.records.first().unwrap().file_offset;
        let count = vdf.records.len();
        entries.push((
            start,
            format!("Records ({}, {} bytes)", count, count * RECORD_SIZE),
        ));
    }

    if vdf.slot_table_offset > 0 {
        entries.push((
            vdf.slot_table_offset,
            format!(
                "Slot table ({} entries, {} bytes)",
                vdf.slot_table.len(),
                vdf.slot_table.len() * 4
            ),
        ));
    }

    entries.push((
        vdf.offset_table_start,
        format!(
            "Offset table ({} entries, {} bytes)",
            vdf.offset_table_count,
            vdf.offset_table_count * 4
        ),
    ));

    entries.push((vdf.first_data_block, "Data blocks start".to_string()));
    entries.push((vdf.data.len(), "End of file".to_string()));

    entries.sort_by_key(|&(off, _)| off);

    for (off, desc) in &entries {
        println!("  0x{:08x}  {}", off, desc);
    }
    println!();
}

fn hexdump(data: &[u8], base_offset: usize, max_bytes: usize) {
    let show = data.len().min(max_bytes);
    for start in (0..show).step_by(16) {
        let end = (start + 16).min(show);
        let chunk = &data[start..end];

        print!("  {:08x}:", base_offset + start);
        for (i, &b) in chunk.iter().enumerate() {
            if i == 8 {
                print!(" ");
            }
            print!(" {:02x}", b);
        }
        for i in chunk.len()..16 {
            if i == 8 {
                print!(" ");
            }
            print!("   ");
        }
        print!("  |");
        for &b in chunk {
            if (0x20..0x7f).contains(&b) {
                print!("{}", b as char);
            } else {
                print!(".");
            }
        }
        for _ in chunk.len()..16 {
            print!(" ");
        }
        println!("|");
    }
    if data.len() > max_bytes {
        println!("  ... ({} more bytes)", data.len() - max_bytes);
    }
}

fn print_section_header(sec: &Section, index: usize) {
    let role = SECTION_ROLES.get(index).copied().unwrap_or("unknown");
    println!();
    println!("Section {} @ 0x{:08x}  [{}]", index, sec.file_offset, role);
    println!(
        "  field3={}  field4={}  field5=0x{:08x}",
        sec.field3, sec.field4, sec.field5
    );
    println!(
        "  region: 0x{:08x}..0x{:08x} ({}B data)",
        sec.data_offset(),
        sec.region_end,
        sec.region_data_size(),
    );
}

fn print_sections(vdf: &VdfFile) {
    println!("=== Sections ({}) ===", vdf.sections.len());

    for (i, sec) in vdf.sections.iter().enumerate() {
        print_section_header(sec, i);

        let data_start = sec.data_offset();
        let region_end = sec.region_end.min(vdf.data.len());
        if data_start >= region_end {
            println!("  (no data / degenerate section)");
            continue;
        }

        let region_data = &vdf.data[data_start..region_end];

        if vdf.name_section_idx == Some(i) {
            println!("  (name table -- shown separately below)");
        } else {
            hexdump(region_data, data_start, MAX_HEXDUMP);
        }
    }
    println!();
}

fn classify_name(name: &str) -> &'static str {
    if SYSTEM_NAMES.contains(&name) {
        "system"
    } else if name.starts_with('.') {
        "group"
    } else if name.starts_with('-') {
        "unit"
    } else if (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric()))
        || VENSIM_BUILTINS
            .iter()
            .any(|&b| b.eq_ignore_ascii_case(name))
    {
        "builtin?"
    } else {
        ""
    }
}

fn print_names(vdf: &VdfFile) {
    let sec_label = vdf
        .name_section_idx
        .map(|i| format!("section {}", i))
        .unwrap_or_else(|| "unknown section".to_string());
    let slotted = vdf.slot_table.len();
    let unslotted = vdf.names.len().saturating_sub(slotted);
    println!(
        "=== Name Table ({} names: {} with slots, {} without, {}) ===",
        vdf.names.len(),
        slotted,
        unslotted,
        sec_label
    );

    for (i, name) in vdf.names.iter().enumerate() {
        let class = classify_name(name);
        if i == slotted && slotted < vdf.names.len() {
            println!("  --- names without slot table entries ---");
        }
        if class.is_empty() {
            println!("  {:>3}  \"{}\"", i, name);
        } else {
            println!("  {:>3}  \"{}\"  ({})", i, name, class);
        }
    }
    println!();
}

fn print_slots(vdf: &VdfFile) {
    if vdf.slot_table.is_empty() {
        println!("=== Slot Table ===");
        println!("  (empty)");
        println!();
        return;
    }

    let sec1_data_start = vdf.sections.get(1).map(|s| s.data_offset()).unwrap_or(0);

    println!(
        "=== Slot Table ({} entries @ 0x{:08x}) ===",
        vdf.slot_table.len(),
        vdf.slot_table_offset
    );
    println!(
        "  {:>3}  {:>7}  {:<36}  {:>8} {:>8} {:>8} {:>8}",
        "Idx", "Sec1Off", "Name", "w[0]", "w[1]", "w[2]", "w[3]"
    );

    for (i, &offset) in vdf.slot_table.iter().enumerate() {
        let name = vdf.names.get(i).map(|s| s.as_str()).unwrap_or("???");
        let abs = sec1_data_start + offset as usize;

        if abs + 16 <= vdf.data.len() {
            let w0 = read_u32(&vdf.data, abs);
            let w1 = read_u32(&vdf.data, abs + 4);
            let w2 = read_u32(&vdf.data, abs + 8);
            let w3 = read_u32(&vdf.data, abs + 12);
            println!(
                "  {:>3}  {:>7}  {:<36}  {:08x} {:08x} {:08x} {:08x}",
                i,
                offset,
                format!("\"{}\"", name),
                w0,
                w1,
                w2,
                w3
            );
        } else {
            println!(
                "  {:>3}  {:>7}  {:<36}  (out of bounds)",
                i,
                offset,
                format!("\"{}\"", name)
            );
        }
    }
    println!();
}

fn fmt_field(val: u32) -> String {
    if val == VDF_SENTINEL {
        "  SENT".to_string()
    } else {
        format!("{:>6}", val)
    }
}

fn print_records(vdf: &VdfFile) {
    println!("=== Variable Metadata Records ({}) ===", vdf.records.len());
    if vdf.records.is_empty() {
        println!("  (none)");
        println!();
        return;
    }

    println!("  SENT = sentinel 0x{:08x}", VDF_SENTINEL);
    println!(
        "  Known: f[0]=type f[1]=sys(23) f[2]=mono_ctr f[10]=sort_key f[11]=ot_idx f[12]=slot_ref"
    );
    println!();

    let mut slot_to_name: std::collections::HashMap<u32, &str> = std::collections::HashMap::new();
    for (i, &slot) in vdf.slot_table.iter().enumerate() {
        if let Some(name) = vdf.names.get(i) {
            slot_to_name.insert(slot, name.as_str());
        }
    }

    // Column header
    print!("  {:>3} {:>10}", "#", "offset");
    for i in 0..16 {
        print!("{:>6}", format!("f{}", i));
    }
    println!("  class slot");

    let ot_count = vdf.offset_table_count;

    for (i, rec) in vdf.records.iter().enumerate() {
        let f = &rec.fields;

        let class = if f[0] == 0 {
            "zero".to_string()
        } else if f[1] == 23 {
            "system".to_string()
        } else if f[10] > 0 && f[11] > 0 && (f[11] as usize) < ot_count {
            format!("model sort={} ot={}", f[10], f[11])
        } else if f[11] > 0 && f[11] as usize >= ot_count {
            format!("ot_oob({})", f[11])
        } else {
            String::new()
        };

        print!("  {:>3} 0x{:08x}", i, rec.file_offset);
        for &val in f {
            print!("{}", fmt_field(val));
        }
        let slot_name = slot_to_name.get(&f[12]).copied().unwrap_or("");
        if slot_name.is_empty() {
            println!("  {}", class);
        } else {
            println!("  {} slot=\"{}\"", class, slot_name);
        }
    }
    println!();
}

fn print_record_ot_ranges(vdf: &VdfFile) {
    println!("=== Record-Derived OT Ranges ===");
    let ranges = vdf.record_ot_ranges();
    if ranges.is_empty() {
        println!("  (none)");
        println!();
        return;
    }

    let covered: usize = ranges.iter().map(|r| r.len()).sum();
    let multi = ranges.iter().filter(|r| r.len() > 1).count();
    println!(
        "  ranges={} covered={} of {} (excluding OT[0]) multi_entry_ranges={}",
        ranges.len(),
        covered,
        vdf.offset_table_count.saturating_sub(1),
        multi
    );
    for (i, r) in ranges.iter().take(20).enumerate() {
        println!(
            "  {:>3}  [{}..{}) len={} records@start={}",
            i,
            r.start,
            r.end,
            r.len(),
            r.record_count
        );
    }
    if ranges.len() > 20 {
        println!("  ... ({} more ranges)", ranges.len() - 20);
    }
    println!();
}

fn print_ref_streams(vdf: &VdfFile) {
    let mut slot_to_name: std::collections::HashMap<u32, &str> = std::collections::HashMap::new();
    for (i, &slot) in vdf.slot_table.iter().enumerate() {
        if let Some(name) = vdf.names.get(i) {
            slot_to_name.insert(slot, name.as_str());
        }
    }

    println!("=== Section 6 Ref Stream ===");
    match vdf.parse_section6_ref_stream() {
        Some((skip, entries, stop)) if !entries.is_empty() => {
            let all_slotted = entries
                .iter()
                .filter(|e| e.slotted_ref_count == e.refs.len())
                .count();
            let mixed = entries
                .iter()
                .filter(|e| e.slotted_ref_count > 0 && e.slotted_ref_count < e.refs.len())
                .count();
            let none = entries.iter().filter(|e| e.slotted_ref_count == 0).count();
            println!(
                "  skip_words={} entries={} stop=0x{:08x} slotted(all/mixed/none)={}/{}/{}",
                skip,
                entries.len(),
                stop,
                all_slotted,
                mixed,
                none
            );
            for (i, e) in entries.iter().take(12).enumerate() {
                let refs: Vec<String> = e
                    .refs
                    .iter()
                    .map(|r| {
                        slot_to_name
                            .get(r)
                            .map(|n| format!("{r}:{n}"))
                            .unwrap_or_else(|| format!("{r}:<sec1>"))
                    })
                    .collect();
                println!(
                    "  {:>3} @0x{:08x} n={} slot_hits={} refs={:?}",
                    i,
                    e.file_offset,
                    e.refs.len(),
                    e.slotted_ref_count,
                    refs
                );
            }
            if entries.len() > 12 {
                println!("  ... ({} more entries)", entries.len() - 12);
            }

            let interesting_names: HashSet<&str> = HashSet::from([
                "IN", "INI", "DEL", "LV1", "LV2", "LV3", "ST", "RT1", "RT2", "DL", "OUTPUT",
                "SMOOTH", "SMOOTHI", "SMOOTH3", "SMOOTH3I", "DELAY1", "DELAY1I", "DELAY3",
                "DELAY3I", "TREND", "NPV",
            ]);
            let interesting_entries: Vec<(usize, Vec<String>)> = entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    let refs: Vec<String> = e
                        .refs
                        .iter()
                        .filter_map(|r| {
                            let name = slot_to_name.get(r).copied()?;
                            (name.starts_with('#') || interesting_names.contains(name))
                                .then(|| format!("{r}:{name}"))
                        })
                        .collect();
                    (!refs.is_empty()).then_some((i, refs))
                })
                .collect();
            if !interesting_entries.is_empty() {
                println!("  interesting entries (module helpers / #names):");
                for (i, refs) in interesting_entries.iter().take(40) {
                    println!("    {:>3}: {:?}", i, refs);
                }
                if interesting_entries.len() > 40 {
                    println!(
                        "    ... ({} more interesting entries)",
                        interesting_entries.len() - 40
                    );
                }
            }
        }
        _ => println!("  (none)"),
    }
    println!();

    println!("=== Section 5 Set Stream ===");
    match vdf.parse_section5_set_stream() {
        Some((skip, entries, stop)) if !entries.is_empty() => {
            println!(
                "  skip_words={} sets={} stop=0x{:08x}",
                skip,
                entries.len(),
                stop
            );
            for (i, e) in entries.iter().take(8).enumerate() {
                let refs: Vec<String> = e
                    .refs
                    .iter()
                    .take(8)
                    .map(|r| {
                        slot_to_name
                            .get(r)
                            .map(|n| format!("{r}:{n}"))
                            .unwrap_or_else(|| format!("{r}:<sec1>"))
                    })
                    .collect();
                println!(
                    "  {:>3} @0x{:08x} n={} size={} dim={} slot_hits={} refs(head)={:?}",
                    i,
                    e.file_offset,
                    e.n,
                    e.refs.len(),
                    e.dimension_size(),
                    e.slotted_ref_count,
                    refs
                );
            }
            if entries.len() > 8 {
                println!("  ... ({} more sets)", entries.len() - 8);
            }
        }
        _ => println!("  (none)"),
    }
    println!();
}

fn section6_code_label(code: u8) -> &'static str {
    match code {
        VDF_SECTION6_OT_CODE_TIME => "time",
        VDF_SECTION6_OT_CODE_STOCK => "stock",
        0x11 => "dynamic?",
        0x17 => "const/system?",
        _ => "unknown",
    }
}

fn print_section6_ot_codes(vdf: &VdfFile) {
    println!("=== Section 6 OT Class Codes ===");
    let Some(codes) = vdf.section6_ot_class_codes() else {
        println!("  (none)");
        println!();
        return;
    };

    let stock_count = codes
        .iter()
        .skip(1)
        .filter(|&&code| code == VDF_SECTION6_OT_CODE_STOCK)
        .count();
    let first_non_stock = codes
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(i, &code)| (code != VDF_SECTION6_OT_CODE_STOCK).then_some(i));

    let mut counts = std::collections::BTreeMap::<u8, usize>::new();
    for &code in &codes {
        *counts.entry(code).or_default() += 1;
    }

    println!(
        "  codes={} stock_like={} first_non_stock={}",
        codes.len(),
        stock_count,
        first_non_stock
            .map(|idx| format!("OT[{idx}]"))
            .unwrap_or_else(|| "none".to_string())
    );
    for (code, count) in counts {
        println!(
            "  code=0x{code:02x}  count={count:>3}  label={}",
            section6_code_label(code)
        );
    }

    println!("  First 40 codes:");
    for (i, code) in codes.iter().take(40).enumerate() {
        println!(
            "    OT[{i:>3}]  0x{code:02x}  {}",
            section6_code_label(*code)
        );
    }
    if codes.len() > 40 {
        println!("  ... ({} more codes)", codes.len() - 40);
    }
    println!();
}

fn print_section6_tail(vdf: &VdfFile) {
    println!("=== Section 6 Tail ===");

    match vdf.section6_ot_final_values() {
        Some(values) if !values.is_empty() => {
            println!("  OT final values: {}", values.len());
            for (ot, value) in values.iter().take(16).enumerate() {
                println!("    OT[{ot:>3}] final={value}");
            }
            if values.len() > 16 {
                println!("    ... ({} more final values)", values.len() - 16);
            }
        }
        _ => println!("  OT final values: (none)"),
    }

    match vdf.section6_lookup_records() {
        Some(records) if !records.is_empty() => {
            println!("  lookup records: {}", records.len());
            for (i, rec) in records.iter().take(16).enumerate() {
                let w = &rec.words;
                println!(
                    "    {:>3} @0x{:08x} ot={} words=[{:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}]",
                    i,
                    rec.file_offset,
                    rec.ot_index(),
                    w[0],
                    w[1],
                    w[2],
                    w[3],
                    w[4],
                    w[5],
                    w[6],
                    w[7],
                    w[8],
                    w[9],
                    w[10],
                    w[11],
                    w[12],
                );
            }
            if records.len() > 16 {
                println!("    ... ({} more lookup records)", records.len() - 16);
            }
        }
        Some(_) => println!("  lookup records: 0"),
        None => println!("  lookup records: (unparsed)"),
    }

    println!();
}

fn print_offset_table(vdf: &VdfFile) {
    println!(
        "=== Offset Table ({} entries @ 0x{:08x}) ===",
        vdf.offset_table_count, vdf.offset_table_start
    );
    let codes = vdf.section6_ot_class_codes();

    for i in 0..vdf.offset_table_count {
        if let Some(raw) = vdf.offset_table_entry(i) {
            let code_suffix = codes
                .as_ref()
                .and_then(|codes| codes.get(i))
                .map(|code| format!("  code=0x{code:02x} ({})", section6_code_label(*code)))
                .unwrap_or_default();
            if vdf.is_data_block_offset(raw) {
                println!("  {:>3}  0x{:08x}  block{}", i, raw, code_suffix);
            } else {
                let f = f32::from_le_bytes(raw.to_le_bytes());
                println!("  {:>3}  0x{:08x}  const = {}{}", i, raw, f, code_suffix);
            }
        }
    }
    println!();
}

fn print_data_blocks(vdf: &VdfFile) {
    // Collect unique block offsets from the offset table (more reliable
    // than enumerate_data_blocks which assumes contiguous packing).
    let mut block_offsets: Vec<usize> = (0..vdf.offset_table_count)
        .filter_map(|i| {
            let raw = vdf.offset_table_entry(i)?;
            if vdf.is_data_block_offset(raw) {
                Some(raw as usize)
            } else {
                None
            }
        })
        .collect();
    block_offsets.sort();
    block_offsets.dedup();

    println!("=== Data Blocks ({}) ===", block_offsets.len());

    for (idx, &offset) in block_offsets.iter().enumerate() {
        if offset + 2 + vdf.bitmap_size > vdf.data.len() {
            println!("  {:>3}  0x{:08x}  (truncated)", idx, offset);
            continue;
        }
        let count = read_u16(&vdf.data, offset) as usize;
        let block_size = 2 + vdf.bitmap_size + count * 4;
        let density = if vdf.time_point_count > 0 {
            (count as f64 / vdf.time_point_count as f64) * 100.0
        } else {
            0.0
        };

        let data_start = offset + 2 + vdf.bitmap_size;
        let first_val = if count > 0 && data_start + 4 <= vdf.data.len() {
            format!("{}", read_f32(&vdf.data, data_start))
        } else {
            "?".to_string()
        };
        let last_val = if count > 1 && data_start + count * 4 <= vdf.data.len() {
            format!("{}", read_f32(&vdf.data, data_start + (count - 1) * 4))
        } else {
            first_val.clone()
        };

        let label = if offset == vdf.first_data_block {
            "  [TIME]"
        } else {
            ""
        };
        println!(
            "  {:>3}  0x{:08x}  {}/{} ({:.0}%)  {}B  first={} last={}{}",
            idx,
            offset,
            count,
            vdf.time_point_count,
            density,
            block_size,
            first_val,
            last_val,
            label
        );
    }
    println!();
}

/// Build an ordered list of (start_offset, end_offset, label) for every known
/// file structure, then check for non-zero data in any gap between adjacent
/// structures (e.g. between the file header and the first section, or after
/// the last section's region).
fn print_gaps(vdf: &VdfFile, file_size: usize) {
    let mut regions: Vec<(usize, usize, String)> = Vec::new();

    // File header
    regions.push((0, FILE_HEADER_SIZE, "file header".to_string()));

    // Each section: full region (magic-to-magic)
    for (i, sec) in vdf.sections.iter().enumerate() {
        let role = SECTION_ROLES.get(i).copied().unwrap_or("unknown");
        regions.push((
            sec.file_offset,
            sec.region_end,
            format!("section {} ({})", i, role),
        ));
    }

    // Records
    if !vdf.records.is_empty() {
        let rec_start = vdf.records.first().unwrap().file_offset;
        let rec_end = vdf.records.last().unwrap().file_offset + RECORD_SIZE;
        regions.push((rec_start, rec_end, "records".to_string()));
    }

    // Slot table
    if vdf.slot_table_offset > 0 && !vdf.slot_table.is_empty() {
        let slot_end = vdf.slot_table_offset + vdf.slot_table.len() * 4;
        regions.push((vdf.slot_table_offset, slot_end, "slot table".to_string()));
    }

    // Offset table
    if vdf.offset_table_count > 0 {
        let ot_end = vdf.offset_table_start + vdf.offset_table_count * 4;
        regions.push((vdf.offset_table_start, ot_end, "offset table".to_string()));
    }

    // Data blocks (from first to end of last)
    if vdf.first_data_block > 0 {
        let blocks = simlin_engine::vdf::enumerate_data_blocks(
            &vdf.data,
            vdf.first_data_block,
            vdf.bitmap_size,
            vdf.time_point_count,
        );
        if let Some(last) = blocks.last() {
            let blocks_end = last.0 + last.2;
            regions.push((vdf.first_data_block, blocks_end, "data blocks".to_string()));
        }
    }

    regions.sort_by_key(|&(start, end, _)| (start, end));

    // Merge overlapping regions (e.g. records and slot tables are sub-structures
    // within a section's region)
    let mut merged: Vec<(usize, usize, String)> = Vec::new();
    for (start, end, label) in regions {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            // Overlapping or adjacent -- extend the merged region
            if end > last.1 {
                last.1 = end;
                last.2 = format!("{} + {}", last.2, label);
            }
            continue;
        }
        merged.push((start, end, label));
    }

    // Check gaps between adjacent merged regions
    let mut gap_count = 0;
    let mut total_gap_bytes = 0;

    for pair in merged.windows(2) {
        let gap_start = pair[0].1;
        let gap_end = pair[1].0;
        if gap_start >= gap_end {
            continue;
        }

        let gap_data = &vdf.data[gap_start..gap_end];
        let non_zero_count = gap_data.iter().filter(|&&b| b != 0).count();
        total_gap_bytes += gap_end - gap_start;

        if non_zero_count > 0 {
            if gap_count == 0 {
                println!("=== Non-Zero Gaps Between Structures ===");
            }
            gap_count += 1;

            println!(
                "\n  Gap: 0x{:08x}..0x{:08x} ({} bytes, {} non-zero)",
                gap_start,
                gap_end,
                gap_end - gap_start,
                non_zero_count,
            );
            println!("  Between: \"{}\" and \"{}\"", pair[0].2, pair[1].2);
            hexdump(gap_data, gap_start, MAX_HEXDUMP);
        }
    }

    // Also check after the last known structure to end of file
    if let Some(last) = merged.last()
        && last.1 < file_size
    {
        let trailing = &vdf.data[last.1..file_size];
        let non_zero_count = trailing.iter().filter(|&&b| b != 0).count();
        total_gap_bytes += file_size - last.1;
        if non_zero_count > 0 {
            if gap_count == 0 {
                println!("=== Non-Zero Gaps Between Structures ===");
            }
            gap_count += 1;
            println!(
                "\n  Trailing data: 0x{:08x}..0x{:08x} ({} bytes, {} non-zero)",
                last.1,
                file_size,
                file_size - last.1,
                non_zero_count,
            );
            println!("  After: \"{}\"", last.2);
            hexdump(trailing, last.1, MAX_HEXDUMP);
        }
    }

    if gap_count == 0 {
        println!("=== Gaps Between Structures ===");
        println!(
            "  No non-zero gaps found ({} gap bytes, all zeros)",
            total_gap_bytes
        );
    } else {
        println!(
            "\n  Total: {} gaps with non-zero data out of {} total gap bytes",
            gap_count, total_gap_bytes
        );
    }
    println!();
}

fn print_summary(vdf: &VdfFile, file_size: usize) {
    let n_block_ot = (0..vdf.offset_table_count)
        .filter(|&i| {
            vdf.offset_table_entry(i)
                .is_some_and(|r| vdf.is_data_block_offset(r))
        })
        .count();
    let n_const_ot = vdf.offset_table_count - n_block_ot;

    let sys_set: HashSet<&str> = SYSTEM_NAMES.iter().copied().collect();
    let n_system = vdf
        .names
        .iter()
        .filter(|n| sys_set.contains(n.as_str()))
        .count();
    let n_groups = vdf.names.iter().filter(|n| n.starts_with('.')).count();
    let n_units = vdf.names.iter().filter(|n| n.starts_with('-')).count();
    let n_builtins = vdf
        .names
        .iter()
        .filter(|n| {
            let name = n.as_str();
            !sys_set.contains(name)
                && !name.starts_with('.')
                && !name.starts_with('-')
                && (VENSIM_BUILTINS
                    .iter()
                    .any(|&b| b.eq_ignore_ascii_case(name))
                    || (name.len() == 1 && name.starts_with(|c: char| !c.is_alphanumeric())))
        })
        .count();
    let n_model_names = vdf.names.len() - n_system - n_groups - n_units - n_builtins;

    let ot_count = vdf.offset_table_count;
    let n_model_recs = vdf
        .records
        .iter()
        .filter(|r| {
            r.fields[0] != 0
                && r.fields[1] != 23
                && r.fields[10] > 0
                && r.fields[11] > 0
                && (r.fields[11] as usize) < ot_count
        })
        .count();

    // Count unique f[12] groups
    let slot_groups: HashSet<u32> = vdf.records.iter().map(|r| r.fields[12]).collect();

    println!("=== Summary ===");
    println!("  File size:      {} bytes", file_size);
    println!("  Sections:       {}", vdf.sections.len());
    println!(
        "  Names:          {} ({} system, {} groups, {} units, {} builtins, {} model vars)",
        vdf.names.len(),
        n_system,
        n_groups,
        n_units,
        n_builtins,
        n_model_names
    );
    let unslotted_count = vdf.names.len().saturating_sub(vdf.slot_table.len());
    if unslotted_count > 0 {
        println!(
            "  Unslotted names: {} (no slot table entry)",
            unslotted_count
        );
    }
    println!(
        "  Records:        {} ({} model var, {} f[12] groups)",
        vdf.records.len(),
        n_model_recs,
        slot_groups.len()
    );
    println!(
        "  OT entries:     {} ({} blocks, {} constants)",
        vdf.offset_table_count, n_block_ot, n_const_ot
    );
    println!("  Data blocks:    {}", n_block_ot);
}
