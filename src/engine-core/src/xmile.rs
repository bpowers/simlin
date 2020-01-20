// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fmt;

use serde::{Deserialize, Serialize};

static VERSION: &str = "1.0";
static NS_HTTPS: &str = "https://docs.oasis-open.org/xmile/ns/XMILE/v1.0";
static NS_HTTP: &str = "http://docs.oasis-open.org/xmile/ns/XMILE/v1.0";

#[derive(Deserialize, Serialize)]
#[serde(rename = "xmile")]
pub struct File {
    #[serde(default)]
    pub version: String,
    #[serde(rename = "xmlns", default)]
    pub namespace: String, // 'https://docs.oasis-open.org/xmile/ns/XMILE/v1.0'
    pub header: Option<Header>,
    pub sim_specs: Option<SimSpecs>,
    #[serde(rename = "model_units")]
    pub units: Option<Units>,
    pub dimensions: Option<Dimensions>,
    pub behavior: Option<Behavior>,
    pub style: Option<Style>,
    pub data: Option<Data>,
    #[serde(rename = "model", default)]
    pub models: Vec<Model>,
    #[serde(rename = "macro", default)]
    pub macros: Vec<Macro>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Data {
    // TODO
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Macro {
    // TODO
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VarDimensions {
    #[serde(rename = "dim")]
    pub dimensions: Option<Vec<VarDimension>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VarDimension {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Dimensions {
    #[serde(rename = "dimension")]
    pub dimensions: Option<Vec<Dimension>>,
}

impl File {
    pub fn get_models(&self) -> &Vec<Model> {
        &self.models
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "File{{")?;
        writeln!(f, "      version:    {}", self.version)?;
        writeln!(f, "      namespace:  {}", self.namespace)?;
        writeln!(f, "      header:     {:?}", self.header)?;
        writeln!(f, "      sim_specs:  {:?}", self.sim_specs)?;
        writeln!(f, "      dimensions: {:?}", self.dimensions)?;
        writeln!(f, "      units:      {:?}", self.units)?;
        writeln!(f, "      behavior:   {:?}", self.behavior)?;
        writeln!(f, "      style:      {:?}", self.style)?;
        writeln!(f, "      models: [")?;
        for m in &self.models {
            writeln!(f, "        {:?}", m)?;
        }
        writeln!(f, "      ]    }}")
    }
}

#[derive(Deserialize, Serialize)]
pub struct Header {
    pub vendor: String,
    pub product: Product,
    pub options: Option<Options>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub caption: Option<Caption>,
    pub image: Option<Image>,
    pub author: Option<String>,
    pub affiliation: Option<String>,
    pub client: Option<String>,
    pub copyright: Option<String>,
    pub created: Option<String>, // ISO 8601 date format, e.g. “ 2014-08-10”
    pub modified: Option<String>, // ISO 8601 date format
    pub uuid: Option<String>,    // IETF RFC4122 format (84-4-4-12 hex digits with the dashes)
    pub includes: Option<Includes>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Caption {}

#[derive(Debug, Deserialize, Serialize)]
pub struct Includes {}

#[derive(Debug, Deserialize, Serialize)]
pub struct Image {
    #[serde(default)]
    pub resource: String, // "JPG, GIF, TIF, or PNG" path, URL, or image embedded in base64 data URI
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Header{{")?;
        writeln!(f, "        vendor:      {}", self.vendor)?;
        writeln!(f, "        product:     {:?}", self.product)?;
        writeln!(f, "        options:     {:?}", self.options)?;
        writeln!(f, "        name:        {:?}", self.name)?;
        writeln!(f, "        version:     {:?}", self.version)?;
        writeln!(f, "        caption:     {:?}", self.caption)?;
        writeln!(f, "        image:       {:?}", self.image)?;
        writeln!(f, "        author:      {:?}", self.author)?;
        writeln!(f, "        affiliation: {:?}", self.affiliation)?;
        writeln!(f, "        client:      {:?}", self.client)?;
        writeln!(f, "        copyright:   {:?}", self.copyright)?;
        writeln!(f, "        created:     {:?}", self.created)?;
        writeln!(f, "        modified:    {:?}", self.modified)?;
        writeln!(f, "        uuid:        {:?}", self.uuid)?;
        writeln!(f, "        includes:    {:?}", self.includes)?;
        write!(f, "      }}")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Product {
    #[serde(rename = "$value")]
    pub name: Option<String>,
    #[serde(rename = "lang")]
    pub language: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Hash, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    UsesArrays {
        maximum_dimensions: Option<i64>,
        invalid_index_value: Option<String>, // e.g. "NaN" or "0"; string for Eq + Hash},
    },
    UsesMacros {
        recursive_macros: Option<bool>,
        option_filters: Option<bool>,
    },
    UsesConveyor {
        arrest: Option<bool>,
        leak: Option<bool>,
    },
    UsesQueue {
        overflow: Option<bool>,
    },
    UsesEventPosters {
        messages: Option<bool>,
    },
    HasModelView,
    UsesOutputs {
        numeric_display: Option<bool>,
        lamp: Option<bool>,
        gauge: Option<bool>,
    },
    UsesInputs {
        numeric_input: Option<bool>,
        list: Option<bool>,
        graphical_input: Option<bool>,
    },
    UsesAnnotation,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Options {
    pub namespace: Option<String>, // string of comma separated namespaces
    #[serde(rename = "$value")]
    pub features: Option<Vec<Feature>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: Option<Dt>,
    #[serde(rename = "savestep")]
    pub save_step: Option<f64>,
    pub method: Option<String>,
    pub time_units: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Dt {
    #[serde(rename = "$value")]
    pub value: f64,
    pub reciprocal: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Dimension {
    pub name: String,
    pub size: Option<u32>,
    #[serde(rename = "elem")]
    pub elements: Option<Vec<Index>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Index {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableType {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GF {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<TableType>,
    #[serde(rename = "xscale")]
    pub x_scale: Option<Scale>,
    #[serde(rename = "yscale")]
    pub y_scale: Option<Scale>,
    #[serde(rename = "xpts")]
    pub x_pts: Option<String>, // comma separated list of points
    #[serde(rename = "ypts")]
    pub y_pts: Option<String>, // comma separated list of points
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Behavior {
    // TODO
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Style {
    // TODO
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Units {
    pub unit: Option<Vec<Unit>>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Unit {
    pub name: String,
    pub eqn: Option<String>,
    pub alias: Option<Vec<String>>,
    pub disabled: Option<bool>,
}

#[derive(Deserialize, Serialize)]
pub struct Model {
    pub name: Option<String>,
    pub run: Option<bool>, // false
    #[serde(rename = "namespace")]
    pub namespaces: Option<String>, // comma separated list of namespaces
    pub resource: Option<String>, // path or URL to separate resource file
    pub sim_specs: Option<SimSpecs>,
    pub variables: Option<Variables>,
    pub views: Option<Views>,
}

#[derive(Deserialize, Serialize)]
pub struct Variables {
    #[serde(rename = "$value")]
    pub variables: Vec<Var>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Views {
    pub view: Vec<View>,
}

impl Model {
    pub fn get_name(&self) -> &str {
        &self.name.as_deref().unwrap_or("main")
    }
}

impl fmt::Debug for Model {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Model{{")?;
        writeln!(f, "          name:       {}", self.get_name())?;
        writeln!(f, "          run:        {}", self.run.unwrap_or(false))?;
        writeln!(f, "          namespaces: {:?}", self.namespaces)?;
        writeln!(f, "          resource:   {:?}", self.resource)?;
        writeln!(f, "          sim_specs:  {:?}", self.sim_specs)?;
        writeln!(f, "          vars: [")?;
        if let Some(vars) = &self.variables {
            for v in &vars.variables {
                writeln!(f, "            {:?}", v)?;
            }
        }
        writeln!(f, "          ]")?;
        writeln!(f, "          views:      {:?}", self.views)?;
        write!(f, "        }}")
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    StockFlow,
    Interface,
    Popup,
    VendorSpecific,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSide {
    Top,
    Left,
    Center,
    Bottom,
    Right,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Point {
    x: f64,
    y: f64,
    uid: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Points {
    #[serde(rename = "pt")]
    points: Vec<Point>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewObject {
    Stock {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
    },
    Flow {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
        #[serde(rename = "pts")]
        points: Option<Points>,
    },
    Aux {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
    },
    Connector {
        uid: Option<i32>,
        label_side: Option<LabelSide>,
        label_angle: Option<f64>,
        from: String,
        to: String,
        angle: Option<f64>,
        #[serde(rename = "pts")]
        points: Option<Points>, // for multi-point connectors
    },
    Module {
        name: String,
        uid: Option<i32>,
        x: f64,
        y: f64,
    },
    Style(Style),
    StackedContainer,
    SimulationDelay,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct View {
    #[serde(rename = "type")]
    pub kind: Option<ViewType>,
    pub background: Option<String>,
    pub page_width: Option<String>,
    pub page_height: Option<String>,
    pub show_pages: Option<bool>,
    #[serde(rename = "$value", default)]
    pub objects: Vec<ViewObject>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ArrayElement {
    pub subscript: String,
    pub eqn: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Module {
    pub name: String,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub refs: Option<Vec<Ref>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Ref {
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NonNegative {}

#[derive(Debug, Deserialize, Serialize)]
pub struct Stock {
    pub name: String,
    pub eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub inflows: Option<Vec<String>>,
    pub outflows: Option<Vec<String>>,
    pub non_negative: Option<NonNegative>,
    pub dimensions: Option<VarDimensions>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Flow {
    pub name: String,
    pub eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub gf: Option<GF>,
    pub non_negative: Option<NonNegative>,
    pub dimensions: Option<VarDimensions>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Aux {
    pub name: String,
    pub eqn: Option<String>,
    pub doc: Option<String>,
    pub units: Option<String>,
    pub gf: Option<GF>,
    pub dimensions: Option<VarDimensions>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Var {
    Stock(Stock),
    Flow(Flow),
    Aux(Aux),
    Module(Module),
}

impl fmt::Debug for Var {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Var::Stock(ref stock) => write!(f, "{:?}", stock),
            Var::Flow(ref flow) => write!(f, "{:?}", flow),
            Var::Aux(ref aux) => write!(f, "{:?}", aux),
            Var::Module(ref module) => write!(f, "{:?}", module),
        }
    }
}

/*
macro_rules! ensure {
    (element: $xmile: expr) => {{
        match $xmile.node {
            Element(_, _) => {}
            _ => {
                return err!("expected an Element, not {:?}", $xmile.node);
            }
        };
    }};
    ($attrs: expr, $name: expr, $val: expr) => {{
        match attr($attrs, $name) {
            Some(val) if &val[..] == $val => {}
            _ => {
                return err!("Expected '{}' to be {}.", $name, $val);
            }
        };
    }};
}

macro_rules! child {
    ($xmile: expr, $name: expr) => {{
        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        match all_tags.len() {
            0 => None,
            1 => Some(all_tags[0]),
            _ => {
                return err!("expected 1 {}, not {}", $name, all_tags.len());
            }
        }
    }};
}

macro_rules! parse_number_list {
    ($nums: expr) => {{
        let mut result: Vec<f64> = Vec::new();
        for n in $nums.split(",").map(|s| {
            use std::str::FromStr;
            f64::from_str(s)
        }) {
            result.push(n?);
        }
        result
    }};
}

macro_rules! required {
    ($xmile: expr, child: $name: expr, text) => {{
        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        match all_tags.len() {
            1 => required!(all_tags[0].borrow(), text),
            _ => {
                return err!("expected 1 {}, not {}", $name, all_tags.len());
            }
        }
    }};
    ($xmile: expr, child: $name: expr, f64) => {{
        use std::str::FromStr;

        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        let value = match all_tags.len() {
            1 => required!(all_tags[0].borrow(), text),
            _ => {
                return err!("expected 1 {}, not {}", $name, all_tags.len());
            }
        };
        f64::from_str(value.as_str())?
    }};
    ($xmile: expr, child: $name: expr, $ty: tt) => {{
        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        if all_tags.len() != 1 {
            return err!("expected 1 {}, not {}", $name, all_tags.len());
        }
        $ty::Deserialize, Serialize(all_tags[0])?
    }};
    ($xmile: expr, text) => {{
        let mut text: String = "".to_string();
        for el in $xmile.children.iter() {
            match el.borrow().node {
                Text(ref t) => {
                    text += t;
                }
                _ => {}
            };
        }
        text.trim().to_string()
    }};
    ($xmile: expr, $ty: tt) => {{
        $ty::Deserialize, Serialize($xmile)?
    }};
    ($xmile: expr, attr: $name: expr, text) => {{
        if let Element(_, ref attrs) = $xmile.node {
            match attr(attrs, $name) {
                Some(val) => val.to_string(),
                None => {
                    return err!("missing required attr {}", $name);
                }
            }
        } else {
            return err!("expected an Element, not {:?}", $xmile.node);
        }
    }};
    ($xmile: expr, attr: $name: expr, f64) => {{
        use std::str::FromStr;
        f64::from_str(required!($xmile, attr: $name, text).as_str())?
    }};
}

macro_rules! optional {
    ($xmile: expr, child: $name: expr, text) => {{
        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        match all_tags.len() {
            0 => None,
            1 => Some(required!(all_tags[0].borrow(), text)),
            _ => {
                return err!("expected 0 or 1 {}, not {}", $name, all_tags.len());
            }
        }
    }};
    ($xmile: expr, child: $name: expr, f64) => {{
        use std::str::FromStr;
        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        match all_tags.len() {
            0 => None,
            1 => Some(f64::from_str(
                required!(all_tags[0].borrow(), text).as_str(),
            )?),
            _ => {
                return err!("expected 0 or 1 {}, not {}", $name, all_tags.len());
            }
        }
    }};
    ($xmile: expr, child: $name: expr, bool) => {{
        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        match all_tags.len() {
            0 => None,
            1 => {
                let is_true = match required!(all_tags[0].borrow(), text).as_str() {
                    "true" => true,
                    "false" => false,
                    "" => true,
                    _ => {
                        return err!("expected inside of {} to be true, false or empty", $name);
                    }
                };
                Some(is_true)
            }
            _ => {
                return err!("expected 0 or 1 {}, not {}", $name, all_tags.len());
            }
        }
    }};
    ($xmile: expr, child: $name: expr, $ty: tt) => {{
        let all_tags: Vec<_> = $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .collect();
        match all_tags.len() {
            0 => None,
            1 => Some($ty::Deserialize, Serialize(all_tags[0])?),
            _ => {
                return err!("expected 0 or 1 {}, not {}", $name, all_tags.len());
            }
        }
    }};
    ($xmile: expr, attr: $name: expr, text) => {{
        if let Element(_, ref attrs) = $xmile.node {
            match attr(attrs, $name) {
                Some(val) => Some(val.to_string()),
                None => None,
            }
        } else {
            return err!("expected an Element, not {:?}", $xmile.node);
        }
    }};
    ($xmile: expr, children: $name: expr, text) => {{
        let mut Deserialize, Serialized: Vec<_> = Vec::new();
        for r in $xmile.children.iter().filter(|h| element_named(h, $name)) {
            Deserialize, Serialized.push(required!(r.borrow(), text));
        }

        Deserialize, Serialized
    }};
    ($xmile: expr, children: $name: expr, $ty: tt) => {{
        let mut Deserialize, Serialized: Vec<_> = Vec::new();
        for r in $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .map($ty::Deserialize, Serialize)
        {
            match r {
                Ok(el) => Deserialize, Serialized.push(el),
                Err(err) => {
                    return err!("consume_many('{}'): {:?}", $name, err);
                }
            };
        }

        Deserialize, Serialized
    }};
}

impl XmileNode for Product {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let xmile = handle.borrow();
        ensure!(element: xmile);

        let product = Product {
            name: required!(xmile, text),
            language: optional!(xmile, attr: "lang", text),
            version: required!(xmile, attr: "version", text),
        };
        Ok(Rc::new(product))
    }
}

impl XmileNode for Options {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let xmile = handle.borrow();
        ensure!(element: xmile);

        println!("TODO: options");
        let options = Options {
            namespaces: Vec::new(),
            features: EnumSet::new(),
            maximum_dimensions: None,
            invalid_index_value: None,
        };
        Ok(Rc::new(options))
    }
}

impl XmileNode for Table {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let xmile = handle.borrow();
        ensure!(element: xmile);

        let ypts = required!(xmile, child: "ypts", text);

        let ys = parse_number_list!(ypts);
        let xs: Vec<f64> = if let Some(xpts) = optional!(xmile, child: "xpts", text) {
            parse_number_list!(xpts)
        } else {
            if let Some(xscale_handle) = child!(xmile, "xscale") {
                let xscale_xmile = xscale_handle.borrow();
                let min = required!(xscale_xmile, attr: "min", f64);
                let max = required!(xscale_xmile, attr: "max", f64);

                let len = ys.len();
                let mut xs: Vec<f64> = Vec::with_capacity(len);

                for i in 0..len {
                    xs[i] = (i as f64) / (len as f64) * (max - min) + min;
                }

                xs
            } else {
                return err!("Expected either xscale or xpts, found neither.");
            }
        };

        let table = Table {
            kind: TableType::Discrete,
            x: xs,
            y: ys,
        };
        Ok(Rc::new(table))
    }
}

impl XmileNode for Header {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let xmile = handle.borrow();
        ensure!(element: xmile);

        let header = Header {
            vendor: required!(xmile, child: "vendor", text),
            product: (*required!(xmile, child: "product", Product)).clone(),
            options: optional!(xmile, child: "options", Options),
            name: optional!(xmile, child: "name", text),
            version: optional!(xmile, child: "version", text),
            author: optional!(xmile, child: "author", text),
            affiliation: optional!(xmile, child: "author", text),
            client: optional!(xmile, child: "client", text),
            copyright: optional!(xmile, child: "copyright", text),
            created: optional!(xmile, child: "created", text),
            modified: optional!(xmile, child: "modified", text),
            uuid: optional!(xmile, child: "uuid", text),
        };
        Ok(Rc::new(header))
    }
}

impl XmileNode for SimSpec {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let xmile = handle.borrow();

        let mut dt = None;
        let mut dt_reciprocal = None;
        // the value can represent either the actual dt, or $x where
        // dt = 1/$x, depending on the attribute "reciprocal"
        if let Some(dt_xmile) = child!(xmile, "dt") {
            if let Element(_, ref dt_attrs) = dt_xmile.borrow().node {
                let val = optional!(xmile, child: "dt", f64);
                match attr(dt_attrs, "reciprocal") {
                    Some(ref is_recip) if is_recip.to_string() == "true" => {
                        dt_reciprocal = val;
                    }
                    _ => {
                        dt = val;
                    }
                };
            }
        }

        let mut savestep = optional!(xmile, child: "savestep", f64);
        if let None = savestep {
            savestep = optional!(xmile, child: "save_step", f64);
        }

        let sim_spec = SimSpec {
            start: required!(xmile, child: "start", f64),
            stop: required!(xmile, child: "stop", f64),
            dt: dt,
            dt_reciprocal: dt_reciprocal,
            savestep: savestep,
            method: optional!(xmile, attr: "method", text),
            time_units: optional!(xmile, attr: "time_units", text),
        };
        Ok(Rc::new(sim_spec))
    }
}

impl XmileNode for Var {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let xmile = handle.borrow();
        ensure!(element: xmile);

        if let Element(ref name, _) = xmile.node {
            let var = match &name.local[..] {
                "stock" => Var::Stock(Stock {
                    name: canonicalize(&required!(xmile, attr: "name", text)),
                    eqn: required!(xmile, child: "eqn", text),
                    inflows: optional!(xmile, children: "inflow", text)
                        .into_iter()
                        .map(|n| canonicalize(&n))
                        .collect(),
                    outflows: optional!(xmile, children: "outflow", text)
                        .into_iter()
                        .map(|n| canonicalize(&n))
                        .collect(),
                    non_negative: optional!(xmile, child: "non_negative", bool),
                }),
                "flow" => Var::Flow(Flow {
                    name: canonicalize(&required!(xmile, attr: "name", text)),
                    eqn: required!(xmile, child: "eqn", text),
                    table: optional!(xmile, child: "gf", Table),
                    non_negative: optional!(xmile, child: "non_negative", bool),
                }),
                "aux" => Var::Aux(Aux {
                    name: canonicalize(&required!(xmile, attr: "name", text)),
                    eqn: required!(xmile, child: "eqn", text),
                    table: optional!(xmile, child: "gf", Table),
                }),
                "module" => Var::Module(Module {
                    name: canonicalize(&required!(xmile, attr: "name", text)),
                    refs: optional!(xmile, children: "connect", Var),
                }),
                "connect" => Var::Ref(Ref {
                    src: canonicalize(&required!(xmile, attr: "from", text)),
                    dst: canonicalize(&required!(xmile, attr: "to", text)),
                }),
                _ => {
                    return err!("unknown variable type {}", &name.local[..]);
                }
            };
            Ok(Rc::new(var))
        } else {
            return err!("unknown var type {:?}", xmile.node);
        }
    }
}

impl XmileNode for Model {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let xmile = handle.borrow();
        ensure!(element: xmile);

        let name = match optional!(xmile, attr: "name", text) {
            Some(name) => name,
            None => "main".to_string(),
        };

        let mut vars: Vec<Rc<Var>> = Vec::new();
        if let Some(vars_xmile) = child!(xmile, "variables") {
            for var_handle in vars_xmile.borrow().children.iter() {
                let var_xmile = var_handle.borrow();
                if let Element(_, _) = var_xmile.node {
                    vars.push(required!(var_handle, Var));
                }
            }
        };

        let model = Model {
            name: name,
            run: false,
            namespaces: Vec::new(),
            resource: None,
            sim_spec: None,
            vars: vars,
            views: Vec::new(),
        };
        Ok(Rc::new(model))
    }
}

impl XmileNode for File {
    fn Deserialize, Serialize(handle: &Handle) -> Result<Rc<Self>> {
        let node = handle.borrow();
        if let rcdom::Document = node.node {
        } else {
            return err!("File expected document, not {:?}", node.node);
        }

        let mut file: Option<File> = None;

        for child_handle in node.children.iter() {
            let xmile = child_handle.borrow();

            if let Element(ref name, ref attrs) = xmile.node {
                if &name.local[..] == "xmile" && &name.prefix[..] == "" {
                    if &name.namespace_url[..] != NS_HTTP && &name.namespace_url[..] != NS_HTTPS {
                        return err!(
                            "Expected v1.0 XMILE namespace, not \"{}\".",
                            &name.namespace_url[..]
                        );
                    }

                    ensure!(attrs, "version", "1.0");

                    let header = required!(xmile, child: "header", Header);
                    let sim_spec = optional!(xmile, child: "sim_specs", SimSpec);
                    let models = optional!(xmile, children: "model", Model);

                    file = Some(File {
                        version: VERSION.to_string(),
                        namespace: NS_HTTPS.to_string(),
                        header: header,
                        sim_spec: sim_spec,
                        dimensions: Vec::new(), // TODO
                        units: Vec::new(),      // TODO
                        behavior: Behavior {
                            // TODO
                            all_non_negative: None,
                            stock_non_negative: None,
                            flow_non_negative: None,
                        },
                        style: Style {}, // TODO
                        models: models,
                    });
                }
            }
        }

        match file {
            Some(f) => Ok(Rc::new(f)),
            None => err!("No xmile tags found."),
        }
    }
}
*/
