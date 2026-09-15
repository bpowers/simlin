// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Which of a view's elements a diagram draws, in what order, and with which
//! neighbours.
//!
//! This is the one statement of those decisions. `render_svg` serializes the
//! resolved elements as SVG and `build_scene` as a display list, so the two
//! can never disagree about whether a flow with a dangling endpoint is drawn,
//! which layer a module sits in, or what folds into the fit-to-content box.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::common::canonicalize;
use crate::datamodel::{self, Equation, View, ViewElement, view_element};
use crate::diagram::common::{Rect, calc_view_box};
use crate::diagram::elements::{
    alias_bounds, aux_bounds, cloud_bounds, group_bounds, module_bounds, stock_bounds,
};
use crate::diagram::flow::flow_bounds;

/// One element a diagram draws, with the neighbours its drawing needs.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(crate) enum ResolvedElement<'a> {
    Group(&'a view_element::Group),
    /// A connector whose endpoints are both in the view.
    Link {
        link: &'a view_element::Link,
        from: &'a ViewElement,
        to: &'a ViewElement,
    },
    /// A flow with at least two points whose source and sink are both in the
    /// view.
    Flow {
        flow: &'a view_element::Flow,
        sink: &'a ViewElement,
        is_arrayed: bool,
    },
    Stock {
        stock: &'a view_element::Stock,
        is_arrayed: bool,
    },
    Cloud(&'a view_element::Cloud),
    Module(&'a view_element::Module),
    Aux {
        aux: &'a view_element::Aux,
        is_arrayed: bool,
    },
    /// An alias, with the name of the element it points at, `None` when
    /// that element is missing from the view.
    Alias {
        alias: &'a view_element::Alias,
        alias_of_name: Option<&'a str>,
    },
}

/// The layer connectors draw in (see [`ResolvedElement::layer`]).
pub(crate) const LINK_LAYER: u8 = 2;

impl ResolvedElement<'_> {
    /// The layer the element draws in: groups beneath connectors, connectors
    /// beneath flows, flows beneath stocks, clouds and modules, and auxes and
    /// aliases on top.
    pub(crate) fn layer(&self) -> u8 {
        match self {
            ResolvedElement::Group(_) => 0,
            ResolvedElement::Link { .. } => LINK_LAYER,
            ResolvedElement::Flow { .. } => 3,
            ResolvedElement::Stock { .. }
            | ResolvedElement::Cloud(_)
            | ResolvedElement::Module(_) => 4,
            ResolvedElement::Aux { .. } | ResolvedElement::Alias { .. } => 5,
        }
    }

    /// The box this element folds into the diagram's fit-to-content bounds.
    /// Connectors fold nothing, matching the web canvas.
    fn content_bounds(&self) -> Option<Rect> {
        match self {
            ResolvedElement::Group(group) => Some(group_bounds(group)),
            ResolvedElement::Link { .. } => None,
            ResolvedElement::Alias {
                alias,
                alias_of_name,
            } => Some(alias_bounds(alias, *alias_of_name)),
            ResolvedElement::Flow { flow, .. } => Some(flow_bounds(flow)),
            ResolvedElement::Stock { stock, .. } => Some(stock_bounds(stock)),
            ResolvedElement::Cloud(cloud) => Some(cloud_bounds(cloud)),
            ResolvedElement::Module(module) => Some(module_bounds(module)),
            ResolvedElement::Aux { aux, .. } => Some(aux_bounds(aux)),
        }
    }
}

/// A model's first stock-and-flow view, resolved for drawing.
pub(crate) struct ResolvedView<'a> {
    pub model: &'a datamodel::Model,
    /// The drawn elements in draw order: ascending layer, and view order
    /// within a layer.
    pub elements: Vec<ResolvedElement<'a>>,
    /// The union of the elements' fit-to-content boxes (the SVG viewBox before
    /// its padding), `None` when no element contributes one.
    pub content_bounds: Option<Rect>,
    arrayed: Arrayed<'a>,
}

impl ResolvedView<'_> {
    /// Whether the named variable has an apply-to-all or arrayed equation, and
    /// so draws stacked copies.
    pub(crate) fn is_arrayed(&self, name: &str) -> bool {
        self.arrayed.is_arrayed(name)
    }
}

/// Which of a model's variables have an apply-to-all or arrayed equation, by
/// canonical ident, read once per view. Drawing asks per element, and several
/// times per link, so an answer must not scan the model's variables: on a view
/// the size of World3 that scan was nearly all of drawing the view.
struct Arrayed<'a>(HashMap<Cow<'a, str>, bool>);

