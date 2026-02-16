// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::Path;

use sha2::{Digest, Sha256};
use simlin_engine::datamodel::{
    self, Dt, Equation, GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale,
    LoopMetadata, Model, ModelGroup, Rect, SimMethod, SimSpecs, StockFlow, View, ViewElement,
    Visibility,
};
use simlin_engine::open_xmile;

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn gen_option_string(opt: &Option<String>) -> String {
    match opt {
        Some(s) => format!("Some(\"{}\".to_string())", escape_string(s)),
        None => "None".to_string(),
    }
}

fn gen_vec_string(v: &[String]) -> String {
    if v.is_empty() {
        "vec![]".to_string()
    } else {
        let items: Vec<String> = v
            .iter()
            .map(|s| format!("\"{}\".to_string()", escape_string(s)))
            .collect();
        format!("vec![{}]", items.join(", "))
    }
}

fn gen_visibility(v: Visibility) -> &'static str {
    match v {
        Visibility::Private => "Visibility::Private",
        Visibility::Public => "Visibility::Public",
    }
}

fn gen_graphical_function_kind(kind: GraphicalFunctionKind) -> &'static str {
    match kind {
        GraphicalFunctionKind::Continuous => "GraphicalFunctionKind::Continuous",
        GraphicalFunctionKind::Extrapolate => "GraphicalFunctionKind::Extrapolate",
        GraphicalFunctionKind::Discrete => "GraphicalFunctionKind::Discrete",
    }
}

fn gen_graphical_function_scale(scale: &GraphicalFunctionScale) -> String {
    format!(
        "GraphicalFunctionScale {{ min: {}_f64, max: {}_f64 }}",
        scale.min, scale.max
    )
}

fn gen_vec_f64(v: &[f64]) -> String {
    if v.is_empty() {
        "vec![]".to_string()
    } else {
        let items: Vec<String> = v.iter().map(|f| format!("{}_f64", f)).collect();
        format!("vec![{}]", items.join(", "))
    }
}

fn gen_option_vec_f64(opt: &Option<Vec<f64>>) -> String {
    match opt {
        Some(v) => format!("Some({})", gen_vec_f64(v)),
        None => "None".to_string(),
    }
}

fn gen_graphical_function(gf: &GraphicalFunction) -> String {
    format!(
        "GraphicalFunction {{
            kind: {},
            x_points: {},
            y_points: {},
            x_scale: {},
            y_scale: {},
        }}",
        gen_graphical_function_kind(gf.kind),
        gen_option_vec_f64(&gf.x_points),
        gen_vec_f64(&gf.y_points),
        gen_graphical_function_scale(&gf.x_scale),
        gen_graphical_function_scale(&gf.y_scale)
    )
}

fn gen_option_graphical_function(opt: &Option<GraphicalFunction>) -> String {
    match opt {
        Some(gf) => format!("Some({})", gen_graphical_function(gf)),
        None => "None".to_string(),
    }
}

fn gen_equation(eq: &Equation) -> String {
    match eq {
        Equation::Scalar(s, units) => {
            format!(
                "Equation::Scalar(\"{}\".to_string(), {})",
                escape_string(s),
                gen_option_string(units)
            )
        }
        Equation::ApplyToAll(dims, s, units) => {
            format!(
                "Equation::ApplyToAll({}, \"{}\".to_string(), {})",
                gen_vec_string(dims),
                escape_string(s),
                gen_option_string(units)
            )
        }
        Equation::Arrayed(dims, elements) => {
            let elems: Vec<String> = elements
                .iter()
                .map(|(name, eqn, units, gf)| {
                    format!(
                        "(\"{}\".to_string(), \"{}\".to_string(), {}, {})",
                        escape_string(name),
                        escape_string(eqn),
                        gen_option_string(units),
                        gen_option_graphical_function(gf)
                    )
                })
                .collect();
            format!(
                "Equation::Arrayed({}, vec![{}])",
                gen_vec_string(dims),
                elems.join(", ")
            )
        }
    }
}

