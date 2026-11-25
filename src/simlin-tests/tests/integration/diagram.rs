// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Corpus models drawn: as SVG, as PNG and as a scene, the scene checked against
//! the SVG.

use resvg::tiny_skia;
use simlin_base::datamodel;
use simlin_diagram::scene::test_support::{
    assert_idents_name_the_displayed_variable, assert_scene_draws_the_svg,
};
use simlin_diagram::{PngRenderOpts, build_scene, render_png, render_svg};

fn load_xmile_project(path: &str) -> datamodel::Project {
    let file = std::fs::File::open(path).unwrap_or_else(|e| {
        panic!("failed to open {}: {}", path, e);
    });
    let mut reader = std::io::BufReader::new(file);
    simlin_formats::open_xmile(&mut reader).unwrap_or_else(|e| {
        panic!("failed to parse {}: {}", path, e);
    })
}

fn open_corpus_model(relative: &str) -> datamodel::Project {
    load_xmile_project(&format!(
        "{}/../../test/{relative}",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Validates that `data` begins with a valid PNG signature.
fn assert_valid_png(data: &[u8]) {
    assert!(data.len() > 8, "PNG data too short");
    assert_eq!(
        &data[0..8],
        &[137, 80, 78, 71, 13, 10, 26, 10],
        "missing PNG header signature"
    );
}

#[test]
fn the_scene_draws_what_the_svg_draws_for_corpus_models() {
    for model in [
        "test-models/samples/teacup/teacup_w_diagram.xmile",
        "test-models/samples/SIR/SIR.xmile",
        "alias1/alias1.stmx",
        "test-models/samples/bpowers-hares_and_lynxes_modules/model.stmx",
    ] {
        let project = open_corpus_model(model);
        let scene = build_scene(&project, "main").expect("the scene builds");
        let svg = render_svg(&project, "main").expect("the SVG renders");
        assert!(!scene.elements.is_empty(), "{model} draws something");
        assert_scene_draws_the_svg(&scene, &svg, model);
        assert_idents_name_the_displayed_variable(&scene, &project);
    }
}

#[test]
fn test_render_teacup_model() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/teacup/teacup_w_diagram.xmile"
    ));
    let svg = render_svg(&project, "main").unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.ends_with("</svg>"));
    // Teacup has stocks, flows, aux, connectors
    let body = &svg[svg.find("</defs>").unwrap()..];
    assert!(body.contains("<g class=\"simlin-stock\">"));
    assert!(body.contains("<g class=\"simlin-flow\">"));
    assert!(body.contains("<g class=\"simlin-aux\">"));
    assert!(body.contains("simlin-connector"));
}

#[test]
fn test_render_sir_model() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/SIR/SIR.xmile"
    ));
    let svg = render_svg(&project, "main").unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.ends_with("</svg>"));
    let body = &svg[svg.find("</defs>").unwrap()..];
    assert!(body.contains("<g class=\"simlin-stock\">"));
    assert!(body.contains("<g class=\"simlin-flow\">"));
}

#[test]
fn test_render_alias_model() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/alias1/alias1.stmx"
    ));
    let svg = render_svg(&project, "main").unwrap();
    assert!(svg.starts_with("<svg"));
    let body = &svg[svg.find("</defs>").unwrap()..];
    assert!(body.contains("<g class=\"simlin-alias\">"));
}

#[test]
fn test_render_modules_model() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/bpowers-hares_and_lynxes_modules/model.stmx"
    ));
    let svg = render_svg(&project, "main").unwrap();
    assert!(svg.starts_with("<svg"));
    let body = &svg[svg.find("</defs>").unwrap()..];
    assert!(body.contains("<g class=\"simlin-module\">"));
}

#[test]
fn test_render_teacup_png() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/teacup/teacup_w_diagram.xmile"
    ));
    let png = render_png(&project, "main", &PngRenderOpts::default())
        .expect("teacup should render to PNG");
    assert_valid_png(&png);
    // Should be a meaningful image, not tiny
    assert!(png.len() > 500);
}

#[test]
fn test_render_teacup_png_scaled() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/teacup/teacup_w_diagram.xmile"
    ));
    let opts = PngRenderOpts {
        width: Some(800),
        height: None,
    };
    let png = render_png(&project, "main", &opts).expect("scaled teacup should render");
    assert_valid_png(&png);

    let pixmap = tiny_skia::Pixmap::decode_png(&png).expect("PNG should decode");
    assert_eq!(pixmap.width(), 800);
}

#[test]
fn test_render_sir_png() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/SIR/SIR.xmile"
    ));
    let png =
        render_png(&project, "main", &PngRenderOpts::default()).expect("SIR should render to PNG");
    assert_valid_png(&png);
}

#[test]
fn test_render_modules_png() {
    let project = load_xmile_project(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/test-models/samples/bpowers-hares_and_lynxes_modules/model.stmx"
    ));
    let png = render_png(&project, "main", &PngRenderOpts::default())
        .expect("modules model should render to PNG");
    assert_valid_png(&png);
}
