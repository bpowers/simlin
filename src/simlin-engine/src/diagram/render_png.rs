// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! SVG-to-PNG rendering using resvg.
//!
//! Converts a model's stock-and-flow diagram to a PNG image by first
//! rendering it to SVG (via [`super::render_svg`]) and then rasterizing
//! the SVG with resvg. The Roboto Light font is embedded into the binary
//! so that text renders identically across all platforms and environments.

use resvg::tiny_skia;
use resvg::usvg;

use crate::datamodel;

/// Roboto Light font data, embedded at compile time.
static ROBOTO_LIGHT: &[u8] = include_bytes!("fonts/Roboto-Light.ttf");

/// Options controlling PNG rendering output.
#[derive(Default)]
pub struct PngRenderOpts {
    /// Target width of the output image in pixels. The height is computed
    /// from the SVG's aspect ratio. If neither `width` nor `height` is set,
    /// the SVG's intrinsic dimensions are used at 1:1 scale.
    pub width: Option<u32>,
    /// Target height of the output image in pixels. The width is computed
    /// from the SVG's aspect ratio. When both `width` and `height` are
    /// set, `width` takes precedence.
    pub height: Option<u32>,
}

/// Renders the named model's diagram to a PNG image.
///
/// This is a convenience wrapper that calls [`super::render_svg`] and then
/// rasterizes the result with resvg. Returns the PNG file bytes.
pub fn render_png(
    project: &datamodel::Project,
    model_name: &str,
    opts: &PngRenderOpts,
) -> Result<Vec<u8>, String> {
    let svg_str = super::render_svg(project, model_name)?;
    svg_to_png(&svg_str, opts)
}

