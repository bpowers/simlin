// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Names, matched forgivingly: the one owner of "which variable did the agent
//! mean", for a name that resolves ([`resolve`]) and for a phrase that has to
//! be searched for ([`rank`]).
//!
//! A name resolves when it canonicalizes to a variable's canonical name, so
//! case, spaces and underscores never matter. Otherwise candidates are ranked
//! by [`similarity`], which reads names as words: a misspelled word, a missing
//! or extra word, words in another order, or a phrase that is part of a name
//! all score, so a name misheard or half-remembered finds its variable. A
//! variable's documentation counts too, more weakly, so a description finds a
//! variable whose name it does not share.
//!
//! Stop words ("the", "of", ...) are dropped from what an agent asks for and
//! never from what a model names: in a phrase they carry nothing, but a name's
//! every word is part of it ("stock a" and "stock b" are two variables).

use crate::datamodel::{Model, Variable};

/// The least similarity a suggestion for an unresolved name needs.
pub(crate) const SUGGESTION_THRESHOLD: f64 = 0.6;
/// How many suggestions an unresolved name comes back with.
pub(crate) const MAX_SUGGESTIONS: usize = 3;
/// The longest phrase the fuzzy matcher compares, in characters: a
/// comparison costs the product of the two lengths, for every variable, and
/// no model's name comes near this. A longer phrase names nothing, and a
/// tool that searches by phrase refuses it.
pub(crate) const MAX_QUERY_CHARS: usize = 256;

/// Words a phrase carries that say nothing about which variable it means.
const STOP_WORDS: [&str; 9] = ["a", "an", "and", "of", "the", "in", "on", "to", "for"];

/// The variable `query` names, or the closest names when none does.
pub(crate) fn resolve<'a>(model: &'a Model, query: &str) -> Result<&'a Variable, Vec<String>> {
    model.get_variable(query).ok_or_else(|| {
        rank(model, query)
            .into_iter()
            .filter(|(score, _)| *score >= SUGGESTION_THRESHOLD)
            .take(MAX_SUGGESTIONS)
            .map(|(_, var)| var.get_ident().to_string())
            .collect()
    })
}

/// Why a reference names nothing to read: the closest names when no variable
/// has the name, or the reason when the variable exists and the element asked
/// of it does not.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Default)]
pub(crate) struct Unresolved {
    pub suggestions: Vec<String>,
    pub reason: Option<String>,
}

/// What `reference` names: a variable, or with a subscript one element of an
/// arrayed variable, spelled as the project's dimensions spell it.
pub(crate) fn resolve_reference<'a>(
    project: &crate::datamodel::Project,
    model: &'a Model,
    reference: &str,
) -> Result<(&'a Variable, Option<String>), Unresolved> {
    let unknown = |suggestions| Unresolved {
        suggestions,
        reason: None,
    };
    if let Some(var) = model.get_variable(reference) {
        return Ok((var, None));
    }
    let Some((base, subscripts)) = split_subscript(reference) else {
        return resolve(model, reference)
            .map(|var| (var, None))
            .map_err(unknown);
    };
    let var = resolve(model, base).map_err(unknown)?;
    let dims = match var.get_equation() {
        Some(
            crate::datamodel::Equation::ApplyToAll(dims, _)
            | crate::datamodel::Equation::Arrayed(dims, ..),
        ) => dims.as_slice(),
        Some(crate::datamodel::Equation::Scalar(_)) | None => &[],
    };
    resolve_element(project, dims, &subscripts)
        .map(|element| (var, Some(element)))
        .map_err(|reason| Unresolved {
            suggestions: vec![],
            reason: Some(format!("{}: {reason}", var.get_ident())),
        })
}

/// A name with a subscript, `population[north]` or `flow[a, b]`: the name
/// before the bracket and each subscript, trimmed; `None` for a name with no
/// subscript or an empty one.
pub(crate) fn split_subscript(query: &str) -> Option<(&str, Vec<&str>)> {
    let query = query.trim();
    let open = query.find('[')?;
    let inner = query.strip_suffix(']')?.get(open + 1..)?;
    let base = query[..open].trim();
    let subscripts: Vec<&str> = inner.split(',').map(str::trim).collect();
    (!base.is_empty() && subscripts.iter().all(|s| !s.is_empty())).then_some((base, subscripts))
}