fn gen_stock(stock: &datamodel::Stock) -> String {
    format!(
        "Variable::Stock(Stock {{
            ident: \"{}\".to_string(),
            equation: {},
            documentation: \"{}\".to_string(),
            units: {},
            inflows: {},
            outflows: {},
            non_negative: {},
            can_be_module_input: {},
            visibility: {},
            ai_state: None,
            uid: {:?},
        }})",
        escape_string(&stock.ident),
        gen_equation(&stock.equation),
        escape_string(&stock.documentation),
        gen_option_string(&stock.units),
        gen_vec_string(&stock.inflows),
        gen_vec_string(&stock.outflows),
        stock.non_negative,
        stock.can_be_module_input,
        gen_visibility(stock.visibility),
        stock.uid
    )
}

fn gen_flow(flow: &datamodel::Flow) -> String {
    format!(
        "Variable::Flow(Flow {{
            ident: \"{}\".to_string(),
            equation: {},
            documentation: \"{}\".to_string(),
            units: {},
            gf: {},
            non_negative: {},
            can_be_module_input: {},
            visibility: {},
            ai_state: None,
            uid: {:?},
        }})",
        escape_string(&flow.ident),
        gen_equation(&flow.equation),
        escape_string(&flow.documentation),
        gen_option_string(&flow.units),
        gen_option_graphical_function(&flow.gf),
        flow.non_negative,
        flow.can_be_module_input,
        gen_visibility(flow.visibility),
        flow.uid
    )
}

fn gen_aux(aux: &datamodel::Aux) -> String {
    format!(
        "Variable::Aux(Aux {{
            ident: \"{}\".to_string(),
            equation: {},
            documentation: \"{}\".to_string(),
            units: {},
            gf: {},
            can_be_module_input: {},
            visibility: {},
            ai_state: None,
            uid: {:?},
        }})",
        escape_string(&aux.ident),
        gen_equation(&aux.equation),
        escape_string(&aux.documentation),
        gen_option_string(&aux.units),
        gen_option_graphical_function(&aux.gf),
        aux.can_be_module_input,
        gen_visibility(aux.visibility),
        aux.uid
    )
}

fn gen_module(module: &datamodel::Module) -> String {
    let refs: Vec<String> = module
        .references
        .iter()
        .map(|r| {
            format!(
                "ModuleReference {{ src: \"{}\".to_string(), dst: \"{}\".to_string() }}",
                escape_string(&r.src),
                escape_string(&r.dst)
            )
        })
        .collect();

    format!(
        "Variable::Module(Module {{
            ident: \"{}\".to_string(),
            model_name: \"{}\".to_string(),
            documentation: \"{}\".to_string(),
            units: {},
            references: vec![{}],
            can_be_module_input: {},
            visibility: {},
            ai_state: None,
            uid: {:?},
        }})",
        escape_string(&module.ident),
        escape_string(&module.model_name),
        escape_string(&module.documentation),
        gen_option_string(&module.units),
        refs.join(", "),
        module.can_be_module_input,
        gen_visibility(module.visibility),
        module.uid
    )
}

fn gen_variable(var: &datamodel::Variable) -> String {
    match var {
        datamodel::Variable::Stock(stock) => gen_stock(stock),
        datamodel::Variable::Flow(flow) => gen_flow(flow),
        datamodel::Variable::Aux(aux) => gen_aux(aux),
        datamodel::Variable::Module(module) => gen_module(module),
    }
}

fn gen_label_side(side: datamodel::view_element::LabelSide) -> &'static str {
    use datamodel::view_element::LabelSide;
    match side {
        LabelSide::Top => "LabelSide::Top",
        LabelSide::Left => "LabelSide::Left",
        LabelSide::Center => "LabelSide::Center",
        LabelSide::Bottom => "LabelSide::Bottom",
        LabelSide::Right => "LabelSide::Right",
    }
}

fn gen_flow_point(p: &datamodel::view_element::FlowPoint) -> String {
    format!(
        "FlowPoint {{ x: {}_f64, y: {}_f64, attached_to_uid: {:?} }}",
        p.x, p.y, p.attached_to_uid
    )
}