/// Rasterizes an SVG string to PNG bytes.
///
/// Exposed separately so callers that already have an SVG string (e.g.
/// from a different rendering path) can convert it directly.
pub fn svg_to_png(svg_str: &str, opts: &PngRenderOpts) -> Result<Vec<u8>, String> {
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_font_data(ROBOTO_LIGHT.to_vec());

    let usvg_opts = usvg::Options {
        font_family: "Roboto Light".to_string(),
        fontdb: std::sync::Arc::new(fontdb),
        ..usvg::Options::default()
    };

    let tree = usvg::Tree::from_str(svg_str, &usvg_opts)
        .map_err(|e| format!("failed to parse SVG: {e}"))?;

    let svg_size = tree.size();
    let svg_w = svg_size.width();
    let svg_h = svg_size.height();

    // Compute the target pixel dimensions while preserving aspect ratio.
    let (px_w, px_h) = match (opts.width, opts.height) {
        (Some(w), _) => {
            let scale = w as f32 / svg_w;
            (w, (svg_h * scale).ceil() as u32)
        }
        (None, Some(h)) => {
            let scale = h as f32 / svg_h;
            ((svg_w * scale).ceil() as u32, h)
        }
        (None, None) => (svg_w.ceil() as u32, svg_h.ceil() as u32),
    };

    if px_w == 0 || px_h == 0 {
        return Err("computed image dimensions are zero".to_string());
    }

    let scale_x = px_w as f32 / svg_w;
    let scale_y = px_h as f32 / svg_h;

    let mut pixmap = tiny_skia::Pixmap::new(px_w, px_h)
        .ok_or_else(|| "failed to allocate pixmap".to_string())?;

    // Fill with white background (SVG diagrams expect a white canvas).
    pixmap.fill(tiny_skia::Color::WHITE);

    let transform = tiny_skia::Transform::from_scale(scale_x, scale_y);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .map_err(|e| format!("failed to encode PNG: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::{self, FlowPoint, LabelSide, LinkShape};
    use crate::datamodel::{
        Aux as AuxVar, Equation, Flow as FlowVar, SimSpecs, Stock as StockVar, StockFlow, Variable,
        View,
    };

    fn make_simple_project(
        elements: Vec<datamodel::ViewElement>,
        variables: Vec<Variable>,
    ) -> datamodel::Project {
        datamodel::Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables,
                views: vec![View::StockFlow(StockFlow {
                    elements,
                    view_box: datamodel::Rect::default(),
                    zoom: 1.0,
                    use_lettered_polarity: false,
                })],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        }
    }

    fn make_aux_ve(name: &str, uid: i32, x: f64, y: f64) -> datamodel::ViewElement {
        datamodel::ViewElement::Aux(view_element::Aux {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
        })
    }

    fn make_stock_ve(name: &str, uid: i32, x: f64, y: f64) -> datamodel::ViewElement {
        datamodel::ViewElement::Stock(view_element::Stock {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
        })
    }

    fn make_cloud_ve(uid: i32, flow_uid: i32, x: f64, y: f64) -> datamodel::ViewElement {
        datamodel::ViewElement::Cloud(view_element::Cloud {
            uid,
            flow_uid,
            x,
            y,
        })
    }

    fn make_flow_ve(
        name: &str,
        uid: i32,
        x: f64,
        y: f64,
        points: Vec<(f64, f64, Option<i32>)>,
    ) -> datamodel::ViewElement {
        datamodel::ViewElement::Flow(view_element::Flow {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            points: points
                .into_iter()
                .map(|(px, py, attached)| FlowPoint {
                    x: px,
                    y: py,
                    attached_to_uid: attached,
                })
                .collect(),
        })
    }

    fn make_link_ve(uid: i32, from_uid: i32, to_uid: i32) -> datamodel::ViewElement {
        datamodel::ViewElement::Link(view_element::Link {
            uid,
            from_uid,
            to_uid,
            shape: LinkShape::Straight,
            polarity: None,
        })
    }

    fn make_scalar_aux_var(name: &str) -> Variable {
        Variable::Aux(AuxVar {
            ident: name.to_string(),
            equation: Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    }

    fn make_scalar_stock_var(name: &str) -> Variable {
        Variable::Stock(StockVar {
            ident: name.to_string(),
            equation: Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    }

    fn make_scalar_flow_var(name: &str) -> Variable {
        Variable::Flow(FlowVar {
            ident: name.to_string(),
            equation: Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
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

    // -- basic API tests -------------------------------------------------

    #[test]
    fn test_render_png_missing_model() {
        let project = make_simple_project(vec![], vec![]);
        let result = render_png(&project, "nonexistent", &PngRenderOpts::default());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_render_png_empty_model() {
        let project = make_simple_project(vec![], vec![]);
        let png = render_png(&project, "main", &PngRenderOpts::default())
            .expect("empty model should render");
        assert_valid_png(&png);
    }

    #[test]
    fn test_render_png_single_aux() {
        let elements = vec![make_aux_ve("growth_rate", 1, 100.0, 200.0)];
        let variables = vec![make_scalar_aux_var("growth_rate")];
        let project = make_simple_project(elements, variables);

        let png = render_png(&project, "main", &PngRenderOpts::default())
            .expect("single aux should render");
        assert_valid_png(&png);
        // PNG should be non-trivial in size (has content)
        assert!(png.len() > 100);
    }

    #[test]
    fn test_render_png_stock_flow_cloud() {
        let elements = vec![
            make_stock_ve("population", 1, 200.0, 100.0),
            make_cloud_ve(2, 3, 100.0, 100.0),
            make_cloud_ve(4, 3, 300.0, 100.0),
            make_flow_ve(
                "births",
                3,
                150.0,
                100.0,
                vec![(100.0, 100.0, Some(2)), (200.0, 100.0, Some(1))],
            ),
        ];
        let variables = vec![
            make_scalar_stock_var("population"),
            make_scalar_flow_var("births"),
        ];
        let project = make_simple_project(elements, variables);

        let png = render_png(&project, "main", &PngRenderOpts::default())
            .expect("stock-flow-cloud should render");
        assert_valid_png(&png);
    }

    #[test]
    fn test_render_png_with_connector() {
        let elements = vec![
            make_aux_ve("rate", 1, 100.0, 100.0),
            make_aux_ve("result", 2, 200.0, 100.0),
            make_link_ve(3, 1, 2),
        ];
        let variables = vec![make_scalar_aux_var("rate"), make_scalar_aux_var("result")];
        let project = make_simple_project(elements, variables);

        let png = render_png(&project, "main", &PngRenderOpts::default())
            .expect("connector should render");
        assert_valid_png(&png);
    }

    // -- scaling tests ---------------------------------------------------

    #[test]
    fn test_render_png_width_scaling() {
        let elements = vec![
            make_aux_ve("a", 1, 50.0, 50.0),
            make_aux_ve("b", 2, 300.0, 250.0),
        ];
        let variables = vec![make_scalar_aux_var("a"), make_scalar_aux_var("b")];
        let project = make_simple_project(elements, variables);

        let opts = PngRenderOpts {
            width: Some(800),
            height: None,
        };
        let png = render_png(&project, "main", &opts).expect("width scaling should work");
        assert_valid_png(&png);

        // Decode to verify dimensions
        let pixmap = tiny_skia::Pixmap::decode_png(&png).expect("PNG should decode");
        assert_eq!(pixmap.width(), 800);
        assert!(pixmap.height() > 0);
    }

    #[test]
    fn test_render_png_height_scaling() {
        let elements = vec![
            make_aux_ve("a", 1, 50.0, 50.0),
            make_aux_ve("b", 2, 300.0, 250.0),
        ];
        let variables = vec![make_scalar_aux_var("a"), make_scalar_aux_var("b")];
        let project = make_simple_project(elements, variables);

        let opts = PngRenderOpts {
            width: None,
            height: Some(600),
        };
        let png = render_png(&project, "main", &opts).expect("height scaling should work");
        assert_valid_png(&png);

        let pixmap = tiny_skia::Pixmap::decode_png(&png).expect("PNG should decode");
        assert_eq!(pixmap.height(), 600);
        assert!(pixmap.width() > 0);
    }

    #[test]
    fn test_render_png_width_takes_precedence() {
        let elements = vec![
            make_aux_ve("a", 1, 50.0, 50.0),
            make_aux_ve("b", 2, 300.0, 250.0),
        ];
        let variables = vec![make_scalar_aux_var("a"), make_scalar_aux_var("b")];
        let project = make_simple_project(elements, variables);

        let opts = PngRenderOpts {
            width: Some(400),
            height: Some(9999),
        };
        let png = render_png(&project, "main", &opts).expect("width precedence should work");
        assert_valid_png(&png);

        let pixmap = tiny_skia::Pixmap::decode_png(&png).expect("PNG should decode");
        assert_eq!(pixmap.width(), 400);
        // Height should be derived from aspect ratio, NOT 9999
        assert!(pixmap.height() < 9999);
    }

    // -- svg_to_png direct tests -----------------------------------------

    #[test]
    fn test_svg_to_png_minimal() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <rect width="100" height="100" fill="red"/>
        </svg>"#;
        let png = svg_to_png(svg, &PngRenderOpts::default()).expect("minimal SVG should render");
        assert_valid_png(&png);
    }

    #[test]
    fn test_svg_to_png_with_text() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50">
            <text x="100" y="25" style="font-family:'Roboto Light';font-size:12px;text-anchor:middle">Hello</text>
        </svg>"#;
        let png = svg_to_png(svg, &PngRenderOpts::default()).expect("text SVG should render");
        assert_valid_png(&png);
    }

    #[test]
    fn test_svg_to_png_invalid_svg() {
        let result = svg_to_png("not valid svg at all", &PngRenderOpts::default());
        assert!(result.is_err());
    }

    // -- integration test with real model file ---------------------------

    fn load_xmile_project(path: &str) -> datamodel::Project {
        let file = std::fs::File::open(path).unwrap_or_else(|e| {
            panic!("failed to open {}: {}", path, e);
        });
        let mut reader = std::io::BufReader::new(file);
        crate::compat::open_xmile(&mut reader).unwrap_or_else(|e| {
            panic!("failed to parse {}: {}", path, e);
        })
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
        let png = render_png(&project, "main", &PngRenderOpts::default())
            .expect("SIR should render to PNG");
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

    // -- white background test -------------------------------------------

    #[test]
    fn test_render_png_has_white_background() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <rect x="0" y="0" width="10" height="10" fill="white"/>
        </svg>"#;
        let png = svg_to_png(svg, &PngRenderOpts::default()).expect("white bg SVG should render");
        let pixmap = tiny_skia::Pixmap::decode_png(&png).expect("PNG should decode");

        // Check corner pixel is white (255, 255, 255, 255)
        let pixel = pixmap.pixels()[0];
        assert_eq!(pixel.red(), 255);
        assert_eq!(pixel.green(), 255);
        assert_eq!(pixel.blue(), 255);
        assert_eq!(pixel.alpha(), 255);
    }
}
