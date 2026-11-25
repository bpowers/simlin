// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Stored projects go on reading as they did.
//!
//! The web app's database holds serialized projects, which no test can reach;
//! the files listed in [`STORED`] stand in for them. Each is bytes a writer of
//! some schema produced, checked in beside the `Debug` form of the project
//! those bytes read to (`test/protobuf-compat/<name>.txt`). Two rules of the
//! schema are held to them:
//!
//! - A field's number and type never change. Every field the schema defines
//!   is on the wire of at least one stored project
//!   ([`every_field_of_the_schema_is_carried_by_a_stored_project`]), so a
//!   renumbered or retyped field reads those bytes as a different project and
//!   fails its golden. A field added to the schema therefore comes with a new
//!   stored project that carries it: serialize any project holding the field
//!   (`serde::serialize(&project)?.encode_to_vec()`), check the bytes in, and
//!   list them here.
//! - A field a stored project lacks reads as the project read without it: the
//!   older files carry none of the fields added since, and their goldens were
//!   written by the reader of their day.
//!
//! The bytes are never rewritten: bytes a current writer produces pin nothing
//! about reading the old ones. [`STORED`] holds each file's SHA-256, so a
//! rewrite fails here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use buffa::Message;
use sha2::{Digest, Sha256};

use super::deserialize;
use crate::project_io;

/// A stored project: its name (the golden is `protobuf-compat/<name>.txt`),
/// its bytes' path relative to `test/`, and the bytes' SHA-256.
struct Stored {
    name: &'static str,
    bytes: &'static str,
    sha256: &'static str,
}

const STORED: [Stored; 12] = [
    // One of everything the schema defines, written by the schema that has
    // `Model.sim_specs`, `View.sketch_compat` and the sketch fields of
    // `ViewElementCompat`.
    Stored {
        name: "every-field-2",
        bytes: "protobuf-compat/every-field-2.pb",
        sha256: "773315929a6cf9902361bd8f247e183108e2313b18a01ce3410ae90c526d8ca8",
    },
    // Written by the schema before that one.
    Stored {
        name: "every-field",
        bytes: "protobuf-compat/every-field.pb",
        sha256: "46f5c1b39f76df0b11ae09a07dd7cb18c4d6314cc15576e4f44f740aa1edb6e9",
    },
    Stored {
        name: "except2",
        bytes: "protobuf-compat/except2.pb",
        sha256: "3f213e9207d827d83db585848da8830f97449306f847795a183c15524cdc3a19",
    },
    Stored {
        name: "mapping",
        bytes: "protobuf-compat/mapping.pb",
        sha256: "9b5aa0ab0fa5b94759fc33b3c2a9612769a665df71a069054756e7ac9ea67bef",
    },
    Stored {
        name: "leaky-conveyor",
        bytes: "protobuf-compat/leaky-conveyor.pb",
        sha256: "1754bb15f88af6cae837978670a9df5dca370ef978e1b5ea259e4002b914abcd",
    },
    Stored {
        name: "queue-coupled-conveyor",
        bytes: "protobuf-compat/queue-coupled-conveyor.pb",
        sha256: "9114eccd7970a995f58b805e5751d8f38881e368c8938c39d4ab33d8b8c5635c",
    },
    Stored {
        name: "modules",
        bytes: "protobuf-compat/modules.pb",
        sha256: "e1870cd688330d514ad8146a34fe73b8f9b960ff31159b7ad2e0a78279bbfd1f",
    },
    // The fields the reader reads and no writer writes: an
    // equation's own `initial_equation` and a dimension's `obsolete_elements`.
    // Encoded from the schema's own messages, since no writer
    // produces them.
    Stored {
        name: "legacy-fields",
        bytes: "protobuf-compat/legacy-fields.pb",
        sha256: "c92a4a7953e2825465c304a39e7649df6ee436367686fe05ac90ef151cf43bd7",
    },
    // Written by schemas older still.
    Stored {
        name: "fishbanks",
        bytes: "fishbanks.protobin",
        sha256: "2e97aa887c0361fb2d3925946e4e57e6600963b07a50c3e23360e46e59ffa37d",
    },
    Stored {
        name: "logistic-growth",
        bytes: "logistic-growth.protobin",
        sha256: "f3356e9eff6885ca83b68a080a99bf31a2d702589247968b206084bc17c61eaa",
    },
    Stored {
        name: "sir",
        bytes: "../src/libsimlin/testdata/SIR_project.pb",
        sha256: "15a86b9247a4ea66927d87e90f8830b62b468748c08857d5fef26b4b00dc9304",
    },
    Stored {
        name: "layoutaux2-20",
        bytes: "../src/libsimlin/testdata/layoutaux2-20.pb",
        sha256: "d60dfb2428ed95bd16ec36221de5ef50d4b7120016d92047309a88bb61f38b69",
    },
];

