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
//! list). Also reported: a graphical function attribute the reader does not
//! act on (a type it does not know, a draft-format `discrete="true"`),
//! wherever the reader reads a graphical function, a macro's body included,
//! with the kind the function is read as. Not reported: a view's
//! style and simulation delay, an empty page,
//! page templates, a tool's preferences, window layout and time formats, and
//! Stella's record of each variable's dependencies (`UNREPORTED`). The files
//! a tool writes carry those whatever the model is, so a warning about them
//! would appear on every open and say nothing about the model.
//!
//! The report is a pass of its own over the file's XML events, not a part of
//! the read. The reader goes on skipping what it does not model exactly as it
//! always has, so a report can neither change the project nor fail the open.
//! The pass holds no recursion, so an element nested ten thousand deep costs
//! it no stack. It finds each view's number and each tag's warning by hash,
//! so its time grows with the file and not with the file's square: a file can
//! hold fifty thousand pages or a hundred thousand kinds of element. It reads
//! only as far into an unread element as a label needs (`LABEL_DEPTH`), and a
//! label leaves out whatever does not unescape. If the XML stops parsing, the
//! report ends there, with what it found.

use std::borrow::Cow;
use std::collections::HashMap;

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::import_losses::{ImportWarning, counted, not_kept};
use crate::xmile::dimensions::GraphicalFunctionKind;

#[cfg(test)]
#[path = "unread_tests.rs"]
mod tests;

/// The tags the reader reads among a model's variables.
const VARIABLES: &[&[u8]] = &[b"stock", b"flow", b"aux", b"module"];

/// The variables whose own `<gf>` the reader reads (`xmile::Flow::gf`,
/// `xmile::Aux::gf`).
const OWN_TABLE: &[&[u8]] = &[b"flow", b"aux"];

/// The variables whose `<element>`s the reader reads, each of which can hold
/// a `<gf>` (`xmile::VarElement::gf`).
const ELEMENT_TABLES: &[&[u8]] = &[b"stock", b"flow", b"aux"];

/// The tags the reader reads on a stock-and-flow view.
const VIEW_OBJECTS: &[&[u8]] = &[
    b"aux",
    b"stock",
    b"flow",
    b"connector",
    b"module",
    b"cloud",
    b"alias",
    b"group",
];

/// The unread elements the report leaves out, since the files a tool writes
/// carry them whatever the model is: a view's settings (its style, Stella's
/// simulation delay) and a tool's own bookkeeping, which it rebuilds
/// (Stella's record of each variable's dependencies).
const UNREPORTED: &[&[u8]] = &[b"style", b"simulation_delay", b"dependencies"];

/// How many levels below an unread element its label may come from. A
/// Stella graph is known by the variable it plots, three levels down: the
/// stacked container's graph's plot's entity.
const LABEL_DEPTH: usize = 3;

/// The children whose own label describes the element holding them.
const DESCRIBING_CHILDREN: &[&[u8]] = &[b"graph", b"plot", b"popup", b"text"];

/// Every warning for what `xml` holds that the datamodel does not keep, in
/// file order: for each model, its variables, then its views, then its
/// stories.
pub(crate) fn unread_content(xml: &[u8]) -> Vec<ImportWarning> {
    let models = scan(xml);
    let several = models.iter().filter(|model| !model.in_macro).count() > 1;
    let mut warnings = Vec::new();
    for model in &models {
        // A place within a model, and which model when the file has several.
        let of_model = |place: String| {
            if several {
                format!("{place} of model '{}'", model.name)
            } else {
                place
            }
        };

        let place = if model.in_macro {
            format!("in macro '{}'", model.name)
        } else if several {
            format!("in model '{}'", model.name)
        } else {
            "in the model".to_string()
        };
        report_by_tag(&place, &model.variables, true, &mut warnings);
        report_unread_gf_attributes(&place, &model.unread_gf_attributes, &mut warnings);

        // An unnamed view is called by its number among the views of its
        // kind; the only view of its kind has no number.
        let mut of_kind: HashMap<ViewKind, usize> = HashMap::new();
        for view in &model.views {
            *of_kind.entry(view.kind).or_default() += 1;
        }
        let mut numbered: HashMap<ViewKind, usize> = HashMap::new();
        for view in &model.views {
            let number = numbered.entry(view.kind).or_default();
            *number += 1;
            let number = *number;
            let noun = match view.kind {
                ViewKind::StockFlow if view.name.is_some() => "view",
                ViewKind::StockFlow => "diagram",
                ViewKind::Interface => "interface page",
                ViewKind::Popup => "popup",
                ViewKind::Other => "view",
            };
            let place = match &view.name {
                Some(name) => format!("on {noun} '{name}'"),
                None if of_kind[&view.kind] > 1 => format!("on {noun} {number}"),
                None => format!("on the {noun}"),
            };
            report_by_tag(&of_model(place), &view.unread, false, &mut warnings);
        }

        if !model.stories.is_empty() {
            let names: Vec<String> = model
                .stories
                .iter()
                .map(|story| story.label.clone().unwrap_or_default())
                .collect();
            let place = of_model("in story mode".to_string());
            warnings.push(not_kept(names.len(), "story", "stories", &place, &names));
        }
    }
    warnings
}

