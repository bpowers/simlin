// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Reading a tool's input: JSON text into the tool's input type, with a
//! mismatch reported where it is in the input.
//!
//! An agent builds its input as a value, not as text, so the line and column
//! a text parser reports say nothing to it, and "expected a number" alone does
//! not say which of an experiment's changes was wrong. [`parse`] reads the
//! text into a JSON value first, and deserializes the input type from that
//! value through a deserializer that knows where it is ([`At`]): the object
//! keys and array indexes on the way to the value being read. The first
//! nested read that fails is where the mismatch is, as `set[1].value`.
//!
//! A text parser's own position cannot serve: an internally tagged enum (an
//! edit's operation, a finding's citation) is read whole before it is
//! matched, so its mismatch is reported at whatever the parser read next,
//! the following element or the end of the list. The tracked read has the
//! same blind spot one level down: the variant's fields are read from the
//! buffered copy, not through [`At`], so a mismatch inside one is first
//! placed at the enum's object. [`parse`] then finds the field by
//! elimination ([`field_at_fault`]) and names it.
//!
//! The input is read as JSON is written, not as serde would accept it: an
//! object is the only shape of a struct (never a list of its fields in
//! order), a tag or other name is only a string (never a variant's index),
//! and a key appears once in an object.

use std::cell::RefCell;

use serde::de::value::{MapAccessDeserializer, StrDeserializer};
use serde::de::{
    DeserializeOwned, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, SeqAccess,
    Visitor,
};
use serde_json::Value;

/// The deepest an equation a tool is given may be, as [`equation_depth`]
/// measures it.
///
/// The parser, and every pass that reads a parsed equation, recurse on its
/// tree, so an equation deep enough overflows the stack of the thread the
/// call runs on, which aborts the process. Measured through `edit_model`
/// (the deepest path: parse, lowering, compile and run) on a 2 MiB thread,
/// Rust's default for a spawned thread and the smallest a host gives a call:
/// a debug build survives a chain of 133 operators (each a unit), 156 nested
/// parentheses (each two: 312 units), 133 nested `IF ... ELSE (` (three each)
/// and 33 nested function calls (each eight: 264); a release build survives
/// 1,552 units or more of every one of those shapes. The bound keeps a
/// quarter's margin under the debug build's least, so a test and a release
/// refuse alike, and an equation an agent writes rarely comes near it.
pub(crate) const MAX_EQUATION_DEPTH: usize = 100;

/// How deep the parsed tree of `text` can be, read from its tokens without
/// parsing it (the parser itself recurses): each operator, `IF` included, is
/// a level of a chain the tree may be (the operators of a sum nest one in
/// another); each open parenthesis or bracket is two more, and each open
/// call's parenthesis eight, a call's frames being the largest. The sum of
/// the operators and of the open brackets at the deepest point is an upper
/// bound, which [`MAX_EQUATION_DEPTH`] is measured in. A token the lexer
/// refuses is passed over: the parser reports it.
pub(crate) fn equation_depth(text: &str) -> usize {
    use crate::lexer::{Lexer, LexerType, Token};
    let mut operators = 0;
    let mut open: Vec<usize> = Vec::new();
    let (mut at, mut deepest) = (0usize, 0usize);
    let mut previous: Option<Token<'_>> = None;
    for token in Lexer::new(text, LexerType::Equation).flatten() {
        let (_, token, _) = token;
        match token {
            Token::If
            | Token::Not
            | Token::Mod
            | Token::Exp
            | Token::Eq
            | Token::Neq
            | Token::Lt
            | Token::Lte
            | Token::Gt
            | Token::Gte
            | Token::And
            | Token::Or
            | Token::Plus
            | Token::Minus
            | Token::Mul
            | Token::Div
            | Token::SafeDiv => operators += 1,
            Token::LParen | Token::LBracket => {
                let weight = match (token, previous) {
                    (Token::LParen, Some(Token::Ident(_))) => 8,
                    _ => 2,
                };
                open.push(weight);
                at += weight;
                deepest = deepest.max(at);
            }
            Token::RParen | Token::RBracket => at -= open.pop().unwrap_or(0),
            Token::Then
            | Token::Else
            | Token::Comma
            | Token::Colon
            | Token::Apostrophe
            | Token::At
            | Token::Nan
            | Token::Ident(_)
            | Token::Num(_) => {}
        }
        previous = Some(token);
    }
    operators + deepest
}

