// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What an XMILE file holds that the datamodel does not keep, reported as
//! [`ImportWarning`]s so a host can say at open what saving the project
//! would lose.
//!
//! Reported, once per kind of element and place: the objects a stock-and-flow
//! view holds besides its diagram (a graph, a text box, a button), the
//! objects any other view (an interface page) holds, since no such view is
//! read, the stories of story mode, and what a model's variables hold that
//! is not a variable (a standalone graphical function, a group's member
//! list). Not reported: a view's style and simulation delay, an empty page,
//! page templates, a tool's preferences, window layout and time formats, and
//! Stella's record of each variable's dependencies
//! ([`UnreadElement::is_reported`]). The files a tool writes carry those
//! whatever the model is, so a warning about them would appear on every
//! open and say nothing about the model.

use crate::import_losses::{ImportWarning, not_kept};
use crate::xmile::File;
use crate::xmile::variables::Var;
use crate::xmile::views::{UnreadElement, ViewObject, ViewType};

#[cfg(test)]
#[path = "unread_tests.rs"]
mod tests;

/// Every warning for what `file` holds that the datamodel does not keep, in
/// file order.
pub(crate) fn unread_content(file: &File) -> Vec<ImportWarning> {
    let mut warnings = Vec::new();
    let several = file.models.len() > 1;
    for model in &file.models {
        // A place within a model, and which model when the file has several.
        let of_model = |place: String| {
            if several {
                format!("{place} of model '{}'", model.get_name())
            } else {
                place
            }
        };

        if let Some(variables) = &model.variables {
            let unread = variables.variables.iter().filter_map(|var| match var {
                Var::Unhandled(element) => Some(element),
                _ => None,
            });
            let place = if several {
                format!("in model '{}'", model.get_name())
            } else {
                "in the model".to_string()
            };
            report_by_tag(&place, unread, &mut warnings);
        }

        let Some(views) = &model.views else {
            continue;
        };
        let all_views: Vec<_> = views.view.iter().flatten().collect();
        // How many views of each kind there are, and how many of each have
        // been placed, so an unnamed view is called by its number among its
        // kind (the only view of its kind has no number).
        let of_kind = |kind: ViewType| {
            all_views
                .iter()
                .filter(|view| view.kind.unwrap_or(ViewType::StockFlow) == kind)
                .count()
        };
        let mut numbered: Vec<(ViewType, usize)> = Vec::new();
        for view in &all_views {
            let kind = view.kind.unwrap_or(ViewType::StockFlow);
            let number = match numbered.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    numbered.push((kind, 1));
                    1
                }
            };
            let unread = view.objects.iter().filter_map(|object| match object {
                ViewObject::Unhandled(element) => Some(element),
                _ => None,
            });
            let noun = match kind {
                ViewType::StockFlow if view.name.is_some() => "view",
                ViewType::StockFlow => "diagram",
                ViewType::Interface => "interface page",
                ViewType::Popup => "popup",
                ViewType::VendorSpecific => "view",
            };
            let place = match &view.name {
                Some(name) => format!("on {noun} '{name}'"),
                None if of_kind(kind) > 1 => format!("on {noun} {number}"),
                None => format!("on the {noun}"),
            };
            report_by_tag(&of_model(place), unread, &mut warnings);
        }

        let stories: Vec<&UnreadElement> = views
            .stories
            .iter()
            .flat_map(|stories| stories.children.iter().map(|child| &child.0))
            .filter(|child| child.is_reported())
            .collect();
        if !stories.is_empty() {
            let names: Vec<String> = stories
                .iter()
                .map(|story| story.label.clone().unwrap_or_default())
                .collect();
            let place = of_model("in story mode".to_string());
            warnings.push(not_kept(stories.len(), "story", "stories", &place, &names));
        }
    }
    warnings
}

/// The words for one element of `tag` and for several: the tag's own words
/// (`text_box`: `text box`, `text boxes`), except where they would not say
/// what it is.
fn noun(tag: &str) -> (String, String) {
    let (one, many) = match tag {
        // Stella's container for graphs and tables, shown one at a time.
        "stacked_container" => ("graph or table", "graphs and tables"),
        // A set of radio buttons.
        "options" => ("option group", "option groups"),
        "gf" => ("graphical function", "graphical functions"),
        // Stella's group lists the variables that belong to it. The group's
        // frame on the diagram is read; the list is not.
        "group" => ("group's member list", "groups' member lists"),
        _ => {
            let one = tag.replace('_', " ");
            let many = if one.ends_with(['s', 'x']) || one.ends_with("ch") || one.ends_with("sh") {
                format!("{one}es")
            } else {
                format!("{one}s")
            };
            return (one, many);
        }
    };
    (one.to_string(), many.to_string())
}

/// One warning per tag of `elements`, found `place`, in the order each tag
/// first appears.
fn report_by_tag<'a>(
    place: &str,
    elements: impl Iterator<Item = &'a UnreadElement>,
    warnings: &mut Vec<ImportWarning>,
) {
    let mut tags: Vec<(&str, Vec<String>)> = Vec::new();
    for element in elements.filter(|element| element.is_reported()) {
        let label = element.label.clone().unwrap_or_default();
        match tags.iter_mut().find(|(tag, _)| *tag == element.tag) {
            Some((_, labels)) => labels.push(label),
            None => tags.push((element.tag.as_str(), vec![label])),
        }
    }
    for (tag, labels) in tags {
        let (one, many) = noun(tag);
        warnings.push(not_kept(labels.len(), &one, &many, place, &labels));
    }
}