fn test_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test")
}

fn bytes_of(stored: &Stored) -> Vec<u8> {
    std::fs::read(test_dir().join(stored.bytes)).expect("the stored project is checked in")
}

/// A field the datamodel gains changes the `Debug` form without changing what
/// is read; `UPDATE_PROTOBUF_COMPAT=1` rewrites the `.txt` files, and the diff
/// to review is the new field at its "none" and nothing else.
#[test]
fn a_stored_project_reads_as_it_did() {
    for stored in &STORED {
        let project = project_io::Project::decode_from_slice(&bytes_of(stored))
            .expect("the stored bytes decode");
        let read = format!(
            "{:#?}\n",
            deserialize(project).expect("the stored project reads")
        );
        let golden = test_dir().join(format!("protobuf-compat/{}.txt", stored.name));
        if std::env::var_os("UPDATE_PROTOBUF_COMPAT").is_some() {
            std::fs::write(&golden, &read).expect("the expected reading is writable");
            continue;
        }
        let expected =
            std::fs::read_to_string(&golden).expect("the expected reading is checked in");
        assert!(
            read == expected,
            "{} reads differently than test/protobuf-compat/{}.txt records",
            stored.bytes,
            stored.name
        );
    }
}

#[test]
fn the_stored_projects_are_the_bytes_that_were_checked_in() {
    for stored in &STORED {
        let digest = format!("{:x}", Sha256::digest(bytes_of(stored)));
        assert_eq!(
            digest, stored.sha256,
            "{} is not the file that was checked in: a stored project is never rewritten",
            stored.bytes
        );
    }
}

// -- The schema, read from `project_io.proto`

/// A field of a message: its number, and its type's full name when the type
/// is a message of the schema (what the wire walk descends into).
struct Field {
    name: String,
    number: u32,
    message: Option<String>,
}

/// Every message of `project_io.proto` by its full name (`Variable.Stock`),
/// with its fields, a `oneof`'s among them.
///
/// The file is read as the subset of the proto3 language it is written in:
/// `message`, `enum` and `oneof` blocks, and fields of the form
/// `[repeated|optional] Type name = N;`. Anything else fails the parse, so a
/// construct this does not read cannot go unnoticed.
fn schema() -> BTreeMap<String, Vec<Field>> {
    enum Block {
        Message(String),
        Enum,
        Oneof,
    }
    let text = include_str!("project_io.proto");
    let code: String = text
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let spaced = code
        .replace('{', " { ")
        .replace('}', " } ")
        .replace(';', " ; ")
        .replace('=', " = ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();

    // First the names, so a field's type can be told from a scalar's.
    let mut declared: BTreeSet<String> = BTreeSet::new();
    // (message, type as written, name, number)
    let mut fields: Vec<(String, String, String, u32)> = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    let scope = |blocks: &[Block]| -> String {
        blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                Block::Message(name) => Some(name.clone()),
                Block::Enum | Block::Oneof => None,
            })
            .unwrap_or_default()
    };
    let mut at = 0;
    while at < tokens.len() {
        match tokens[at..] {
            ["syntax", "=", _, ";", ..] => at += 4,
            ["package", _, ";", ..] => at += 3,
            [";", ..] => at += 1,
            ["}", ..] => {
                blocks.pop().expect("a block to close");
                at += 1;
            }
            ["message", name, "{", ..] => {
                let outer = scope(&blocks);
                let full = if outer.is_empty() {
                    name.to_string()
                } else {
                    format!("{outer}.{name}")
                };
                declared.insert(full.clone());
                blocks.push(Block::Message(full));
                at += 3;
            }
            ["enum", _, "{", ..] => {
                blocks.push(Block::Enum);
                at += 3;
            }
            ["oneof", _, "{", ..] => {
                blocks.push(Block::Oneof);
                at += 3;
            }
            [_, "=", _, ";", ..] if matches!(blocks.last(), Some(Block::Enum)) => at += 4,
            ["repeated" | "optional", ty, name, "=", number, ";", ..] => {
                fields.push((
                    scope(&blocks),
                    ty.to_string(),
                    name.to_string(),
                    number.parse().expect("a field number"),
                ));
                at += 6;
            }
            [ty, name, "=", number, ";", ..] => {
                fields.push((
                    scope(&blocks),
                    ty.to_string(),
                    name.to_string(),
                    number.parse().expect("a field number"),
                ));
                at += 5;
            }
            _ => panic!(
                "project_io.proto holds something the test does not read at {:?}",
                &tokens[at..tokens.len().min(at + 6)]
            ),
        }
    }
    assert!(blocks.is_empty(), "project_io.proto closes every block");

    let mut messages: BTreeMap<String, Vec<Field>> = declared
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();
    for (message, ty, name, number) in fields {
        // A type name is looked up from the innermost scope outward.
        let mut scope: Vec<&str> = message.split('.').collect();
        let resolved = loop {
            let candidate = scope
                .iter()
                .copied()
                .chain([ty.as_str()])
                .collect::<Vec<_>>()
                .join(".");
            if declared.contains(&candidate) {
                break Some(candidate);
            }
            if scope.pop().is_none() {
                break None;
            }
        };
        messages
            .get_mut(&message)
            .expect("a field sits in a declared message")
            .push(Field {
                name,
                number,
                message: resolved,
            });
    }
    messages
}

