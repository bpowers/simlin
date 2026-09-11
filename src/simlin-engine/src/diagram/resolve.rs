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

use std::collections::HashMap;

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

impl ResolvedElement<'_> {
    /// The layer the element draws in: groups beneath connectors, connectors
    /// beneath flows, flows beneath stocks, clouds and modules, and auxes and
    /// aliases on top.
    pub(crate) fn layer(&self) -> u8 {
        match self {
            ResolvedElement::Group(_) => 0,
            ResolvedElement::Link { .. } => 2,
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
}

impl ResolvedView<'_> {
    /// Whether the named variable has an apply-to-all or arrayed equation, and
    /// so draws stacked copies.
    pub(crate) fn is_arrayed(&self, name: &str) -> bool {
        is_arrayed(self.model, name)
    }
}

fn is_arrayed(model: &datamodel::Model, name: &str) -> bool {
    model
        .get_variable(name)
        .and_then(|v| v.get_equation())
        .map(|eq| matches!(eq, Equation::ApplyToAll(..) | Equation::Arrayed(..)))
        .unwrap_or(false)
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

    let mut elements: Vec<ResolvedElement<'a>> = Vec::with_capacity(stock_flow.elements.len());
    for element in &stock_flow.elements {
        let resolved = match element {
            ViewElement::Group(group) => ResolvedElement::Group(group),
            ViewElement::Link(link) => {
                let (Some(from), Some(to)) = (lookup(link.from_uid), lookup(link.to_uid)) else {
                    continue;
                };
                ResolvedElement::Link { link, from, to }
            }
            ViewElement::Flow(flow) => {
                if flow.points.len() < 2 {
                    continue;
                }
                let source_uid = flow.points.first().and_then(|p| p.attached_to_uid);
                let sink_uid = flow.points.last().and_then(|p| p.attached_to_uid);
                let (Some(source_uid), Some(sink_uid)) = (source_uid, sink_uid) else {
                    continue;
                };
                if lookup(source_uid).is_none() {
                    continue;
                }
                let Some(sink) = lookup(sink_uid) else {
                    continue;
                };
                ResolvedElement::Flow {
                    flow,
                    sink,
                    is_arrayed: is_arrayed(model, &flow.name),
                }
            }
            ViewElement::Stock(stock) => ResolvedElement::Stock {
                stock,
                is_arrayed: is_arrayed(model, &stock.name),
            },
            ViewElement::Cloud(cloud) => ResolvedElement::Cloud(cloud),
            ViewElement::Module(module) => ResolvedElement::Module(module),
            ViewElement::Aux(aux) => ResolvedElement::Aux {
                aux,
                is_arrayed: is_arrayed(model, &aux.name),
            },
            ViewElement::Alias(alias) => ResolvedElement::Alias {
                alias,
                alias_of_name: lookup(alias.alias_of_uid).and_then(|e| e.get_name()),
            },
        };
        elements.push(resolved);
    }

    let content_bounds = calc_view_box(
        &elements
            .iter()
            .map(ResolvedElement::content_bounds)
            .collect::<Vec<_>>(),
    );
    // A stable sort, so elements within a layer keep their view order.
    elements.sort_by_key(ResolvedElement::layer);

    Ok(ResolvedView {
        model,
        elements,
        content_bounds,
    })
}
