use xml5ever::rcdom::NodeEnum::*;
use xml5ever::rcdom::{self, Handle};
use xml5ever::tokenizer::{Attribute, QName};

use sd::common::{canonicalize, Ident, Result};
use sd::enum_set::{CLike, EnumSet};
use std::fmt;
use std::rc::Rc;
use xml5ever::tendril::StrTendril;

static VERSION: &'static str = "1.0";
static NS_HTTPS: &'static str = "https://docs.oasis-open.org/xmile/ns/XMILE/v1.0";
static NS_HTTP: &'static str = "http://docs.oasis-open.org/xmile/ns/XMILE/v1.0";

pub trait XmileNode {
    fn deserialize(handle: &Handle) -> Result<Rc<Self>>;
}

pub struct File {
    version: String,
    namespace: String, // 'https://docs.oasis-open.org/xmile/ns/XMILE/v1.0'
    header: Rc<Header>,
    sim_spec: Option<Rc<SimSpec>>,
    dimensions: Vec<Dimension>,
    units: Vec<Unit>,
    behavior: Behavior,
    style: Style,
    models: Vec<Rc<Model>>,
}

impl File {
    pub fn get_models(&self) -> &Vec<Rc<Model>> {
        &self.models
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "File{{\n")?;
        write!(f, "      version:    {}\n", self.version)?;
        write!(f, "      namespace:  {}\n", self.namespace)?;
        write!(f, "      header:     {:?}\n", self.header)?;
        write!(f, "      sim_spec:   {:?}\n", self.sim_spec)?;
        write!(f, "      dimensions: {:?}\n", self.dimensions)?;
        write!(f, "      units:      {:?}\n", self.units)?;
        write!(f, "      behavior:   {:?}\n", self.behavior)?;
        write!(f, "      style:      {:?}\n", self.style)?;
        write!(f, "      models: [\n")?;
        for m in &self.models {
            write!(f, "        {:?}\n", m)?;
        }
        write!(f, "      ]\n    }}")
    }
}

pub struct Header {
    vendor: String,
    product: Product,
    options: Option<Rc<Options>>,
    name: Option<String>,
    version: Option<String>,
    author: Option<String>,
    affiliation: Option<String>,
    client: Option<String>,
    copyright: Option<String>,
    created: Option<String>,  // ISO 8601 date format, e.g. “ 2014-08-10”
    modified: Option<String>, // ISO 8601 date format
    uuid: Option<String>,     // IETF RFC4122 format (84-4-4-12 hex digits with the dashes)
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Header{{\n")?;
        write!(f, "        vendor:      {}\n", self.vendor)?;
        write!(f, "        product:     {:?}\n", self.product)?;
        write!(f, "        options:     {:?}\n", self.options)?;
        write!(f, "        name:        {:?}\n", self.name)?;
        write!(f, "        versions:    {:?}\n", self.version)?;
        write!(f, "        author:      {:?}\n", self.author)?;
        write!(f, "        affiliation: {:?}\n", self.affiliation)?;
        write!(f, "        client:      {:?}\n", self.client)?;
        write!(f, "        copyright:   {:?}\n", self.copyright)?;
        write!(f, "        created:     {:?}\n", self.created)?;
        write!(f, "        modified:    {:?}\n", self.modified)?;
        write!(f, "        uuid:        {:?}\n", self.uuid)?;
        write!(f, "      }}")
    }
}

#[derive(Debug, Clone)]
pub struct Product {
    name: String,
    language: Option<String>,
    version: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Copy, Clone)]
pub enum Feature {
    UsesArrays,
    UsesMacros,
    UsesConveyor,
    UsesQueue,
    UsesEventPosters,
    HasModelView,
    UsesOutputs,
    UsesInputs,
    UsesAnnotation,
    // macros
    RecursiveMacros,
    OptionFilters,
    // conveyors
    Arrest,
    Leak,
    // queues
    Overflow,
    // event posters
    Messages,
    // outputs
    NumericDisplay,
    Lamp,
    Gauge,
    // inputs
    NumericInput,
    List,
    GraphicalInput,
}

impl CLike for Feature {
    fn to_usize(&self) -> usize {
        self.clone() as usize
    }
    fn from_usize(v: usize) -> Feature {
        use sd::core::mem;

        assert!(
            v < mem::size_of::<Feature>() * 8,
            "USize {} can't fit in feature.",
            v
        );

        unsafe { mem::transmute(v as u8) }
    }
}

#[derive(Debug)]
pub struct Options {
    namespaces: Vec<String>,
    features: EnumSet<Feature>,
    // arrays
    maximum_dimensions: Option<i32>,
    invalid_index_value: Option<f64>, // only 0 or NaN
}

#[derive(Debug)]
pub struct SimSpec {
    start: f64,
    stop: f64,
    dt: Option<f64>,
    dt_reciprocal: Option<f64>,
    savestep: Option<f64>,
    method: Option<String>,
    time_units: Option<String>,
}