/// The refusal of an equation a tool is given that is deeper than
/// [`MAX_EQUATION_DEPTH`], or nothing: asked of every equation text an agent
/// sends before anything parses it.
pub(crate) fn equation_too_deep(text: &str) -> Option<String> {
    let depth = equation_depth(text);
    (depth > MAX_EQUATION_DEPTH).then(|| {
        format!(
            "the equation is nested too deeply to read safely: it measures {depth}, and one may \
             measure at most {MAX_EQUATION_DEPTH} (each operator counts 1, each level of \
             parentheses or brackets 2, each nested function call 8); compute its parts in \
             variables of their own"
        )
    })
}

/// Why input is not what a tool takes.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum Mismatch {
    /// The text is not JSON: the parser's reason, with its position in the
    /// text, which is all there is to point at.
    NotJson(String),
    /// The JSON is not the tool's input: where (empty for the input as a
    /// whole) and why.
    NotInput { path: String, reason: String },
}

/// `text` as a tool's input type.
pub(crate) fn parse<I: DeserializeOwned>(text: &str) -> Result<I, Mismatch> {
    let Unique(value) =
        serde_json::from_str(text).map_err(|err| Mismatch::NotJson(err.to_string()))?;
    let Failed {
        steps,
        reason,
        names,
    } = match read::<I>(&value) {
        Ok(input) => return Ok(input),
        Err(failed) => failed,
    };
    let steps = field_at_fault::<I>(&value, steps, &reason, &names);
    Err(Mismatch::NotInput {
        path: path_of(&steps),
        reason: plain(&reason),
    })
}

/// A read that failed: the steps to where, why, and the places of the names
/// it read as values before it failed (the tags of internally tagged enums).
struct Failed {
    steps: Vec<Step>,
    reason: String,
    names: Vec<Vec<Step>>,
}

/// `value` as the input type, or where and why the read failed.
fn read<I: DeserializeOwned>(value: &Value) -> Result<I, Failed> {
    let trail = Trail::default();
    I::deserialize(At {
        value,
        trail: &trail,
    })
    .map_err(|err: serde_json::Error| Failed {
        steps: trail.failed.take().unwrap_or_default(),
        reason: err.to_string(),
        names: trail.names.take(),
    })
}

/// The steps to the field of the object at `steps` that a mismatch is in,
/// when the read placed it at the object (an internally tagged enum's
/// variant is read from a copy, out of the tracked read's sight): the one
/// key whose removal makes the mismatch go away. Removing any other field
/// leaves it, since an object's entries are read in order before a missing
/// one is noted; removing the one at fault ends it, the read then failing
/// for the missing field or not at all. The tag is passed over: it was read
/// (`names`) and named the variant, and removing it ends any mismatch
/// inside the variant. `steps` unchanged when the value there is no object,
/// or no one key is at fault.
fn field_at_fault<I: DeserializeOwned>(
    value: &Value,
    steps: Vec<Step>,
    reason: &str,
    names: &[Vec<Step>],
) -> Vec<Step> {
    // A missing field is no field present to name.
    if reason.starts_with("missing field") {
        return steps;
    }
    let Some(Value::Object(entries)) = value_at(value, &steps) else {
        return steps;
    };
    let under = |key: &String| {
        let mut at = steps.clone();
        at.push(Step::Key(key.clone()));
        at
    };
    let at_fault: Vec<&String> = entries
        .keys()
        .filter(|key| !names.contains(&under(key)))
        .filter(|key| {
            let mut without = value.clone();
            if let Some(Value::Object(entries)) = value_at_mut(&mut without, &steps) {
                entries.remove(key.as_str());
            }
            !matches!(read::<I>(&without), Err(again) if again.reason == reason)
        })
        .collect();
    match at_fault.as_slice() {
        [key] => {
            let mut steps = steps;
            steps.push(Step::Key((*key).clone()));
            steps
        }
        _ => steps,
    }
}