/// An element the reader does not keep: its tag without a namespace prefix,
/// and what it is called, when it says (see [`Label`]).
struct Unread {
    tag: String,
    label: Option<String>,
}

/// A view's type, as its `type` attribute names it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ViewKind {
    StockFlow,
    Interface,
    Popup,
    Other,
}

struct View {
    kind: ViewKind,
    name: Option<String>,
    unread: Vec<Unread>,
}

/// A `<gf>` attribute the reader does not act on.
struct UnreadGfAttribute {
    /// As the file spells it, with the variable it is on:
    /// `type="stepwise" on effect`.
    spelled: String,
    /// The kind the function's `type` names, which the reader reads it as;
    /// None when the type names none and the function is read as the default
    /// kind.
    read_by_type: Option<GraphicalFunctionKind>,
}

/// What one model, or one macro's body, holds that the datamodel does not
/// keep, by place.
struct Model {
    name: String,
    /// A `<macro>`, whose body the reader reads with a model's content model
    /// (`xmile::Macro::variables`). Only its graphical functions are looked
    /// at.
    in_macro: bool,
    variables: Vec<Unread>,
    unread_gf_attributes: Vec<UnreadGfAttribute>,
    views: Vec<View>,
    stories: Vec<Unread>,
}

/// Where the pass is, among the elements outside any unread one.
#[derive(Clone, Copy)]
enum Scope {
    File,
    Model,
    Variables,
    /// A macro, and its body's variables, where only graphical functions are
    /// looked at.
    Macro,
    MacroVariables,
    /// A variable whose own `<gf>` the reader reads (`OWN_TABLE`) or whose
    /// `<element>`s it reads (`ELEMENT_TABLES`).
    Variable {
        own_table: bool,
        element_tables: bool,
    },
    /// One of a variable's `<element>`s, whose `<gf>` the reader reads.
    Element,
    Views,
    View(ViewKind),
    Stories,
    /// Anywhere the report does not look.
    Elsewhere,
}

/// Where an unread element sits, in the last model and view begun.
#[derive(Clone, Copy)]
enum Place {
    Variables,
    View,
    Stories,
}

/// What a labeled element holds that could name it: its `name`, its `title`
/// or `label` (Stella's `isee:page_title` counts as a title), the variable
/// its first entity names, what its first describing child is called (a
/// graph, a graph's plot, an annotation's popup and its text), or its own
/// text, in that order. A slider has a title and the variable it sets; a
/// text box has only its text; an untitled graph is known by what it plots.
struct Label {
    name: Option<String>,
    title: Option<String>,
    entity: Option<String>,
    child: Option<String>,
    /// The element's own text so far, or None once a piece of it does not
    /// unescape: a text missing a piece would misquote the file.
    text: Option<String>,
}

impl Label {
    fn from_attributes(element: &BytesStart) -> Label {
        let mut label = Label {
            name: None,
            title: None,
            entity: None,
            child: None,
            text: Some(String::new()),
        };
        // An attribute that does not parse or unescape names nothing.
        for attribute in element.attributes().flatten() {
            let Some(value) = value_of(&attribute) else {
                continue;
            };
            match attribute.key.local_name().as_ref() {
                b"name" => keep_first(&mut label.name, &value),
                b"title" | b"page_title" | b"label" => keep_first(&mut label.title, &value),
                _ => {}
            }
        }
        label
    }

    /// The first of what the element is called that says anything.
    fn best(self) -> Option<String> {
        let text = self.text.filter(|text| !text.trim().is_empty());
        self.name
            .or(self.title)
            .or(self.entity)
            .or(self.child)
            .or(text)
    }
}

/// An attribute's value as the read sees it, or None when it does not
/// unescape.
fn value_of<'a>(attribute: &Attribute<'a>) -> Option<Cow<'a, str>> {
    attribute.normalized_value(XmlVersion::default()).ok()
}

