// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Where does a generated link score differ from the one the model's own code
//! gives when it is run again with that one input changed?
//!
//! Usage:
//!   cargo run --release -p simlin-tests --features instruments --example ltm_replay -- model...
//!   cargo run --release ... --example ltm_replay -- --list model...   # every link that differs
//!
//! A model is an `.mdl`, an XMILE file, a systems-format `.txt`, or a directory
//! to look for them in.
//! Each prints how many scores agree and, for each way the rest differ, how
//! many do and the first of them (`simlin_engine::ltm_replay_compare::Why`
//! says what each means); a directory prints the models with something to say,
//! and a total.

use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use simlin_base::datamodel::Project;
use simlin_engine::ltm_replay_compare::{Comparison, OneSided, Why, compare};

const EXTENSIONS: [&str; 5] = ["mdl", "xmile", "stmx", "itmx", "txt"];

fn models_under(path: &Path, found: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            models_under(&entry.path(), found);
        }
    } else if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
    {
        found.push(path.to_path_buf());
    }
}

fn open(path: &Path) -> Option<Project> {
    let bytes = std::fs::read(path).ok()?;
    let text = || std::str::from_utf8(&bytes).ok();
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "mdl" => simlin_formats::open_vensim(text()?).ok(),
        "txt" => simlin_formats::open_systems(text()?).ok(),
        _ => simlin_formats::open_xmile(&mut BufReader::new(bytes.as_slice())).ok(),
    }
}

/// The lines that say what a comparison found beyond agreement.
fn lines(found: &Comparison, list: bool) -> Vec<String> {
    let most = if list { usize::MAX } else { 1 };
    let mut out = Vec::new();
    for (why, count) in found.counts() {
        out.push(format!("{count:>6}  {why:?}"));
        for d in found.differences.iter().filter(|d| d.why == why).take(most) {
            out.push(format!(
                "          {} -> {}: generated {:e}, replayed {:e} (row {})",
                d.from, d.to, d.generated, d.replayed, d.row
            ));
        }
    }
    let mut one_sided = |what: &str, links: &[OneSided]| {
        if links.is_empty() {
            return;
        }
        out.push(format!("{:>6}  {what}", links.len()));
        for link in links.iter().take(most) {
            out.push(format!("          {} -> {}", link.from, link.to));
        }
    };
    one_sided(
        "scored, and the target's code does not read it",
        &found.not_read,
    );
    one_sided("scored, and an end of it is no one slot", &found.not_slots);
    one_sided(
        "read by the target's code, and not scored",
        &found.not_scored,
    );
    if !found.misreproduced.is_empty() {
        out.push(format!(
            "{:>6}  slots whose code does not give back the run's value",
            found.misreproduced.len()
        ));
        for m in found.misreproduced.iter().take(most) {
            out.push(format!(
                "          {} at step {}: the run {:e}, replayed {:e}",
                m.name, m.step, m.ran, m.replayed
            ));
        }
    }
    out
}

fn main() {
    let (mut list, mut models) = (false, Vec::new());
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--list" => list = true,
            path => models_under(Path::new(path), &mut models),
        }
    }
    models.sort();
    let one = models.len() == 1;
    let (mut compared, mut agreed, mut active) = (0usize, 0usize, 0usize);
    let mut total: BTreeMap<Why, usize> = BTreeMap::new();
    let (mut not_read, mut not_slots, mut not_scored, mut misreproduced) = (0, 0, 0, 0);
    let mut refused: BTreeMap<String, usize> = BTreeMap::new();
    for path in &models {
        let Some(project) = open(path) else {
            *refused.entry("cannot be opened".to_string()).or_default() += 1;
            continue;
        };
        let found = match compare(&project) {
            Ok(found) => found,
            Err(why) => {
                if one {
                    println!("{}: {why}", path.display());
                }
                *refused.entry(why.to_string()).or_default() += 1;
                continue;
            }
        };
        compared += 1;
        agreed += found.agreed;
        active += found.agreed_active;
        for (why, count) in found.counts() {
            *total.entry(why).or_default() += count;
        }
        not_read += found.not_read.len();
        not_slots += found.not_slots.len();
        not_scored += found.not_scored.len();
        misreproduced += found.misreproduced.len();
        let lines = lines(&found, list);
        if one || !lines.is_empty() {
            println!(
                "{}: {} agree, {} of them active",
                path.display(),
                found.agreed,
                found.agreed_active
            );
            for line in lines {
                println!("{line}");
            }
        }
    }
    if !one {
        println!("\n{compared} models: {agreed} agree, {active} of them active");
        for (why, count) in total {
            println!("{count:>6}  {why:?}");
        }
        println!("{not_read:>6}  scored, not read");
        println!("{not_slots:>6}  scored, not slots");
        println!("{not_scored:>6}  read, not scored");
        println!("{misreproduced:>6}  slots not given back");
        println!("not compared: {}", refused.values().sum::<usize>());
        for (why, count) in refused {
            println!("{count:>6}  {}", why.chars().take(120).collect::<String>());
        }
    }
}