fn gen_link_shape(shape: &datamodel::view_element::LinkShape) -> String {
    use datamodel::view_element::LinkShape;
    match shape {
        LinkShape::Straight => "LinkShape::Straight".to_string(),
        LinkShape::Arc(angle) => format!("LinkShape::Arc({}_f64)", angle),
        LinkShape::MultiPoint(points) => {
            let pts: Vec<String> = points.iter().map(gen_flow_point).collect();
            format!("LinkShape::MultiPoint(vec![{}])", pts.join(", "))
        }
    }
}

fn gen_link_polarity(pol: &Option<datamodel::view_element::LinkPolarity>) -> String {
    use datamodel::view_element::LinkPolarity;
    match pol {
        None => "None".to_string(),
        Some(LinkPolarity::Positive) => "Some(LinkPolarity::Positive)".to_string(),
        Some(LinkPolarity::Negative) => "Some(LinkPolarity::Negative)".to_string(),
    }
}

fn gen_view_element(elem: &ViewElement) -> String {
    match elem {
        ViewElement::Aux(a) => format!(
            "ViewElement::Aux(view_element::Aux {{
                name: \"{}\".to_string(),
                uid: {},
                x: {}_f64,
                y: {}_f64,
                label_side: {},
            }})",
            escape_string(&a.name),
            a.uid,
            a.x,
            a.y,
            gen_label_side(a.label_side)
        ),
        ViewElement::Stock(s) => format!(
            "ViewElement::Stock(view_element::Stock {{
                name: \"{}\".to_string(),
                uid: {},
                x: {}_f64,
                y: {}_f64,
                label_side: {},
            }})",
            escape_string(&s.name),
            s.uid,
            s.x,
            s.y,
            gen_label_side(s.label_side)
        ),
        ViewElement::Flow(f) => {
            let pts: Vec<String> = f.points.iter().map(gen_flow_point).collect();
            format!(
                "ViewElement::Flow(view_element::Flow {{
                    name: \"{}\".to_string(),
                    uid: {},
                    x: {}_f64,
                    y: {}_f64,
                    label_side: {},
                    points: vec![{}],
                }})",
                escape_string(&f.name),
                f.uid,
                f.x,
                f.y,
                gen_label_side(f.label_side),
                pts.join(", ")
            )
        }
        ViewElement::Link(l) => format!(
            "ViewElement::Link(view_element::Link {{
                uid: {},
                from_uid: {},
                to_uid: {},
                shape: {},
                polarity: {},
            }})",
            l.uid,
            l.from_uid,
            l.to_uid,
            gen_link_shape(&l.shape),
            gen_link_polarity(&l.polarity)
        ),
        ViewElement::Module(m) => format!(
            "ViewElement::Module(view_element::Module {{
                name: \"{}\".to_string(),
                uid: {},
                x: {}_f64,
                y: {}_f64,
                label_side: {},
            }})",
            escape_string(&m.name),
            m.uid,
            m.x,
            m.y,
            gen_label_side(m.label_side)
        ),
        ViewElement::Alias(a) => format!(
            "ViewElement::Alias(view_element::Alias {{
                uid: {},
                alias_of_uid: {},
                x: {}_f64,
                y: {}_f64,
                label_side: {},
            }})",
            a.uid,
            a.alias_of_uid,
            a.x,
            a.y,
            gen_label_side(a.label_side)
        ),
        ViewElement::Cloud(c) => format!(
            "ViewElement::Cloud(view_element::Cloud {{
                uid: {},
                flow_uid: {},
                x: {}_f64,
                y: {}_f64,
            }})",
            c.uid, c.flow_uid, c.x, c.y
        ),
        ViewElement::Group(g) => format!(
            "ViewElement::Group(view_element::Group {{
                uid: {},
                name: \"{}\".to_string(),
                x: {}_f64,
                y: {}_f64,
                width: {}_f64,
                height: {}_f64,
            }})",
            g.uid,
            escape_string(&g.name),
            g.x,
            g.y,
            g.width,
            g.height
        ),
    }
}

