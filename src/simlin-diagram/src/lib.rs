// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A model's stock-and-flow diagram drawn: as SVG, as PNG, and as the scene
//! display list a host draws, with the exact geometry of each element.

#![forbid(unsafe_code)]

pub(crate) mod arrowhead;
pub mod common;
pub mod connector;
pub mod constants;
pub mod elements;
pub mod flow;
pub mod flow_geometry;
pub mod label;
mod path;
mod render;
#[cfg(feature = "png_render")]
mod render_png;
pub mod resolve;
pub mod scene;

pub use render::render_svg;
#[cfg(feature = "png_render")]
pub use render_png::{PngRenderOpts, render_png, svg_to_png};
pub use scene::{
    LabelPaint, SCENE_VERSION, Scene, SceneBounds, SceneCircle, SceneElement, SceneElementKind,
    SceneLabel, SceneLabelLine, ScenePaint, ScenePath, SceneRectangle, SceneShape, SceneTextAnchor,
    SparklineSlot, TextBaseline, build_scene,
};