/// Keeps the first `value` that says anything.
fn keep_first(slot: &mut Option<String>, value: &str) {
    if slot.is_none() && !value.trim().is_empty() {
        *slot = Some(value.to_owned());
    }
}

/// An element inside an unread one, as the pass keeps it.
enum Frame {
    /// The unread element itself, whose label the frames below it feed.
    Root(Label),
    /// An entity, which names a variable.
    Entity(Label),
    /// A child whose own label describes its parent.
    Describing(Label),
    /// Anything the label does not read.
    Skipped,
}

impl Frame {
    fn label(&mut self) -> Option<&mut Label> {
        match self {
            Frame::Root(label) | Frame::Entity(label) | Frame::Describing(label) => Some(label),
            Frame::Skipped => None,
        }
    }
}

/// The unread element whose subtree the pass is inside: its tag, where it
/// sits, and the frames open within it.
struct Capture {
    tag: String,
    place: Place,
    frames: Vec<Frame>,
}

/// The unread elements of every model in `xml`, by place.
fn scan(xml: &[u8]) -> Vec<Model> {
    let mut reader = Reader::from_reader(xml);
    // Configured as the deserializer configures its own reader, so the pass
    // meets the elements the read meets: every element has an end.
    reader.config_mut().expand_empty_elements = true;

    let mut models: Vec<Model> = Vec::new();
    let mut scopes: Vec<Scope> = Vec::new();
    let mut capture: Option<Capture> = None;
    // The variable the pass is inside, by name.
    let mut table_of: Option<String> = None;
    loop {
        let event = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(event) => event,
        };
        if let Some(open) = &mut capture {
            if open.feed(event) {
                let Capture {
                    tag,
                    place,
                    mut frames,
                } = capture.take().expect("open");
                let label = frames.pop().and_then(|frame| match frame {
                    Frame::Root(label) => label.best(),
                    _ => None,
                });
                if let Some(model) = models.last_mut() {
                    let unread = Unread { tag, label };
                    match place {
                        Place::Variables => model.variables.push(unread),
                        Place::Stories => model.stories.push(unread),
                        Place::View => {
                            if let Some(view) = model.views.last_mut() {
                                view.unread.push(unread);
                            }
                        }
                    }
                }
            }
            continue;
        }
        match event {
            Event::Start(element) => {
                let local = element.local_name();
                let tag = local.as_ref();
                let parent = scopes.last().copied();
                let unread_at = |place: Place| {
                    (!UNREPORTED.contains(&tag)).then(|| Capture {
                        tag: String::from_utf8_lossy(tag).into_owned(),
                        place,
                        frames: vec![Frame::Root(Label::from_attributes(&element))],
                    })
                };
                let scope = match (parent, tag) {
                    // The read does not look at the root's name either.
                    (None, _) => Scope::File,
                    (Some(Scope::File), tag @ (b"model" | b"macro")) => {
                        let in_macro = tag == b"macro";
                        let name = Label::from_attributes(&element).name;
                        models.push(Model {
                            name: name.unwrap_or_else(|| "main".to_string()),
                            in_macro,
                            variables: Vec::new(),
                            unread_gf_attributes: Vec::new(),
                            views: Vec::new(),
                            stories: Vec::new(),
                        });
                        if in_macro { Scope::Macro } else { Scope::Model }
                    }
                    (Some(Scope::Model), b"variables") => Scope::Variables,
                    (Some(Scope::Macro), b"variables") => Scope::MacroVariables,
                    (Some(Scope::Model), b"views") => Scope::Views,
                    (Some(Scope::Views), b"view") => {
                        let kind = view_kind(&element);
                        if let Some(model) = models.last_mut() {
                            model.views.push(View {
                                kind,
                                name: Label::from_attributes(&element).name,
                                unread: Vec::new(),
                            });
                        }
                        Scope::View(kind)
                    }
                    (Some(Scope::Views), b"stories") => Scope::Stories,
                    (Some(Scope::Variables), tag) if !VARIABLES.contains(&tag) => {
                        capture = unread_at(Place::Variables);
                        Scope::Elsewhere
                    }
                    (Some(Scope::Variables | Scope::MacroVariables), tag)
                        if OWN_TABLE.contains(&tag) || ELEMENT_TABLES.contains(&tag) =>
                    {
                        table_of = Label::from_attributes(&element).name;
                        Scope::Variable {
                            own_table: OWN_TABLE.contains(&tag),
                            element_tables: ELEMENT_TABLES.contains(&tag),
                        }
                    }
                    (
                        Some(Scope::Variable {
                            element_tables: true,
                            ..
                        }),
                        b"element",
                    ) => Scope::Element,
                    (
                        Some(
                            Scope::Variable {
                                own_table: true, ..
                            }
                            | Scope::Element,
                        ),
                        b"gf",
                    ) => {
                        if let Some(model) = models.last_mut() {
                            let variable = table_of.as_deref().unwrap_or_default();
                            let (unread, read_by_type) = unread_gf_attributes(&element);
                            model
                                .unread_gf_attributes
                                .extend(unread.into_iter().map(|spelled| UnreadGfAttribute {
                                    spelled: format!("{spelled} on {variable}"),
                                    read_by_type,
                                }));
                        }
                        Scope::Elsewhere
                    }
                    (Some(Scope::View(ViewKind::StockFlow)), tag)
                        if VIEW_OBJECTS.contains(&tag) =>
                    {
                        Scope::Elsewhere
                    }
                    (Some(Scope::View(_)), _) => {
                        capture = unread_at(Place::View);
                        Scope::Elsewhere
                    }
                    (Some(Scope::Stories), _) => {
                        capture = unread_at(Place::Stories);
                        Scope::Elsewhere
                    }
                    _ => Scope::Elsewhere,
                };
                // A captured element's end closes the capture, not a scope.
                if capture.is_none() {
                    scopes.push(scope);
                }
            }
            Event::End(_) => {
                scopes.pop();
            }
            _ => {}
        }
    }
    models
}