fn gen_rect(r: &Rect) -> String {
    format!(
        "Rect {{ x: {}_f64, y: {}_f64, width: {}_f64, height: {}_f64 }}",
        r.x, r.y, r.width, r.height
    )
}

fn gen_stock_flow(sf: &StockFlow) -> String {
    let elems: Vec<String> = sf.elements.iter().map(gen_view_element).collect();
    format!(
        "StockFlow {{
            elements: vec![{}],
            view_box: {},
            zoom: {}_f64,
            use_lettered_polarity: {},
        }}",
        elems.join(",\n                "),
        gen_rect(&sf.view_box),
        sf.zoom,
        sf.use_lettered_polarity
    )
}

fn gen_view(view: &View) -> String {
    match view {
        View::StockFlow(sf) => format!("View::StockFlow({})", gen_stock_flow(sf)),
    }
}

fn gen_loop_metadata(lm: &LoopMetadata) -> String {
    let uids: Vec<String> = lm.uids.iter().map(|u| u.to_string()).collect();
    format!(
        "LoopMetadata {{
            uids: vec![{}],
            deleted: {},
            name: \"{}\".to_string(),
            description: \"{}\".to_string(),
        }}",
        uids.join(", "),
        lm.deleted,
        escape_string(&lm.name),
        escape_string(&lm.description)
    )
}

fn gen_model_group(g: &ModelGroup) -> String {
    format!(
        "ModelGroup {{
            name: \"{}\".to_string(),
            doc: {},
            parent: {},
            members: {},
            run_enabled: {},
        }}",
        escape_string(&g.name),
        gen_option_string(&g.doc),
        gen_option_string(&g.parent),
        gen_vec_string(&g.members),
        g.run_enabled
    )
}

fn gen_dt(dt: &Dt) -> String {
    match dt {
        Dt::Dt(v) => format!("Dt::Dt({}_f64)", v),
        Dt::Reciprocal(v) => format!("Dt::Reciprocal({}_f64)", v),
    }
}

fn gen_option_dt(opt: &Option<Dt>) -> String {
    match opt {
        Some(dt) => format!("Some({})", gen_dt(dt)),
        None => "None".to_string(),
    }
}

fn gen_sim_method(m: SimMethod) -> &'static str {
    match m {
        SimMethod::Euler => "SimMethod::Euler",
        SimMethod::RungeKutta2 => "SimMethod::RungeKutta2",
        SimMethod::RungeKutta4 => "SimMethod::RungeKutta4",
    }
}

fn gen_sim_specs(specs: &SimSpecs) -> String {
    format!(
        "SimSpecs {{
            start: {}_f64,
            stop: {}_f64,
            dt: {},
            save_step: {},
            sim_method: {},
            time_units: {},
        }}",
        specs.start,
        specs.stop,
        gen_dt(&specs.dt),
        gen_option_dt(&specs.save_step),
        gen_sim_method(specs.sim_method),
        gen_option_string(&specs.time_units)
    )
}

fn gen_option_sim_specs(opt: &Option<SimSpecs>) -> String {
    match opt {
        Some(specs) => format!("Some({})", gen_sim_specs(specs)),
        None => "None".to_string(),
    }
}

fn gen_model(model: &Model) -> String {
    let vars: Vec<String> = model.variables.iter().map(gen_variable).collect();
    let views: Vec<String> = model.views.iter().map(gen_view).collect();
    let loop_meta: Vec<String> = model.loop_metadata.iter().map(gen_loop_metadata).collect();
    let groups: Vec<String> = model.groups.iter().map(gen_model_group).collect();

    format!(
        "Model {{
        name: \"{}\".to_string(),
        sim_specs: {},
        variables: vec![
            {}
        ],
        views: vec![{}],
        loop_metadata: vec![{}],
        groups: vec![{}],
    }}",
        escape_string(&model.name),
        gen_option_sim_specs(&model.sim_specs),
        vars.join(",\n            "),
        views.join(", "),
        loop_meta.join(", "),
        groups.join(", ")
    )
}

