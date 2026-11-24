# Copyright 2026 The Simlin Authors. All rights reserved.
# Use of this source code is governed by the Apache License,
# Version 2.0, that can be found in the LICENSE file.

"""Tests of check-engine-layers.py over small crates written to a directory."""

from __future__ import annotations

import copy
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-engine-layers.py"
spec = importlib.util.spec_from_file_location("check_engine_layers", SCRIPT)
assert spec is not None and spec.loader is not None
layers = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = layers
spec.loader.exec_module(layers)

TABLE = {
    "source": "src",
    "test_features": ["test-support"],
    "layers": {
        "low": {"uses": [], "modules": ["low"]},
        "high": {"uses": ["low"], "modules": ["high"]},
    },
}

LIB = """
mod low;
pub mod high;
pub use other_crate;
pub use self::high::Shown;
pub use crate::high::{inner::Named, Before as After};
#[cfg(test)]
mod root_tests;
"""

HIGH = """
#[macro_export]
macro_rules! shout { () => {} }
pub fn g() {}
"""

# A reference from `low` to `high`, in every form one is written in.
REFERENCES = [
    "fn f() { crate::high::g(); }",
    "use crate::high;",
    "use crate::high as h;",
    "use crate::high::*;",
    "use crate::{low::x, high::{g, inner::{a, b as c}}};",
    "use super::high::g;",
    "use super::{low::x, high::g};",
    "mod inner { use super::super::high::g; }",
    "mod inner { fn f() {} } use super::high::g;",
    "mod inner { mod deeper { use crate::high::g; } }",
    "pub(crate) use crate::high::g;",
    "fn f(x: Vec<crate::high::T>) {}",
    "fn f() { low::<crate::high::T>(); }",
    "impl crate::high::T for S {}",
    "macro_rules! m { () => { $crate::high::g() } }",
    "#[derive(crate::high::D)] struct S;",
    '#[serde(with = "crate::high::codec")] struct S;',
    "use crate::Shown;",
    "use crate::Named;",
    "use crate::After;",
    "fn f() { shout!(); }",
    "fn f() { crate::shout!(); }",
    # A quote in a character, a string or a lifetime does not open a string.
    "fn f() { let a = '\"'; crate::high::g(); let b = '\"'; }",
    "fn f() { let a = '\\''; crate::high::g(); let b = \"\\\"\"; }",
    "fn f<'a>(x: &'a str) { crate::high::g(); let c = 'a'; }",
    'fn f() { let a = r#"""#; crate::high::g(); let b = r"\\"; }',
    "fn f() { 1..2; 1.5; crate::high::g(); }",
    # Compiled without tests, whatever else it depends on.
    "#[cfg_attr(test, derive(Debug))] struct S(crate::high::T);",
]

NO_REFERENCES = [
    "// crate::high::g",
    "/// [`crate::high::g`]\nfn f() {}",
    "/* crate::high /* crate::high */ crate::high */ fn f() {}",
    'fn f() { let a = "crate::high::g"; }',
    'fn f() { let a = "\\"crate::high::g"; }',
    'fn f() { let a = r#"" crate::high::g"#; }',
    "use crate::low::x; fn f() { self::x(); super::low::x(); }",
    "use crate::other_crate::x;",
    "fn f(high: u8) { high::g(); vec![high]; }",
    "pub(crate) fn f(&self) { self.g() }",
]

# Whether a build without tests never compiles what is under the predicate.
CFG_PREDICATES = [
    ("test", True),
    ('feature = "test-support"', True),
    ('any(test, feature = "test-support")', True),
    ('all(test, feature = "file_io")', True),
    ("all(unix, any(test))", True),
    ("not(not(test))", True),
    ("not(test)", False),
    ("unix", False),
    ('feature = "file_io"', False),
    ("any(test, unix)", False),
    ("all(not(test), unix)", False),
    ("not(any(test, unix))", False),
    ("all()", False),
]