/// The attributes of a `<gf>` the reader does not act on, each as the file
/// spells it, and the kind its `type` names, which the reader reads it as
/// (None: the default kind).
///
/// `type` is XMILE's (1.0 section 4.1.3), and the reader's own
/// `GraphicalFunctionKind::from_type_attribute` says which of its values are
/// known. `discrete="true"` is not XMILE 1.0's for a graphical function: the
/// corpus holds `<gf discrete="false">` from a writer of the draft format
/// (`test/test-models/tests/lookups/test_lookups_no-indirect.xmile`, written
/// by go-xmile). That `discrete="true"` means what `type="discrete"` does is
/// unverified, so the reader does not act on it and it is reported here;
/// `discrete="false"` asks for nothing the default kind is not.
fn unread_gf_attributes(gf: &BytesStart) -> (Vec<String>, Option<GraphicalFunctionKind>) {
    let mut unread = Vec::new();
    let mut read_by_type = None;
    for attribute in gf.attributes().flatten() {
        let Some(value) = value_of(&attribute) else {
            continue;
        };
        match attribute.key.local_name().as_ref() {
            b"type" => match GraphicalFunctionKind::from_type_attribute(&value) {
                Some(kind) => read_by_type = Some(kind),
                None => unread.push(format!("type=\"{value}\"")),
            },
            b"discrete" if value.trim().eq_ignore_ascii_case("true") => {
                unread.push(format!("discrete=\"{value}\""));
            }
            _ => {}
        }
    }
    (unread, read_by_type)
}

/// One warning per kind the graphical functions holding `attributes` are
/// read as, in the order each first appears, saying whether that is the
/// default kind or the kind their `type` names.
fn report_unread_gf_attributes(
    place: &str,
    attributes: &[UnreadGfAttribute],
    warnings: &mut Vec<ImportWarning>,
) {
    // At most one group per kind and one for the default, so a linear search
    // keeps the pass linear.
    let mut groups: Vec<(Option<GraphicalFunctionKind>, Vec<String>)> = Vec::new();
    for attribute in attributes {
        match groups
            .iter_mut()
            .find(|(read_by_type, _)| *read_by_type == attribute.read_by_type)
        {
            Some((_, spelled)) => spelled.push(attribute.spelled.clone()),
            None => groups.push((attribute.read_by_type, vec![attribute.spelled.clone()])),
        }
    }
    for (read_by_type, spelled) in groups {
        let said = match read_by_type {
            None => (
                "is not one the reader knows, so its function is read as continuous".to_string(),
                "are not ones the reader knows, so their functions are read as continuous"
                    .to_string(),
            ),
            Some(kind) => {
                let kind = match kind {
                    GraphicalFunctionKind::Continuous => "continuous",
                    GraphicalFunctionKind::Extrapolate => "extrapolate",
                    GraphicalFunctionKind::Discrete => "discrete",
                };
                (
                    format!(
                        "is not one the reader acts on; its function is read as {kind}, as its \
                         type says"
                    ),
                    format!(
                        "are not ones the reader acts on; their functions are read as {kind}, \
                         as their types say"
                    ),
                )
            }
        };
        warnings.push(counted(
            spelled.len(),
            "graphical function type",
            "graphical function types",
            place,
            (&said.0, &said.1),
            &spelled,
        ));
    }
}