/// The element `subscripts` name of a variable arrayed over `dims` (the
/// dimension names its equation lists), spelled as the project's dimensions
/// spell it and joined with commas (`north`, `a,b`), or why no element is
/// named. A subscript matches an element case, spaces and underscores aside,
/// and an indexed dimension's elements are its numbers from 1.
pub(crate) fn resolve_element(
    project: &crate::datamodel::Project,
    dims: &[String],
    subscripts: &[&str],
) -> Result<String, String> {
    if dims.is_empty() {
        return Err("it is not arrayed".to_string());
    }
    if dims.len() != subscripts.len() {
        return Err(format!(
            "it is arrayed over {} ({}), so an element takes {} subscript{}",
            dims.len(),
            dims.join(", "),
            dims.len(),
            if dims.len() == 1 { "" } else { "s" }
        ));
    }
    let mut parts = Vec::with_capacity(dims.len());
    for (dim_name, subscript) in dims.iter().zip(subscripts) {
        let dim = project
            .dimensions
            .iter()
            .find(|d| crate::canonicalize(&d.name) == crate::canonicalize(dim_name));
        let found = match dim.map(|d| &d.elements) {
            Some(crate::datamodel::DimensionElements::Named(elements)) => elements
                .iter()
                .find(|e| crate::canonicalize(e) == crate::canonicalize(subscript))
                .cloned(),
            Some(crate::datamodel::DimensionElements::Indexed(size)) => subscript
                .parse::<u32>()
                .ok()
                .filter(|i| (1..=*size).contains(i))
                .map(|i| i.to_string()),
            None => None,
        };
        let Some(element) = found else {
            let known = match dim.map(|d| &d.elements) {
                Some(crate::datamodel::DimensionElements::Named(elements)) => {
                    let shown: Vec<&str> = elements.iter().take(12).map(String::as_str).collect();
                    let more = elements.len().saturating_sub(shown.len());
                    format!(
                        "{}{}",
                        shown.join(", "),
                        if more > 0 {
                            format!(" and {more} more")
                        } else {
                            String::new()
                        }
                    )
                }
                Some(crate::datamodel::DimensionElements::Indexed(size)) => format!("1 to {size}"),
                None => "none the project defines".to_string(),
            };
            return Err(format!(
                "`{subscript}` is not an element of {dim_name} (its elements: {known})"
            ));
        };
        parts.push(element);
    }
    Ok(parts.join(","))
}

/// Every variable of `model` with its similarity to `phrase`, closest first
/// (ties by name, so the order is a function of the model); none for a
/// phrase longer than [`MAX_QUERY_CHARS`].
pub(crate) fn rank<'a>(model: &'a Model, phrase: &str) -> Vec<(f64, &'a Variable)> {
    if phrase.chars().count() > MAX_QUERY_CHARS {
        return Vec::new();
    }
    let query = query_words(phrase);
    let mut ranked: Vec<(f64, &Variable)> = model
        .variables
        .iter()
        .map(|var| (score(&query, var), var))
        .collect();
    ranked.sort_by(|(a, va), (b, vb)| {
        b.total_cmp(a)
            .then_with(|| va.get_ident().cmp(vb.get_ident()))
    });
    ranked
}

/// How well `query` names or describes `var`: its name's similarity, or
/// how much of the query its documentation covers, a likeness (at most
/// [`FUZZY_CEILING`]), whichever is higher. A query shorter than
/// [`MIN_FUZZY_CHARS`] covers documentation only by the words it starts.
fn score(query: &[String], var: &Variable) -> f64 {
    let name = similarity(query, &words(var.get_ident()));
    let documentation = match var {
        Variable::Stock(s) => s.documentation.as_str(),
        Variable::Flow(f) => f.documentation.as_str(),
        Variable::Aux(a) => a.documentation.as_str(),
        Variable::Module(m) => m.documentation.as_str(),
    };
    let documentation = words(documentation);
    let covers = if query.join(" ").chars().count() < MIN_FUZZY_CHARS {
        let begun = query
            .iter()
            .filter(|q| documentation.iter().any(|w| w.starts_with(q.as_str())))
            .count();
        begun as f64 / query.len().max(1) as f64
    } else {
        covered(query, &documentation)
    };
    name.max(FUZZY_CEILING * covers)
}