# What a test-only attribute can be written on, by where it is written: the
# text around it, and the forms. Each refers to `high`.
TEST_ONLY = {
    "{}": [
        "mod tests { use crate::high::g; fn f() { {} } }",
        "mod tests;",
        "fn f<A, B>(a: A) -> B where A: Into<B>, B: Copy { crate::high::g() }",
        "pub(crate) fn f() { crate::high::g() }",
        "pub const fn f() { crate::high::g() }",
        "async fn f() { crate::high::g() }",
        "unsafe fn f() { crate::high::g() }",
        'extern "C" { fn f(x: crate::high::A); }',
        "const X: [u8; 2] = { crate::high::X };",
        "static X: u8 = crate::high::X;",
        "use crate::high::{a, b};",
        "pub use crate::high::a;",
        "type T = Result<crate::high::A, crate::high::B>;",
        "struct S { a: crate::high::A, b: u8 }",
        "struct S<A, B>(crate::high::A, u8) where A: Copy, B: Copy;",
        "enum E { A(crate::high::A), B }",
        "union U { a: crate::high::A, b: u8 }",
        "trait T: crate::high::U { fn f(); }",
        "impl<A, B> crate::high::T for S<A, B> where A: Copy, B: Copy { fn f() {} }",
        "macro_rules! m { () => { $crate::high::g() } }",
        "macro_rules! m( () => ( $crate::high::g() ) );",
        "thread_local! { static X: crate::high::A = 1; }",
        "declare!(crate::high::A, 1);",
    ],
    "fn around() {{ {} }}": [
        "let x = if a { crate::high::a() } else { crate::high::b() };",
        "let f = |a, b| crate::high::g(a, b);",
        "if a < b { crate::high::g() } else if c { crate::high::h() } else { crate::high::i() }",
        "for x in crate::high::all() { x(); }",
        "while a { crate::high::g() }",
        "loop { crate::high::g() }",
        "match x { crate::high::A => 1, _ => 2 }",
        "unsafe { crate::high::g() }",
        "{ crate::high::g() }",
        "crate::high::g(1, 2);",
    ],
    "struct Around {{ {} }}": [
        "a: HashMap<crate::high::A, HashMap<u8, crate::high::B>>,",
        "pub(crate) a: crate::high::A,",
        "f: fn(u8) -> crate::high::A,",
    ],
    "enum Around {{ {} }}": ["A(crate::high::A, u8),", "A { a: crate::high::A, b: u8 },"],
    "fn around() {{ match x {{ {} _ => {{}} }} }}": [
        "a if a < b => { crate::high::g() }",
        "a if a < b => crate::high::g(a, b),",
        "A(a, b) => crate::high::g(a, b),",
        "A => { crate::high::g() },",
    ],
}

# The same places with what a build without tests compiles in them.
PRODUCTION = {
    "{}": "fn after() { crate::high::after(); }",
    "fn around() {{ {} }}": "crate::high::after();",
    "struct Around {{ {} }}": "after: crate::high::After,",
    "enum Around {{ {} }}": "After(crate::high::After),",
    "fn around() {{ match x {{ {} _ => {{}} }} }}": "After => crate::high::after(),",
}


def check(files: dict[str, str | None], table: dict = TABLE) -> list[str]:
    """What the check says of a crate of `files`, beside the usual ones (None
    leaves a usual one out)."""
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for name, text in {"lib.rs": LIB, "low.rs": "", "high.rs": HIGH, **files}.items():
            if text is not None:
                path = root / "src" / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8")
        return layers.check(root, table)


class ReferenceTests(unittest.TestCase):
    def test_every_form_of_a_reference_is_one(self) -> None:
        for source in REFERENCES:
            with self.subTest(source):
                found = check({"low.rs": source})
                self.assertTrue(found)
                for error in found:
                    self.assertIn("src/low.rs:1: low must not use high", error)

    def test_what_only_looks_like_a_reference_is_none(self) -> None:
        for source in NO_REFERENCES:
            with self.subTest(source):
                self.assertEqual(check({"low.rs": source}), [])

    def test_a_layer_may_use_itself_and_the_layers_it_names(self) -> None:
        self.assertEqual(check({"high.rs": "use crate::low::x; use crate::high::y;"}), [])

    def test_a_reference_is_reported_at_its_line(self) -> None:
        [error] = check({"low.rs": "/* a\n b */\nfn f() {\n    crate::high::g();\n}\n"})
        self.assertIn("src/low.rs:4:", error)

    def test_a_name_the_crate_root_does_not_say_the_module_of_is_an_error(self) -> None:
        for source in ("use crate::*;", "use super::*;", "use crate::Missing;", "use crate::{self as root};"):
            with self.subTest(source):
                [error] = check({"low.rs": source})
                self.assertIn("names no module of the crate root", error)
        [error] = check({"lib.rs": "mod low; mod high; pub use self::high::*;"})
        self.assertIn("a glob re-export hides", error)

    def test_a_path_above_the_crate_root_is_an_error(self) -> None:
        with self.assertRaisesRegex(layers.LayerError, "above the crate root"):
            check({"low.rs": "use super::super::high;"})