fn value_at<'a>(value: &'a Value, steps: &[Step]) -> Option<&'a Value> {
    steps.iter().try_fold(value, |value, step| match step {
        Step::Key(key) => value.get(key),
        Step::Index(index) => value.get(index),
    })
}

fn value_at_mut<'a>(value: &'a mut Value, steps: &[Step]) -> Option<&'a mut Value> {
    steps.iter().try_fold(value, |value, step| match step {
        Step::Key(key) => value.get_mut(key),
        Step::Index(index) => value.get_mut(index),
    })
}

/// A JSON value with no key twice in any object: a text parser keeps the
/// last of a repeated key, which reads an input as other than what was sent.
struct Unique(Value);

impl<'de> serde::Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Unique, D::Error> {
        deserializer.deserialize_any(UniqueVisitor).map(Unique)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }

    fn visit_i64<E>(self, n: i64) -> Result<Value, E> {
        Ok(Value::from(n))
    }

    fn visit_u64<E>(self, n: u64) -> Result<Value, E> {
        Ok(Value::from(n))
    }

    fn visit_f64<E>(self, n: f64) -> Result<Value, E> {
        Ok(serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E>(self, s: &str) -> Result<Value, E> {
        Ok(Value::String(s.to_string()))
    }

    fn visit_string<E>(self, s: String) -> Result<Value, E> {
        Ok(Value::String(s))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Unique(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut entries = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let Unique(value) = map.next_value()?;
            if entries.contains_key(&key) {
                return Err(serde::de::Error::custom(format!(
                    "the key `{key}` appears twice in one object"
                )));
            }
            entries.insert(key, value);
        }
        Ok(Value::Object(entries))
    }
}

/// A deserializer's reason with the Rust types it names said as JSON's.
fn plain(reason: &str) -> String {
    [
        ("expected f64", "expected a number"),
        ("expected f32", "expected a number"),
        ("expected usize", "expected a whole number"),
        ("expected u64", "expected a whole number"),
        ("expected u32", "expected a whole number"),
        ("expected i64", "expected a whole number"),
    ]
    .iter()
    .fold(reason.to_string(), |reason, (rust, json)| {
        reason.replace(rust, json)
    })
}

/// One step into a JSON value.
#[derive(Clone, PartialEq)]
enum Step {
    Key(String),
    Index(usize),
}

/// The steps to the value being read, and the path of the first nested read
/// that failed: the deepest, since a failure is noted as it leaves the read
/// it happened in and only the first note is kept.
#[derive(Default)]
struct Trail {
    steps: RefCell<Vec<Step>>,
    failed: RefCell<Option<Vec<Step>>>,
    /// Where each name read as a value was ([`At::deserialize_identifier`]).
    names: RefCell<Vec<Vec<Step>>>,
}

impl Trail {
    /// Read under `step`, noting the path when the read fails.
    fn under<T>(
        &self,
        step: Step,
        read: impl FnOnce() -> Result<T, serde_json::Error>,
    ) -> Result<T, serde_json::Error> {
        self.steps.borrow_mut().push(step);
        let result = read();
        if result.is_err() {
            let mut failed = self.failed.borrow_mut();
            if failed.is_none() {
                *failed = Some(self.steps.borrow().clone());
            }
        }
        self.steps.borrow_mut().pop();
        result
    }
}

/// `steps` written as a path: `set[1].value`.
fn path_of(steps: &[Step]) -> String {
    let mut path = String::new();
    for step in steps {
        match step {
            Step::Key(key) => {
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
            }
            Step::Index(index) => path.push_str(&format!("[{index}]")),
        }
    }
    path
}

/// A JSON value, and where in the input it is.
#[derive(Clone, Copy)]
struct At<'a> {
    value: &'a Value,
    trail: &'a Trail,
}