/// The words of a name or text: canonicalized (lowercase, spaces and
/// underscores alike) and split on anything that is not a letter or a digit.
pub(crate) fn words(text: &str) -> Vec<String> {
    crate::canonicalize(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The words of a phrase an agent asks with: its [`words`] with the stop words
/// dropped, when anything else is left.
pub(crate) fn query_words(phrase: &str) -> Vec<String> {
    let all = words(phrase);
    let content: Vec<String> = all
        .iter()
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .cloned()
        .collect();
    if content.is_empty() { all } else { content }
}

/// The similarity of a query's words to a name's, in `[0, 1]`, in three
/// tiers that never overlap, so a match of a better kind always ranks first:
///
/// - 1 for the name itself;
/// - from 0.75 for the query's words starting the name's words, in order (a
///   phrase that is part of a name, its words whole or begun: "pop" or
///   "population gr"), higher the more of the name it is. A word is matched
///   from its start, never inside it: "age" is no part of "average" or
///   "usage";
/// - below that ([`FUZZY_CEILING`]), a likeness: the best of the two joined
///   compared as strings (a typo across a word boundary, a missing space)
///   and the words compared as sets (a misspelled word, words reordered, a
///   word missing or extra). A query shorter than [`MIN_FUZZY_CHARS`] has
///   none: one letter off in three is another word, not a typo.
pub(crate) fn similarity(query: &[String], name: &[String]) -> f64 {
    if query.is_empty() || name.is_empty() {
        return 0.0;
    }
    if query == name {
        return 1.0;
    }
    let q = query.join(" ");
    let n = name.join(" ");
    let begun = |run: &[String]| {
        run.iter()
            .zip(query)
            .all(|(n, q)| n.starts_with(q.as_str()))
    };
    // A query's stop words are dropped ([`query_words`]), so its words
    // start a name's with the name's dropped too: "life of land".
    let content: Vec<String> = name
        .iter()
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .cloned()
        .collect();
    if name.windows(query.len()).any(begun) || content.windows(query.len()).any(begun) {
        return 0.75 + 0.25 * (q.len() as f64 / n.len() as f64);
    }
    if q.chars().count() < MIN_FUZZY_CHARS {
        return 0.0;
    }
    let whole = string_similarity(&q, &n);
    let words = 0.7 * covered(query, name) + 0.3 * covered(name, query);
    whole.max(words).min(FUZZY_CEILING)
}

/// The highest a likeness scores ([`similarity`]): under the least a
/// phrase that starts a name's words scores, so word starts outrank it.
pub(crate) const FUZZY_CEILING: f64 = 0.74;

/// The fewest characters a query has for a likeness to count
/// ([`similarity`]).
pub(crate) const MIN_FUZZY_CHARS: usize = 4;

/// How well `words` are matched by `by`: the mean, over `words`, of each
/// word's best string similarity to a word of `by`.
fn covered(words: &[String], by: &[String]) -> f64 {
    if words.is_empty() || by.is_empty() {
        return 0.0;
    }
    let total: f64 = words
        .iter()
        .map(|w| {
            by.iter()
                .map(|b| string_similarity(w, b))
                .fold(0.0, f64::max)
        })
        .sum();
    total / words.len() as f64
}

/// `1 - distance / longer length`, over characters, with the Levenshtein
/// distance: 1 for equal strings, 0 for strings with nothing in common.
pub(crate) fn string_similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longer = a.len().max(b.len());
    if longer == 0 {
        return 1.0;
    }
    1.0 - levenshtein(&a, &b) as f64 / longer as f64
}

/// The fewest single-character insertions, deletions and substitutions that
/// turn `a` into `b`.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