class TestCodeTests(unittest.TestCase):
    def test_a_predicate_is_test_only_when_no_build_without_tests_meets_it(self) -> None:
        for predicate, test_only in CFG_PREDICATES:
            with self.subTest(predicate):
                found = check({"low.rs": f"#[cfg({predicate})]\nuse crate::high::g;"})
                self.assertEqual(found == [], test_only)
        self.assertEqual(check({"low.rs": "#[test]\nfn f() { crate::high::g(); }"}), [])

    def test_a_test_only_item_is_skipped_and_nothing_after_it_is(self) -> None:
        for around, forms in TEST_ONLY.items():
            for form in forms:
                with self.subTest(form):
                    production = PRODUCTION[around]
                    for attributes in ("#[cfg(test)] #[allow(unused)]", "#[allow(unused)] #[cfg(test)]"):
                        item = f"{attributes} {form}"
                        self.assertEqual(check({"low.rs": around.format(item)}), [])
                        [error] = check({"low.rs": around.format(f"{item} {production}")})
                        self.assertIn("after", error.lower())

    def test_a_test_only_field_can_end_at_its_structs_brace(self) -> None:
        for around in ("struct S {{ {} }}", "enum E {{ {} }}", "fn f() {{ match x {{ {} }} }}"):
            with self.subTest(around):
                item = around.format("#[cfg(test)] A(crate::high::A)")
                [error] = check({"low.rs": f"{item} fn after() {{ crate::high::after(); }}"})
                self.assertIn("crate::high::after", error)

    def test_every_word_an_item_can_start_with_is_covered(self) -> None:
        firsts = set()
        for form in (form for forms in TEST_ONLY.values() for form in forms):
            words = [token.text for token in layers.tokenize(form)]
            if words[0] == "pub":
                words = words[4:] if words[1] == "(" else words[1:]
            firsts.add(words[0])
        self.assertLessEqual(layers.BLOCK_ITEMS | layers.SEMICOLON_ITEMS | {"const"}, firsts)

    def test_a_file_mounted_under_a_test_only_item_is_not_read(self) -> None:
        upward = "use crate::high::g;"
        for mount in ("#[cfg(test)] mod t;", '#[cfg(test)] #[path = "low_t.rs"] mod t;', "#[cfg(test)] mod t { mod u; }"):
            with self.subTest(mount):
                files = {"low.rs": mount, "low/t.rs": upward, "low_t.rs": upward, "low/t/u.rs": upward}
                self.assertEqual(check(files), [])
        self.assertEqual(check({"low.rs": "mod t;", "low/t.rs": f"#![cfg(test)] {upward}"}), [])


class ModuleTreeTests(unittest.TestCase):
    def test_every_production_file_is_read(self) -> None:
        upward = "use crate::high::g;"
        trees = [
            {"low.rs": "mod a;", "low/a.rs": upward},
            {"low.rs": "mod a;", "low/a/mod.rs": upward},
            {"low.rs": None, "low/mod.rs": "mod a;", "low/a.rs": upward},
            {"low.rs": "mod a;", "low/a.rs": "mod b;", "low/a/b.rs": "use super::super::super::high;"},
            {"low.rs": '#[path = "elsewhere.rs"] mod a;', "elsewhere.rs": "use super::super::high;"},
            {"low.rs": "mod a { mod b; }", "low/a/b.rs": upward},
            {"low.rs": 'mod a { #[path = "c.rs"] mod b; }', "low/a/c.rs": upward},
            {"low.rs": "mod a;", "low/a.rs": "#[cfg(unix)] mod b;", "low/a/b.rs": upward},
        ]
        for files in trees:
            with self.subTest(files):
                [error] = check(files)
                self.assertIn("low must not use high", error)

    def test_a_module_with_no_file_is_an_error(self) -> None:
        with self.assertRaises(layers.LayerError):
            check({"low.rs": "mod a;"})


class TableTests(unittest.TestCase):
    def table(self, **changes: dict[str, list[str]]) -> dict:
        table = copy.deepcopy(TABLE)
        for layer, change in changes.items():
            table["layers"].setdefault(layer, {"uses": [], "modules": []}).update(change)
        return table

    def test_the_table_and_the_crate_name_the_same_modules(self) -> None:
        [error] = check({}, self.table(high={"modules": []}))
        self.assertIn("module 'high' is in no layer", error)
        [error] = check({}, self.table(low={"modules": ["low", "gone"]}))
        self.assertIn("module 'gone' does not exist", error)
        [error] = check({}, self.table(high={"modules": ["high", "low"]}))
        self.assertIn("module 'low' is in two layers", error)

    def test_a_layer_uses_only_layers_listed_before_it(self) -> None:
        for uses in (["high"], ["low"], ["missing"]):
            with self.subTest(uses):
                [error] = check({}, self.table(low={"uses": uses}))
                self.assertIn("which is not a layer listed before it", error)


if __name__ == "__main__":
    unittest.main()