impl<'de> Deserializer<'de> for At<'_> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.value {
            Value::Null => visitor.visit_unit(),
            Value::Bool(b) => visitor.visit_bool(*b),
            Value::Number(n) => {
                if let Some(u) = n.as_u64() {
                    visitor.visit_u64(u)
                } else if let Some(i) = n.as_i64() {
                    visitor.visit_i64(i)
                } else {
                    // A JSON number that is neither is a float.
                    visitor.visit_f64(n.as_f64().unwrap_or(f64::NAN))
                }
            }
            Value::String(s) => visitor.visit_str(s),
            // An internally tagged enum is read through here, and serde
            // would take a list as its tag and fields in order: an agent's
            // operation or citation is an object, its tag a field.
            Value::Array(_) if expects_tagged(&visitor) => Err(serde::de::Error::invalid_type(
                serde::de::Unexpected::Seq,
                &"an object",
            )),
            Value::Array(items) => visitor.visit_seq(Items {
                items: items.iter().enumerate(),
                trail: self.trail,
            }),
            Value::Object(entries) => visitor.visit_map(Entries {
                entries: entries.iter(),
                value: None,
                trail: self.trail,
            }),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.value {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    /// An externally tagged enum: a string names a unit variant, and an
    /// object with one entry a variant with its content. (An internally
    /// tagged enum, the tools' own, is read as any value.)
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.value {
            Value::String(variant) => {
                let variant: StrDeserializer<'_, serde_json::Error> =
                    variant.as_str().into_deserializer();
                visitor.visit_enum(variant)
            }
            Value::Object(entries) => visitor.visit_enum(MapAccessDeserializer::new(Entries {
                entries: entries.iter(),
                value: None,
                trail: self.trail,
            })),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    /// A struct is an object: a list of its fields in order, which serde
    /// would read as one, is no input an agent means.
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.value {
            Value::Object(_) => self.deserialize_any(visitor),
            other => Err(serde::de::Error::invalid_type(
                unexpected(other),
                &"an object",
            )),
        }
    }

    /// A name read as a value (an internally tagged enum's tag) is a string:
    /// a number, which serde would read as a variant's index, is none.
    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.value {
            Value::String(name) => {
                let at = self.trail.steps.borrow().clone();
                self.trail.names.borrow_mut().push(at);
                visitor.visit_str(name)
            }
            other => Err(serde::de::Error::invalid_type(unexpected(other), &"a name")),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map
    }
}

/// Whether `visitor` reads an internally tagged enum, as serde's derive
/// says what it expects: the one way to tell it from a list a field holds,
/// both being read as any value.
fn expects_tagged<'de, V: Visitor<'de>>(visitor: &V) -> bool {
    let expected: &dyn serde::de::Expected = visitor;
    expected.to_string().starts_with("internally tagged enum")
}

/// What a value is, as a mismatch names it.
fn unexpected(value: &Value) -> serde::de::Unexpected<'_> {
    use serde::de::Unexpected;
    match value {
        Value::Null => Unexpected::Unit,
        Value::Bool(b) => Unexpected::Bool(*b),
        Value::Number(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => Unexpected::Unsigned(u),
            (None, Some(i)) => Unexpected::Signed(i),
            _ => Unexpected::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => Unexpected::Str(s),
        Value::Array(_) => Unexpected::Seq,
        Value::Object(_) => Unexpected::Map,
    }
}

/// An array's items, each read under its index.
struct Items<'a> {
    items: std::iter::Enumerate<std::slice::Iter<'a, Value>>,
    trail: &'a Trail,
}

impl<'de> SeqAccess<'de> for Items<'_> {
    type Error = serde_json::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        let Some((index, value)) = self.items.next() else {
            return Ok(None);
        };
        let trail = self.trail;
        trail
            .under(Step::Index(index), || seed.deserialize(At { value, trail }))
            .map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len())
    }
}

/// An object's entries, each key and each value read under the key: a key
/// the input type does not have is a mismatch at that key.
struct Entries<'a> {
    entries: serde_json::map::Iter<'a>,
    value: Option<(&'a String, &'a Value)>,
    trail: &'a Trail,
}

impl<'de> MapAccess<'de> for Entries<'_> {
    type Error = serde_json::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.value = Some((key, value));
        self.trail
            .under(Step::Key(key.clone()), || {
                let key: StrDeserializer<'_, serde_json::Error> = key.as_str().into_deserializer();
                seed.deserialize(key)
            })
            .map(Some)
    }

    fn next_value_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        let Some((key, value)) = self.value.take() else {
            return Err(serde::de::Error::custom("a value is read after its key"));
        };
        let trail = self.trail;
        trail.under(Step::Key(key.clone()), || {
            seed.deserialize(At { value, trail })
        })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
