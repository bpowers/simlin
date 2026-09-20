// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a reader reports it does not keep ([`ImportWarning`]), and the
//! wording the readers share.

use crate::errors::join_quoted_names;

/// Something a file holds that its reader does not bring into the project,
/// so a save of the project, in that file's format or any other, does not
/// write it back: a sketch's comment, an interface page's slider, a custom
/// graph. A reader reports its losses so a host can say, when the file
/// opens, what saving it would lose. Losses are reported once per kind and
/// place (a view, a page, the model), with a count and examples, never once
/// per element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportWarning {
    /// What is not kept and where, as a sentence a person reads: `29
    /// comments on view 'View 1' are not kept, such as 'The World3 Model'`.
    pub message: String,
}

/// How many examples a warning names.
const EXAMPLES: usize = 3;

/// How many characters of an example a warning shows.
const LONGEST: usize = 40;

/// How many of a thing there are, in words: `1 image`, `3 images`.
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The names a warning can show, in the order given: each on one line and
/// cut to a readable length, each once, and none that is empty (an empty
/// name is nothing a person would recognize).
fn shown_names(names: &[String]) -> Vec<String> {
    let mut shown: Vec<String> = Vec::new();
    for name in names {
        let line = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = match line.char_indices().nth(LONGEST) {
            Some((end, _)) => format!("{}...", line[..end].trim_end()),
            None => line,
        };
        if !line.is_empty() && !shown.contains(&line) {
            shown.push(line);
        }
    }
    shown
}

/// A warning that `n` things of a kind, found `where_`, are not kept, with
/// up to three of their `names` as examples:
///
/// - `29 comments on view 'View 1' are not kept, such as 'a', 'b', and 'c'`
/// - `2 sliders on view 'View 1' are not kept: 'a' and 'b'`, when every one
///   is named
/// - `12 arrows on view 'View 1' are not kept`, when none is
///
/// `where_` is a phrase that places them: `on view 'View 1'`, `in the model`.
pub(crate) fn not_kept(
    n: usize,
    one: &str,
    many: &str,
    where_: &str,
    names: &[String],
) -> ImportWarning {
    let verb = if n == 1 { "is" } else { "are" };
    let mut message = format!("{} {where_} {verb} not kept", count(n, one, many));
    let shown = shown_names(names);
    if !shown.is_empty() {
        let listed: Vec<&str> = shown.iter().take(EXAMPLES).map(String::as_str).collect();
        let every_one = shown.len() == n && n <= EXAMPLES;
        let joiner = if every_one { ": " } else { ", such as " };
        message.push_str(joiner);
        message.push_str(&join_quoted_names(&listed));
    }
    ImportWarning { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_warning_counts_places_and_names_what_is_not_kept() {
        assert_eq!(
            not_kept(1, "image", "images", "on view 'Main'", &names(&["a.bmp"])).message,
            "1 image on view 'Main' is not kept: 'a.bmp'"
        );
        assert_eq!(
            not_kept(
                2,
                "slider",
                "sliders",
                "on view 'Main'",
                &names(&["a", "b"])
            )
            .message,
            "2 sliders on view 'Main' are not kept: 'a' and 'b'"
        );
        // More than three, or some unnamed: examples, not the whole list.
        assert_eq!(
            not_kept(
                5,
                "comment",
                "comments",
                "on view 'Main'",
                &names(&[
                    "Population",
                    "",
                    "  Capital\n  sector ",
                    "Food",
                    "Pollution"
                ])
            )
            .message,
            "5 comments on view 'Main' are not kept, such as \
             'Population', 'Capital sector', and 'Food'"
        );
        assert_eq!(
            not_kept(
                3,
                "comment",
                "comments",
                "on view 'Main'",
                &names(&["a", "", "b"])
            )
            .message,
            "3 comments on view 'Main' are not kept, such as 'a' and 'b'"
        );
        // Nothing to name: the count alone.
        assert_eq!(
            not_kept(2, "shape", "shapes", "on view 'Main'", &names(&["", " "])).message,
            "2 shapes on view 'Main' are not kept"
        );
    }

    #[test]
    fn an_example_is_named_once_on_one_readable_line() {
        // Three copies of one name are not three examples.
        assert_eq!(
            not_kept(
                3,
                "variable",
                "variables",
                "on view 'Main'",
                &names(&["Time", "Time", "Time"])
            )
            .message,
            "3 variables on view 'Main' are not kept, such as 'Time'"
        );
        let long = "x".repeat(60);
        assert_eq!(shown_names(&[long]), vec![format!("{}...", "x".repeat(40))]);
    }
}
