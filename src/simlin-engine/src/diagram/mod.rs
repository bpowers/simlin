// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod arrowhead;
pub mod common;
pub(crate) mod connector;
pub mod constants;
pub(crate) mod elements;
pub(crate) mod flow;
pub(crate) mod flow_geometry;
pub(crate) mod label;
mod path;
mod render;
#[cfg(feature = "png_render")]
mod render_png;
pub(crate) mod resolve;
pub(crate) mod scene;

pub use render::render_svg;
#[cfg(feature = "png_render")]
pub use render_png::{PngRenderOpts, render_png, svg_to_png};
pub use scene::{
    LabelPaint, SCENE_VERSION, Scene, SceneBounds, SceneCircle, SceneElement, SceneElementKind,
    SceneLabel, SceneLabelLine, ScenePaint, ScenePath, SceneRectangle, SceneShape, SceneTextAnchor,
    SparklineSlot, TextBaseline, build_scene,
};