// -- The wire

/// A base-128 varint at `at`, and where it ends.
fn varint(bytes: &[u8], mut at: usize) -> (u64, usize) {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = bytes[at];
        at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (value, at);
        }
        shift += 7;
    }
}

/// Records every `(message, field number)` on the wire of `bytes`, read as a
/// `message` of `schema`, descending into the fields whose type is a message.
fn walk(
    schema: &BTreeMap<String, Vec<Field>>,
    message: &str,
    bytes: &[u8],
    carried: &mut BTreeSet<(String, u32)>,
) {
    let mut at = 0;
    while at < bytes.len() {
        let (key, next) = varint(bytes, at);
        at = next;
        let number = (key >> 3) as u32;
        carried.insert((message.to_string(), number));
        match key & 7 {
            0 => at = varint(bytes, at).1,
            1 => at += 8,
            5 => at += 4,
            2 => {
                let (len, next) = varint(bytes, at);
                let payload = &bytes[next..next + len as usize];
                at = next + len as usize;
                let nested = schema[message]
                    .iter()
                    .find(|field| field.number == number)
                    .and_then(|field| field.message.as_deref());
                if let Some(nested) = nested {
                    walk(schema, nested, payload, carried);
                }
            }
            other => panic!("wire type {other} in a stored project"),
        }
    }
}

/// Every `(message, field number)` the schema defines within a `Project`.
fn fields_of_a_project(schema: &BTreeMap<String, Vec<Field>>) -> BTreeSet<(String, u32)> {
    let mut reached: BTreeSet<&str> = BTreeSet::new();
    let mut pending = vec!["Project"];
    while let Some(message) = pending.pop() {
        if reached.insert(message) {
            pending.extend(
                schema[message]
                    .iter()
                    .filter_map(|field| field.message.as_deref()),
            );
        }
    }
    reached
        .into_iter()
        .flat_map(|message| {
            schema[message]
                .iter()
                .map(move |field| (message.to_string(), field.number))
        })
        .collect()
}

/// Fields of the schema no stored project carries, each with why: nothing
/// writes them. A row here whose field is carried fails the test below, so
/// the list holds only what it must.
const CARRIED_BY_NOTHING: [(&str, &str, &str); 1] = [(
    "View",
    "kind",
    "STOCK_FLOW, the one view type, is the proto3 default, which is never on the wire",
)];

#[test]
fn every_field_of_the_schema_is_carried_by_a_stored_project() {
    let schema = schema();
    let mut carried: BTreeSet<(String, u32)> = BTreeSet::new();
    for stored in &STORED {
        walk(&schema, "Project", &bytes_of(stored), &mut carried);
    }
    let name_of = |(message, number): &(String, u32)| -> String {
        let field = schema[message]
            .iter()
            .find(|field| field.number == *number)
            .expect("a field of the schema");
        format!("{message}.{}", field.name)
    };
    let excused: BTreeSet<String> = CARRIED_BY_NOTHING
        .iter()
        .map(|(message, field, _why)| format!("{message}.{field}"))
        .collect();
    let defined = fields_of_a_project(&schema);
    let missing: Vec<String> = defined
        .iter()
        .filter(|field| !carried.contains(*field))
        .map(name_of)
        .filter(|name| !excused.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "no stored project carries {missing:#?}: add a stored project that does (see the \
         module's rustdoc)"
    );
    let stale: Vec<String> = defined
        .iter()
        .filter(|field| carried.contains(*field))
        .map(name_of)
        .filter(|name| excused.contains(name))
        .collect();
    assert!(stale.is_empty(), "carried after all: {stale:#?}");
}
