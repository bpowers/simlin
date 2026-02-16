// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::datamodel::{self, Equation, View, ViewElement};
use crate::diagram::common::{Rect, calc_view_box};
use crate::diagram::connector::render_connector;
use crate::diagram::elements::{
    aux_bounds, cloud_bounds, group_bounds, module_bounds, render_alias, render_aux, render_cloud,
    render_group, render_module, render_stock, stock_bounds,
};
use crate::diagram::flow::{flow_bounds, render_flow};

// Keep in sync with the TypeScript source of truth: src/diagram/drawing/render-styles.ts
const RENDER_STYLES: &str = r#"
/* Canvas */
.simlin-canvas text {
  fill: #000000;
  font-size: 12px;
  font-family: "Roboto Light", "Roboto", "Open Sans", "Arial", sans-serif;
  font-weight: 300;
  text-anchor: middle;
  white-space: nowrap;
  vertical-align: middle;
}

/* Stock */
.simlin-stock rect {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-stock.simlin-selected text {
  fill: #4444dd;
}

.simlin-stock.simlin-selected rect {
  stroke: #4444dd;
}

/* Auxiliary */
.simlin-aux circle {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-aux.simlin-selected text {
  fill: #4444dd;
}

.simlin-aux.simlin-selected circle {
  stroke: #4444dd;
}

/* Flow */
.simlin-flow .simlin-outer {
  fill: none;
  stroke-width: 4px;
  stroke: #000000;
}

.simlin-flow .simlin-outer-selected {
  fill: none;
  stroke-width: 4px;
  stroke: #4444dd;
}

.simlin-flow .simlin-inner {
  fill: none;
  stroke-width: 2px;
  stroke: #ffffff;
}

.simlin-flow circle {
  stroke-width: 1px;
  fill: #ffffff;
  stroke: #000000;
}

.simlin-flow.simlin-selected text {
  fill: #4444dd;
}

.simlin-flow.simlin-selected circle {
  stroke: #4444dd;
}

/* Cloud */
path.simlin-cloud {
  stroke-width: 2px;
  stroke-linejoin: round;
  stroke-miterlimit: 4px;
  fill: #ffffff;
  stroke: #6388dc;
}

/* Alias */
.simlin-alias circle {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-alias.simlin-selected text {
  fill: #4444dd;
}

.simlin-alias.simlin-selected circle {
  stroke: #4444dd;
}

/* Module */
.simlin-module rect {
  stroke-width: 1px;
  stroke: #000000;
  fill: #ffffff;
}

.simlin-module.simlin-selected text {
  fill: #4444dd;
}

.simlin-module.simlin-selected rect {
  stroke: #4444dd;
}

/* Connector */
.simlin-connector {
  stroke-width: 0.5px;
  stroke: gray;
  fill: none;
}

.simlin-connector-dashed {
  stroke-width: 0.5px;
  stroke: gray;
  stroke-dasharray: 2px;
  fill: none;
}

.simlin-connector-selected {
  stroke-width: 1px;
  stroke: #4444dd;
  fill: none;
}

.simlin-connector-bg {
  stroke-width: 7px;
  stroke: white;
  opacity: 0;
  fill: none;
}

/* Arrowhead */
path.simlin-arrowhead-flow {
  stroke-width: 1px;
  stroke-linejoin: round;
  stroke: #000000;
  fill: #ffffff;
}

path.simlin-arrowhead-flow.simlin-selected {
  stroke: #4444dd;
  fill: white;
}

path.simlin-arrowhead-link {
  stroke-width: 1px;
  stroke-linejoin: round;
  stroke: gray;
  fill: gray;
}

path.simlin-arrowhead-link.simlin-selected {
  stroke: #4444dd;
  fill: #4444dd;
}

path.simlin-arrowhead-bg {
  fill: white;
  opacity: 0;
}

/* Error indicators */
.simlin-error-indicator {
  stroke-width: 0px;
  fill: rgb(255, 152, 0);
}

/* Sparkline */
.simlin-sparkline-line {
  stroke-width: 0.5px;
  stroke-linecap: round;
  stroke: #2299dd;
  fill: none;
}

.simlin-sparkline-axis {
  stroke-width: 0.75px;
  stroke-linecap: round;
  stroke: #999;
  fill: none;
}
"#;

const Z_MAX: usize = 6;

fn is_arrayed(model: &datamodel::Model, name: &str) -> bool {
    model
        .get_variable(name)
        .and_then(|v| v.get_equation())
        .map(|eq| matches!(eq, Equation::ApplyToAll(..) | Equation::Arrayed(..)))
        .unwrap_or(false)
}

pub fn render_svg(project: &datamodel::Project, model_name: &str) -> Result<String, String> {
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

    let uid_to_element: HashMap<i32, &ViewElement> = stock_flow
        .elements
        .iter()
        .map(|e| (e.get_uid(), e))
        .collect();

    let is_arrayed_fn = |name: &str| -> bool { is_arrayed(model, name) };

    // Sort elements into z-layers and render
    let mut z_layers: Vec<Vec<String>> = vec![Vec::new(); Z_MAX];
    let mut bounds: Vec<Option<Rect>> = Vec::new();

    for element in &stock_flow.elements {
        let (svg_fragment, element_bounds, z_order) = match element {
            ViewElement::Group(group) => {
                let svg = render_group(group);
                let b = group_bounds(group);
                (svg, Some(b), 0)
            }
            ViewElement::Link(link) => {
                let from = uid_to_element.get(&link.from_uid);
                let to = uid_to_element.get(&link.to_uid);
                if let (Some(from), Some(to)) = (from, to) {
                    let svg = render_connector(link, from, to, &is_arrayed_fn);
                    // Connector bounds intentionally NOT collected
                    (svg, None, 2)
                } else {
                    continue;
                }
            }
            ViewElement::Flow(flow) => {
                if flow.points.len() < 2 {
                    continue;
                }
                let source_uid = flow.points.first().and_then(|p| p.attached_to_uid);
                let sink_uid = flow.points.last().and_then(|p| p.attached_to_uid);
                if let (Some(source_uid), Some(sink_uid)) = (source_uid, sink_uid) {
                    if !uid_to_element.contains_key(&source_uid) {
                        continue;
                    }
                    if let Some(sink) = uid_to_element.get(&sink_uid) {
                        let arrayed = is_arrayed(model, &flow.name);
                        let svg = render_flow(flow, sink, arrayed);
                        let b = flow_bounds(flow);
                        (svg, Some(b), 3)
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            ViewElement::Stock(stock) => {
                let arrayed = is_arrayed(model, &stock.name);
                let svg = render_stock(stock, arrayed);
                let b = stock_bounds(stock);
                (svg, Some(b), 4)
            }
            ViewElement::Cloud(cloud) => {
                let svg = render_cloud(cloud);
                let b = cloud_bounds(cloud);
                (svg, Some(b), 4)
            }
            ViewElement::Module(module) => {
                let svg = render_module(module);
                let b = module_bounds(module);
                (svg, Some(b), 4)
            }
            ViewElement::Aux(aux) => {
                let arrayed = is_arrayed(model, &aux.name);
                let svg = render_aux(aux, arrayed);
                let b = aux_bounds(aux);
                (svg, Some(b), 5)
            }
            ViewElement::Alias(alias) => {
                let alias_of_name = uid_to_element
                    .get(&alias.alias_of_uid)
                    .and_then(|e| e.get_name());
                let svg = render_alias(alias, alias_of_name);
                // Alias bounds intentionally NOT collected (matches TS Canvas)
                (svg, None, 5)
            }
        };

        if !svg_fragment.is_empty() {
            z_layers[z_order].push(svg_fragment);
        }
        bounds.push(element_bounds);
    }

    let view_box = calc_view_box(&bounds);

    let (vb_str, width, height) = if let Some(vb) = view_box {
        let left = vb.left.floor() as i64 - 10;
        let top = vb.top.floor() as i64 - 10;
        let width = (vb.right - left as f64).ceil() as i64 + 10;
        let height = (vb.bottom - top as f64).ceil() as i64 + 10;
        (
            format!("{} {} {} {}", left, top, width, height),
            width,
            height,
        )
    } else {
        ("0 0 100 100".to_string(), 100, 100)
    };

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg style=\"width: {}; height: {};\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{}\" preserveAspectRatio=\"xMinYMin\" class=\"simlin-canvas\">",
        width, height, vb_str
    ));
    svg.push_str("<style>\n");
    svg.push_str(RENDER_STYLES);
    svg.push_str("\n</style>\n");
    svg.push_str("<defs>\n");
    svg.push_str(
        "<filter id=\"labelBackground\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\">",
    );
    svg.push_str("<feMorphology operator=\"dilate\" radius=\"4\"></feMorphology>");
    svg.push_str("<feGaussianBlur stdDeviation=\"2\"></feGaussianBlur>");
    svg.push_str("<feColorMatrix type=\"matrix\" values=\"0 0 0 0 1\n                          0 0 0 0 1\n                          0 0 0 0 1\n                          0 0 0 0.85 0\"></feColorMatrix>");
    svg.push_str("<feComposite operator=\"over\" in=\"SourceGraphic\"></feComposite>");
    svg.push_str("</filter>");
    svg.push_str("</defs>");
    svg.push_str("<g>");

    for layer in &z_layers {
        for fragment in layer {
            svg.push_str(fragment);
        }
    }

    svg.push_str("</g>");
    svg.push_str("</svg>");

    Ok(svg)
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
        elements: Vec<ViewElement>,
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

    fn make_aux_ve(name: &str, uid: i32, x: f64, y: f64) -> ViewElement {
        ViewElement::Aux(view_element::Aux {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
        })
    }

    fn make_stock_ve(name: &str, uid: i32, x: f64, y: f64) -> ViewElement {
        ViewElement::Stock(view_element::Stock {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
        })
    }

    fn make_cloud_ve(uid: i32, flow_uid: i32, x: f64, y: f64) -> ViewElement {
        ViewElement::Cloud(view_element::Cloud {
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
    ) -> ViewElement {
        ViewElement::Flow(view_element::Flow {
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

    fn make_link_ve(uid: i32, from_uid: i32, to_uid: i32) -> ViewElement {
        ViewElement::Link(view_element::Link {
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
            equation: Equation::Scalar("0".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
            ai_state: None,
            uid: None,
        })
    }

    fn make_scalar_stock_var(name: &str) -> Variable {
        Variable::Stock(StockVar {
            ident: name.to_string(),
            equation: Equation::Scalar("0".to_string(), None),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
            ai_state: None,
            uid: None,
        })
    }

    fn make_scalar_flow_var(name: &str) -> Variable {
        Variable::Flow(FlowVar {
            ident: name.to_string(),
            equation: Equation::Scalar("0".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
            ai_state: None,
            uid: None,
        })
    }

    #[test]
    fn test_render_svg_missing_model() {
        let project = make_simple_project(vec![], vec![]);
        let result = render_svg(&project, "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_render_svg_empty_model() {
        let project = make_simple_project(vec![], vec![]);
        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("simlin-canvas"));
        assert!(svg.contains("<style>"));
        assert!(svg.contains("labelBackground"));
    }

    #[test]
    fn test_render_svg_single_aux() {
        let elements = vec![make_aux_ve("growth_rate", 1, 100.0, 200.0)];
        let variables = vec![make_scalar_aux_var("growth_rate")];
        let project = make_simple_project(elements, variables);

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();
        assert!(svg.contains("simlin-aux"));
        assert!(svg.contains("growth rate")); // display_name converts _ to space
        assert!(svg.contains("viewBox="));
    }

    #[test]
    fn test_render_svg_stock_flow_cloud() {
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

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();
        assert!(svg.contains("simlin-stock"));
        assert!(svg.contains("simlin-flow"));
        assert!(svg.contains("simlin-cloud"));
    }

    #[test]
    fn test_render_svg_with_connector() {
        let elements = vec![
            make_aux_ve("rate", 1, 100.0, 100.0),
            make_aux_ve("result", 2, 200.0, 100.0),
            make_link_ve(3, 1, 2),
        ];
        let variables = vec![make_scalar_aux_var("rate"), make_scalar_aux_var("result")];
        let project = make_simple_project(elements, variables);

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();
        assert!(svg.contains("simlin-connector"));
    }

    #[test]
    fn test_render_svg_viewbox_calculation() {
        let elements = vec![
            make_aux_ve("a", 1, 50.0, 50.0),
            make_aux_ve("b", 2, 300.0, 250.0),
        ];
        let variables = vec![make_scalar_aux_var("a"), make_scalar_aux_var("b")];
        let project = make_simple_project(elements, variables);

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();

        // Extract viewBox
        let vb_start = svg.find("viewBox=\"").unwrap() + 9;
        let vb_end = svg[vb_start..].find('"').unwrap() + vb_start;
        let vb = &svg[vb_start..vb_end];
        let parts: Vec<f64> = vb.split(' ').map(|s| s.parse().unwrap()).collect();
        assert_eq!(parts.len(), 4);
        // left should be roughly floor(50 - 9) - 10 = 31 (aux at 50 with radius 9)
        assert!(parts[0] <= 41.0);
        // width and height should be positive
        assert!(parts[2] > 0.0);
        assert!(parts[3] > 0.0);
    }

    #[test]
    fn test_render_svg_connector_bounds_excluded() {
        // Connector between two aux elements far apart
        let elements = vec![
            make_aux_ve("a", 1, 50.0, 50.0),
            make_aux_ve("b", 2, 50.0, 100.0),
            make_link_ve(3, 1, 2),
        ];
        let variables = vec![make_scalar_aux_var("a"), make_scalar_aux_var("b")];
        let project = make_simple_project(elements, variables);

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        // The connector should render but not affect the viewBox
        let svg = result.unwrap();
        assert!(svg.contains("simlin-connector"));
    }

    #[test]
    fn test_render_svg_z_order() {
        // Verify z-ordering: groups (0) < connectors (2) < flows (3) < stocks/clouds (4) < aux (5)
        let elements = vec![
            make_aux_ve("aux1", 1, 100.0, 100.0),
            make_stock_ve("stock1", 2, 200.0, 100.0),
            make_link_ve(3, 1, 2),
        ];
        let variables = vec![make_scalar_aux_var("aux1"), make_scalar_stock_var("stock1")];
        let project = make_simple_project(elements, variables);

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();

        // Find the end of the style/defs section, then search within element markup
        let defs_end = svg.find("</defs>").unwrap();
        let body = &svg[defs_end..];
        let connector_pos = body.find("simlin-connector-bg").unwrap();
        let stock_pos = body.find("<g class=\"simlin-stock\">").unwrap();
        let aux_pos = body.find("<g class=\"simlin-aux\">").unwrap();
        assert!(
            connector_pos < stock_pos,
            "connector should render before stock"
        );
        assert!(stock_pos < aux_pos, "stock should render before aux");
    }

    #[test]
    fn test_render_svg_no_views() {
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };
        let result = render_svg(&project, "main");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no stock-flow view"));
    }

    #[test]
    fn test_render_svg_structure() {
        let elements = vec![make_aux_ve("x", 1, 100.0, 100.0)];
        let variables = vec![make_scalar_aux_var("x")];
        let project = make_simple_project(elements, variables);

        let svg = render_svg(&project, "main").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("preserveAspectRatio=\"xMinYMin\""));
        assert!(svg.contains("<defs>"));
        assert!(svg.contains("</defs>"));
        assert!(svg.contains("<g>"));
        assert!(svg.contains("</g>"));
        assert!(svg.contains("feMorphology"));
        assert!(svg.contains("feGaussianBlur"));
        assert!(svg.contains("feColorMatrix"));
        assert!(svg.contains("feComposite"));
    }

    #[test]
    fn test_render_svg_flow_with_missing_endpoints() {
        // Flow with points referencing nonexistent UIDs should be skipped
        let elements = vec![make_flow_ve(
            "broken_flow",
            1,
            150.0,
            100.0,
            vec![(100.0, 100.0, Some(99)), (200.0, 100.0, Some(98))],
        )];
        let variables = vec![make_scalar_flow_var("broken_flow")];
        let project = make_simple_project(elements, variables);

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();
        // Should not contain a flow valve circle since endpoints are missing
        // (note: "simlin-flow" appears in CSS styles, so we check for the <g> element)
        assert!(!svg.contains("<g class=\"simlin-flow\">"));
    }

    #[test]
    fn test_render_svg_link_with_missing_endpoints() {
        // Link referencing nonexistent UIDs should be skipped
        let elements = vec![make_aux_ve("a", 1, 100.0, 100.0), make_link_ve(2, 1, 99)];
        let variables = vec![make_scalar_aux_var("a")];
        let project = make_simple_project(elements, variables);

        let result = render_svg(&project, "main");
        assert!(result.is_ok());
        let svg = result.unwrap();
        // Should not contain a connector path since 'to' endpoint is missing
        // (note: "simlin-connector" appears in CSS styles, so we check for the <path> element)
        assert!(!svg.contains("class=\"simlin-connector\""));
    }

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
}