fn model_name_to_fn_name(name: &str) -> String {
    name.replace(['-', ' '], "_")
}

pub fn generate(stdlib_dir: &str, output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stdlib_path = Path::new(stdlib_dir);

    let mut entries: Vec<_> = fs::read_dir(stdlib_path)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "stmx"))
        .collect();
    entries.sort_by_key(|e| e.path());

    // Compute hash of all .stmx files for staleness detection
    // Include filenames to detect renames even when content stays the same
    let mut hasher = Sha256::new();
    for entry in &entries {
        let path = entry.path();
        let file_stem = path
            .file_stem()
            .expect("stmx file should have a file stem")
            .to_string_lossy();
        hasher.update(file_stem.as_bytes());
        hasher.update(fs::read(&path)?);
    }
    let hash = format!("{:x}", hasher.finalize());

    // Parse all models
    let mut models: Vec<(String, Model)> = Vec::new();
    for entry in &entries {
        let path = entry.path();
        let file_stem = path
            .file_stem()
            .expect("stmx file should have a file stem")
            .to_string_lossy()
            .into_owned();

        let file = File::open(&path)?;
        let mut reader = BufReader::new(file);
        let project = open_xmile(&mut reader).map_err(|e| format!("{}: {}", path.display(), e))?;

        if project.models.len() != 1 {
            return Err(format!(
                "{}: expected 1 model, found {}",
                path.display(),
                project.models.len()
            )
            .into());
        }

        models.push((
            file_stem,
            project
                .models
                .into_iter()
                .next()
                .expect("already verified exactly 1 model"),
        ));
    }

    // Generate Rust code
    let mut output = File::create(output_path)?;

    writeln!(output, "// @generated from stdlib/*.stmx")?;
    writeln!(
        output,
        "// DO NOT EDIT - regenerate with: pnpm rebuild-stdlib"
    )?;
    writeln!(output, "//")?;
    writeln!(output, "// Stdlib SHA256: {}", hash)?;
    writeln!(output)?;
    writeln!(
        output,
        "#![allow(clippy::approx_constant, clippy::excessive_precision, clippy::unreadable_literal, unused_imports)]"
    )?;
    writeln!(output)?;
    writeln!(
        output,
        "use crate::datamodel::{{
    Aux, Dt, Equation, Flow, GraphicalFunction, GraphicalFunctionKind,
    GraphicalFunctionScale, LoopMetadata, Model, ModelGroup, Module, ModuleReference,
    Rect, SimMethod, SimSpecs, Stock, StockFlow, Variable, View, ViewElement, Visibility,
    view_element,
}};
use crate::datamodel::view_element::{{FlowPoint, LabelSide, LinkPolarity, LinkShape}};"
    )?;
    writeln!(output)?;

    // Generate MODEL_NAMES constant
    writeln!(
        output,
        "pub const MODEL_NAMES: [&str; {}] = [",
        models.len()
    )?;
    for (name, _) in &models {
        writeln!(output, "    \"{}\",", name)?;
    }
    writeln!(output, "];")?;
    writeln!(output)?;

    // Generate get function
    writeln!(output, "pub fn get(name: &str) -> Option<Model> {{")?;
    writeln!(output, "    match name {{")?;
    for (name, _) in &models {
        writeln!(
            output,
            "        \"{}\" => Some({}()),",
            name,
            model_name_to_fn_name(name)
        )?;
    }
    writeln!(output, "        _ => None,")?;
    writeln!(output, "    }}")?;
    writeln!(output, "}}")?;
    writeln!(output)?;

    // Generate individual model functions
    for (name, model) in &models {
        writeln!(output, "fn {}() -> Model {{", model_name_to_fn_name(name))?;
        writeln!(output, "    {}", gen_model(model))?;
        writeln!(output, "}}")?;
        writeln!(output)?;
    }

    eprintln!(
        "Generated {} with {} models (hash: {})",
        output_path,
        models.len(),
        &hash[..12]
    );

    Ok(())
}