impl<'a> Arrayed<'a> {
    fn of(model: &'a datamodel::Model) -> Arrayed<'a> {
        let mut by_ident = HashMap::with_capacity(model.variables.len());
        for variable in &model.variables {
            // The first variable of an ident decides, the one
            // `Model::get_variable` finds.
            by_ident
                .entry(canonicalize(variable.get_ident()))
                .or_insert_with(|| {
                    matches!(
                        variable.get_equation(),
                        Some(Equation::ApplyToAll(..) | Equation::Arrayed(..))
                    )
                });
        }
        Arrayed(by_ident)
    }

    fn is_arrayed(&self, name: &str) -> bool {
        self.0
            .get(canonicalize(name).as_ref())
            .copied()
            .unwrap_or(false)
    }
}

/// Resolves `model_name`'s first stock-and-flow view.
///
/// An element whose drawing needs a neighbour the view does not hold -- a
/// link with a missing endpoint, a flow with a missing source or sink or fewer
/// than two points -- is not drawn and so does not appear.
pub(crate) fn resolve_view<'a>(
    project: &'a datamodel::Project,
    model_name: &str,
) -> Result<ResolvedView<'a>, String> {
    let model = project
        .get_model(model_name)
        .ok_or_else(|| format!("model '{}' not found", model_name))?;

    let stock_flow = model
        .views
        .first()
        .map(|v| match v {
            View::StockFlow(sf) => sf,
        })
        .ok_or_else(|| "no stock-flow view found".to_string())?;

    let uid_to_element: HashMap<i32, &'a ViewElement> = stock_flow
        .elements
        .iter()
        .map(|e| (e.get_uid(), e))
        .collect();
    let lookup = |uid: i32| -> Option<&'a ViewElement> { uid_to_element.get(&uid).copied() };

    let arrayed = Arrayed::of(model);
    let is_arrayed = |name: &str| arrayed.is_arrayed(name);
    let elements: Vec<ResolvedElement<'a>> = stock_flow
        .elements
        .iter()
        .filter_map(|element| resolve_element(element, &lookup, &is_arrayed))
        .collect();

    let content_bounds = calc_view_box(
        &elements
            .iter()
            .map(ResolvedElement::content_bounds)
            .collect::<Vec<_>>(),
    );
    let mut elements = elements;
    // A stable sort, so elements within a layer keep their view order.
    elements.sort_by_key(ResolvedElement::layer);

    Ok(ResolvedView {
        model,
        elements,
        content_bounds,
        arrayed,
    })
}

/// One element as a diagram draws it, with the neighbours its drawing needs
/// found through `lookup`: `None` for an element that is not drawn -- a link or
/// flow whose endpoints `lookup` does not hold, a flow with fewer than two
/// points. `resolve_view` resolves a whole view through this, and a gesture
/// preview resolves the elements a frame changed over the base view with the
/// frame's changes substituted, so the two can never disagree about how an
/// element draws.
pub(crate) fn resolve_element<'a>(
    element: &'a ViewElement,
    lookup: &dyn Fn(i32) -> Option<&'a ViewElement>,
    is_arrayed: &dyn Fn(&str) -> bool,
) -> Option<ResolvedElement<'a>> {
    Some(match element {
        ViewElement::Group(group) => ResolvedElement::Group(group),
        ViewElement::Link(link) => ResolvedElement::Link {
            link,
            from: lookup(link.from_uid)?,
            to: lookup(link.to_uid)?,
        },
        ViewElement::Flow(flow) => {
            if flow.points.len() < 2 {
                return None;
            }
            let source_uid = flow.points.first().and_then(|p| p.attached_to_uid)?;
            let sink_uid = flow.points.last().and_then(|p| p.attached_to_uid)?;
            lookup(source_uid)?;
            ResolvedElement::Flow {
                flow,
                sink: lookup(sink_uid)?,
                is_arrayed: is_arrayed(&flow.name),
            }
        }
        ViewElement::Stock(stock) => ResolvedElement::Stock {
            stock,
            is_arrayed: is_arrayed(&stock.name),
        },
        ViewElement::Cloud(cloud) => ResolvedElement::Cloud(cloud),
        ViewElement::Module(module) => ResolvedElement::Module(module),
        ViewElement::Aux(aux) => ResolvedElement::Aux {
            aux,
            is_arrayed: is_arrayed(&aux.name),
        },
        ViewElement::Alias(alias) => ResolvedElement::Alias {
            alias,
            alias_of_name: lookup(alias.alias_of_uid).and_then(|e| e.get_name()),
        },
    })
}