/// A view's type, from its `type` attribute; a view without one is a
/// stock-and-flow view.
fn view_kind(view: &BytesStart) -> ViewKind {
    let kind = view
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == b"type")
        .and_then(|attribute| value_of(&attribute).map(Cow::into_owned));
    match kind.as_deref() {
        None | Some("stock_flow") => ViewKind::StockFlow,
        Some("interface") => ViewKind::Interface,
        Some("popup") => ViewKind::Popup,
        Some(_) => ViewKind::Other,
    }
}

impl Capture {
    /// Takes one event inside the unread element; true once the element has
    /// ended.
    fn feed(&mut self, event: Event) -> bool {
        match event {
            Event::Start(element) => {
                let local = element.local_name();
                let tag = local.as_ref();
                let labeled = self.frames.len() <= LABEL_DEPTH
                    && matches!(
                        self.frames.last(),
                        Some(Frame::Root(_) | Frame::Describing(_))
                    );
                let frame = if !labeled {
                    Frame::Skipped
                } else if tag == b"entity" {
                    Frame::Entity(Label::from_attributes(&element))
                } else if DESCRIBING_CHILDREN.contains(&tag) {
                    Frame::Describing(Label::from_attributes(&element))
                } else {
                    Frame::Skipped
                };
                self.frames.push(frame);
                false
            }
            Event::End(_) => {
                if self.frames.len() == 1 {
                    return true;
                }
                let frame = self.frames.pop().expect("an open frame");
                if let Some(parent) = self.frames.last_mut().and_then(Frame::label) {
                    match frame {
                        Frame::Entity(entity) => {
                            if let Some(name) = entity.name {
                                keep_first(&mut parent.entity, &name);
                            }
                        }
                        Frame::Describing(child) => {
                            if let Some(called) = child.best() {
                                keep_first(&mut parent.child, &called);
                            }
                        }
                        Frame::Root(_) | Frame::Skipped => {}
                    }
                }
                false
            }
            Event::Text(text) => {
                self.push_text(text.decode().ok().as_deref());
                false
            }
            Event::CData(data) => {
                self.push_text(data.decode().ok().as_deref());
                false
            }
            Event::GeneralRef(reference) => {
                // A character reference or one of XML's five named entities;
                // any other reference says nothing a label can show.
                let resolved = match reference.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    Ok(None) => reference.decode().ok().and_then(|name| {
                        quick_xml::escape::resolve_predefined_entity(&name).map(str::to_owned)
                    }),
                    Err(_) => None,
                };
                self.push_text(resolved.as_deref());
                false
            }
            _ => false,
        }
    }

    /// Text belongs to the element it sits directly in, when that one is
    /// labeled; None is a piece that does not unescape.
    fn push_text(&mut self, piece: Option<&str>) {
        if let Some(label) = self.frames.last_mut().and_then(Frame::label) {
            label.text = match (label.text.take(), piece) {
                (Some(text), Some(piece)) => Some(text + piece),
                _ => None,
            };
        }
    }
}

/// The words for one element of `tag` and for several: the tag's own words
/// (`text_box`: `text box`, `text boxes`), except where they would not say
/// what it is. Among a model's variables, a group is Stella's list of the
/// variables that belong to it: the group's frame on the diagram is read,
/// and the list is not.
fn noun(tag: &str, among_variables: bool) -> (String, String) {
    let (one, many) = match tag {
        "group" if among_variables => ("group's member list", "groups' member lists"),
        // Stella's container for graphs and tables, shown one at a time.
        "stacked_container" => ("graph or table", "graphs and tables"),
        // A set of radio buttons.
        "options" => ("option group", "option groups"),
        "gf" => ("graphical function", "graphical functions"),
        "aux" => ("auxiliary", "auxiliaries"),
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
fn report_by_tag(
    place: &str,
    elements: &[Unread],
    among_variables: bool,
    warnings: &mut Vec<ImportWarning>,
) {
    // Each tag's labels, in first appearance, found by tag.
    let mut tags: Vec<(&str, Vec<String>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for element in elements {
        let at = *index.entry(&element.tag).or_insert_with(|| {
            tags.push((&element.tag, Vec::new()));
            tags.len() - 1
        });
        tags[at].1.push(element.label.clone().unwrap_or_default());
    }
    for (tag, labels) in tags {
        let (one, many) = noun(tag, among_variables);
        warnings.push(not_kept(labels.len(), &one, &many, place, &labels));
    }
}