#[derive(Debug)]
pub struct Dimension {
    name: String,
    size: u32,
    elements: Option<Vec<String>>,
}

#[derive(Debug)]
pub enum TableType {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Debug)]
pub struct Table {
    kind: TableType,
    x: Vec<f64>,
    y: Vec<f64>,
}

#[derive(Debug)]
pub struct Behavior {
    all_non_negative: Option<bool>,
    stock_non_negative: Option<bool>,
    flow_non_negative: Option<bool>,
}

#[derive(Debug)]
pub struct Style {}

#[derive(Debug)]
pub struct Unit {
    name: String,
    eqn: String,
    alias: String,
}

pub struct Model {
    name: String,
    run: bool, // false
    namespaces: Vec<String>,
    resource: Option<String>, // path or URL to separate resource file
    sim_spec: Option<Rc<SimSpec>>,
    vars: Vec<Rc<Var>>,
    views: Vec<Rc<View>>,
}

impl Model {
    pub fn get_name(&self) -> &String {
        &self.name
    }
}

impl fmt::Debug for Model {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Model{{\n")?;
        write!(f, "          name:       {}\n", self.name)?;
        write!(f, "          run:        {}\n", self.run)?;
        write!(f, "          namespaces: {:?}\n", self.namespaces)?;
        write!(f, "          resource:   {:?}\n", self.resource)?;
        write!(f, "          sim_spec:   {:?}\n", self.sim_spec)?;
        write!(f, "          vars: [\n")?;
        for v in &self.vars {
            write!(f, "            {:?}\n", v)?;
        }
        write!(f, "          ]\n")?;
        write!(f, "          views:      {:?}\n", self.views)?;
        write!(f, "        }}")
    }
}

#[derive(Debug)]
pub enum ViewType {
    StockFlow,
    Interface,
    Popup,
    VendorSpecific,
}

#[derive(Debug)]
pub struct View {
    kind: ViewType,
}

#[derive(Debug)]
pub struct Module {
    pub name: Ident,
    pub refs: Vec<Rc<Var>>,
}

#[derive(Debug)]
pub struct Ref {
    pub src: Ident,
    pub dst: Ident,
}

#[derive(Debug)]
pub struct Stock {
    pub name: Ident,
    pub eqn: String,
    pub inflows: Vec<Ident>,
    pub outflows: Vec<Ident>,
    pub non_negative: Option<bool>,
}

#[derive(Debug)]
pub struct Flow {
    pub name: Ident,
    pub eqn: String,
    pub table: Option<Rc<Table>>,
    pub non_negative: Option<bool>,
}

#[derive(Debug)]
pub struct Aux {
    pub name: Ident,
    pub eqn: String,
    pub table: Option<Rc<Table>>,
}

pub enum Var {
    Stock(Stock),
    Flow(Flow),
    Aux(Aux),
    Module(Module),
    Ref(Ref),
}

impl fmt::Debug for Var {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            &Var::Stock(ref stock) => write!(f, "{:?}", stock),
            &Var::Flow(ref flow) => write!(f, "{:?}", flow),
            &Var::Aux(ref aux) => write!(f, "{:?}", aux),
            &Var::Module(ref module) => write!(f, "{:?}", module),
            &Var::Ref(ref reference) => write!(f, "{:?}", reference),
        }
    }
}

fn named(qname: &QName, local: &str) -> bool {
    &qname.prefix[..] == "" && &qname.local[..] == local
}

fn attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a StrTendril> {
    for attr in attrs {
        if named(&attr.name, name) {
            return Some(&attr.value);
        }
    }
    None
}

fn element_named(h: &Handle, tag_name: &str) -> bool {
    let node = h.borrow();
    match node.node {
        Element(ref name, _) if named(name, tag_name) => true,
        _ => false,
    }
}

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
        $ty::deserialize(all_tags[0])?
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
        $ty::deserialize($xmile)?
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
            1 => Some($ty::deserialize(all_tags[0])?),
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
        let mut deserialized: Vec<_> = Vec::new();
        for r in $xmile.children.iter().filter(|h| element_named(h, $name)) {
            deserialized.push(required!(r.borrow(), text));
        }

        deserialized
    }};
    ($xmile: expr, children: $name: expr, $ty: tt) => {{
        let mut deserialized: Vec<_> = Vec::new();
        for r in $xmile
            .children
            .iter()
            .filter(|h| element_named(h, $name))
            .map($ty::deserialize)
        {
            match r {
                Ok(el) => deserialized.push(el),
                Err(err) => {
                    return err!("consume_many('{}'): {:?}", $name, err);
                }
            };
        }

        deserialized
    }};
}

impl XmileNode for Product {
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
    fn deserialize(handle: &Handle) -> Result<Rc<Self>> {
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
