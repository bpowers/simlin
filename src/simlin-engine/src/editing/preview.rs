// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a frame draws: scene elements for everything a plan changed, and the
//! base elements they stand in for.
//!
//! A host draws the base view's scene once per edit and, while a gesture is
//! live, hides a frame's `hidden` elements and draws its `elements` on top, so a
//! frame costs in proportion to what the gesture touches. The elements come from
//! the scene's own per-element builder over the base view with the plan's
//! changes substituted, so a preview draws exactly what the committed view will.

use std::collections::{HashMap, HashSet};

use crate::datamodel::ViewElement;
use crate::diagram::common::Point as DiagramPoint;
use crate::diagram::resolve::resolve_element;
use crate::diagram::scene::{SceneElement, dangling_link_scene_element, scene_element};

use super::base::BaseView;
use super::gesture::Plan;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct Preview {
    /// Base elements the frame does not draw as they were: substituted,
    /// removed, and links redrawn because an endpoint moved.
    pub hidden: Vec<i32>,
    /// What the frame draws in their place, and what it adds, in draw order.
    pub elements: Vec<SceneElement>,
}

/// The scene elements `plan` draws over `base`'s scene.
pub fn preview(base: &BaseView, plan: &Plan) -> Preview {
    let changed: HashMap<i32, &ViewElement> =
        plan.changed.iter().map(|e| (e.get_uid(), e)).collect();
    let removed: HashSet<i32> = plan.removed.iter().copied().collect();
    let lookup = |uid: i32| -> Option<&ViewElement> {
        if removed.contains(&uid) {
            return None;
        }
        changed.get(&uid).copied().or_else(|| base.get(uid))
    };
    let is_arrayed = |name: &str| base.is_arrayed(name);

    // Every changed element redraws, and so does every link touching one: its
    // line follows a moved end whether or not its shape changed.
    let mut redraw: Vec<i32> = Vec::with_capacity(plan.changed.len());
    let mut seen: HashSet<i32> = HashSet::with_capacity(plan.changed.len());
    for element in &plan.changed {
        let uid = element.get_uid();
        if seen.insert(uid) {
            redraw.push(uid);
        }
    }
    for element in &plan.changed {
        for link in base.touching_links(element.get_uid()) {
            if !removed.contains(&link.uid) && seen.insert(link.uid) {
                redraw.push(link.uid);
            }
        }
    }

    let mut hidden: Vec<i32> = redraw
        .iter()
        .copied()
        .filter(|&uid| base.get(uid).is_some())
        .collect();
    hidden.extend(
        plan.removed
            .iter()
            .copied()
            .filter(|&uid| base.get(uid).is_some()),
    );

    let mut elements: Vec<SceneElement> = Vec::with_capacity(redraw.len() + 1);
    for &uid in &redraw {
        let Some(element) = lookup(uid) else {
            continue;
        };
        if let Some(resolved) = resolve_element(element, &lookup, &is_arrayed) {
            elements.extend(scene_element(&resolved, &is_arrayed));
        }
    }
    if let Some(dangling) = &plan.dangling_link
        && let Some(from) = lookup(dangling.from)
    {
        let to = DiagramPoint {
            x: dangling.to.x,
            y: dangling.to.y,
        };
        elements.extend(dangling_link_scene_element(
            dangling.uid,
            from,
            to,
            &is_arrayed,
        ));
    }
    // A stable sort: within a layer, changed elements keep view order and
    // additions follow.
    elements.sort_by_key(|e| e.layer);
    Preview { hidden, elements }
}
