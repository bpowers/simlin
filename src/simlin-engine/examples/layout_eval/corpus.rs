// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The curated corpus: which models the sweep lays out, how far each model's
//! shipped diagram can be trusted as a taste anchor, and how models load.

use std::io::BufReader;

use simlin_engine::{datamodel, open_vensim, open_xmile};

/// The model name the layout pipeline and renderer operate on. `Project::get_model`
/// maps "main" to the single/main model (matching `tests/integration/layout.rs`).
pub const MAIN_MODEL: &str = "main";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xmile,
    Vensim,
}

/// How far a model's shipped view can be trusted as a quality exemplar.
///
/// A hand-drawn diagram is ground truth for arrangement, but the metric scores
/// the geometry OUR renderer draws. That matches the author's picture only when
/// the authoring tool draws the way we do, so the anchors are graded rather than
/// treated alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reference {
    /// One view authored in Stella or Simlin, whose conventions our renderer
    /// reproduces (a named circle/box with a side label): its score is directly
    /// comparable to a generated layout's.
    Curated,
    /// One view authored in Vensim. Its arrangement is a trustworthy exemplar,
    /// but Vensim draws a variable AS its wrapped name (no circle, no side
    /// label), so the label geometry our renderer imposes on it is not what the
    /// author saw; label-dependent terms over it are not comparable.
    Imported,
    /// Several Vensim views the importer stacks into one diagram with group
    /// boxes: an exemplar of decomposing a large model, not one comparable
    /// diagram.
    MultiView,
    /// No shipped diagram: the model is only ever laid out by the generator.
    None,
}

/// Size class, for reading results and for running a cheap subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Up to ~20 variables: a textbook model.
    Small,
    /// ~20-100 variables: what an agent or a student builds.
    Medium,
    /// Over ~100 variables: a published research model.
    Large,
}

impl Tier {
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "small" => Some(Tier::Small),
            "medium" => Some(Tier::Medium),
            "large" => Some(Tier::Large),
            _ => None,
        }
    }
}

/// One corpus entry. Paths are relative to `CARGO_MANIFEST_DIR`
/// (`src/simlin-engine`) unless absolute.
#[derive(Clone, Debug)]
pub struct ModelSpec {
    pub key: String,
    pub path: String,
    pub format: Format,
    pub tier: Tier,
    pub reference: Reference,
}

struct Entry {
    key: &'static str,
    rel_path: &'static str,
    format: Format,
    tier: Tier,
    reference: Reference,
}

use Format::{Vensim, Xmile};
use Reference::{Curated, Imported, MultiView};
use Tier::{Large, Medium, Small};

/// The curated corpus. Every entry is verified to exist on disk and load; each
/// earns its place by exercising something the others do not.
const CORPUS: &[Entry] = &[
    // Textbook small models.
    Entry {
        key: "teacup",
        rel_path: "../../test/test-models/samples/teacup/teacup.stmx",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    Entry {
        key: "sir",
        rel_path: "../../test/test-models/samples/SIR/SIR.stmx",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    // default_projects: the app's curated, hand-laid-out built-in projects, the
    // primary taste anchors (and the reference-pair tests' fixtures).
    Entry {
        key: "logistic_growth",
        rel_path: "../../default_projects/logistic-growth/model.xmile",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    Entry {
        key: "population",
        rel_path: "../../default_projects/population/model.xmile",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    Entry {
        key: "fishbanks",
        rel_path: "../../default_projects/fishbanks/model.xmile",
        format: Xmile,
        tier: Medium,
        reference: Curated,
    },
    Entry {
        key: "reliability",
        rel_path: "../../default_projects/reliability/model.xmile",
        format: Xmile,
        tier: Medium,
        reference: Curated,
    },
    // Small and medium single-view Vensim models: the hand arrangement is the
    // exemplar (chains in rows, parameters beside their consumers).
    Entry {
        key: "lotka_volterra",
        rel_path: "../../test/test-models/samples/Lotka_Volterra/Lotka_Volterra.mdl",
        format: Vensim,
        tier: Small,
        reference: Imported,
    },
    Entry {
        key: "workforce",
        rel_path: "../../test/test-models/samples/Workforce/workforce.mdl",
        format: Vensim,
        tier: Medium,
        reference: Imported,
    },
    Entry {
        key: "bathtub",
        rel_path: "../../test/metasd/bathtub-statistics/integration3.mdl",
        format: Vensim,
        tier: Medium,
        reference: Imported,
    },
    Entry {
        key: "catastrophe",
        rel_path: "../../test/metasd/early-warnings-catastrophe/catastropeWarning2.mdl",
        format: Vensim,
        tier: Medium,
        reference: Imported,
    },
    Entry {
        key: "groupon",
        rel_path: "../../test/metasd/social-network-valuation/groupon 1.mdl",
        format: Vensim,
        tier: Medium,
        reference: Imported,
    },
    // Delay and smooth structures (implicit modules behind builtins).
    Entry {
        key: "delays",
        rel_path: "../../test/delays/model.xmile",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    // No shipped diagram: laid out only by the generator, like a model an agent
    // builds from equations.
    Entry {
        key: "arms_race",
        rel_path: "../../test/arms_race_3party/arms_race.stmx",
        format: Xmile,
        tier: Small,
        reference: Reference::None,
    },
    // Modules.
    Entry {
        key: "hares_and_foxes",
        rel_path: "../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    Entry {
        key: "ai_modules_arrays",
        rel_path: "../../test/ai-information/WithModulesAndArrays.stmx",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    // Aliases.
    Entry {
        key: "alias1",
        rel_path: "../../test/alias1/alias1.stmx",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    // Arrays and several flows into one stock.
    Entry {
        key: "cross_element",
        rel_path: "../../test/cross_element_ltm/cross_element.stmx",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    Entry {
        key: "arrayed_pop",
        rel_path: "../../test/arrayed_population_ltm/arrayed_population.stmx",
        format: Xmile,
        tier: Small,
        reference: Curated,
    },
    // An AI-generated model a human then edited: the kind of model the MCP
    // server lays out.
    Entry {
        key: "ai_edited",
        rel_path: "../../test/ai-information/GeneratedByAIThenEdited.stmx",
        format: Xmile,
        tier: Large,
        reference: Curated,
    },
    // Large published Vensim models.
    Entry {
        key: "wrld3_03",
        rel_path: "../../test/metasd/WRLD3-03/wrld3-03.mdl",
        format: Vensim,
        tier: Large,
        reference: MultiView,
    },
    Entry {
        key: "beer_game",
        rel_path: "../../test/metasd/beer-game/RealBeer4-Sterman13.mdl",
        format: Vensim,
        tier: Medium,
        reference: MultiView,
    },
    Entry {
        key: "wonderland",
        rel_path: "../../test/metasd/wonderland/Wonderland3.mdl",
        format: Vensim,
        tier: Medium,
        reference: MultiView,
    },
    Entry {
        key: "mortgage_econ",
        rel_path: "../../test/bobby/vdf/econ/mark2.mdl",
        format: Vensim,
        tier: Medium,
        reference: MultiView,
    },
    Entry {
        key: "land_use",
        rel_path: "../../test/land_model/land_model.stmx",
        format: Xmile,
        tier: Large,
        reference: MultiView,
    },
    // Multi-view Vensim: hand-authored references that decompose a big model
    // into ~25-50-variable views connected by ghost variables -- the exemplars
    // of what a readable layout of a large model looks like.
    Entry {
        key: "scirev",
        rel_path: "../../test/metasd/scientific-revolution/scirev8.mdl",
        format: Vensim,
        tier: Large,
        reference: MultiView,
    },
    Entry {
        key: "thyroid",
        rel_path: "../../test/metasd/thyroid-dynamics/thyroid-2008-d.mdl",
        format: Vensim,
        tier: Large,
        reference: MultiView,
    },
    Entry {
        key: "covid19",
        rel_path: "../../test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl",
        format: Vensim,
        tier: Large,
        reference: MultiView,
    },
    Entry {
        key: "industrial_dynamics",
        rel_path: "../../test/metasd/industrial-dynamics/IDch15/IDch15d.mdl",
        format: Vensim,
        tier: Large,
        reference: MultiView,
    },
];

/// The corpus as owned specs, followed by any ad-hoc `extra` entries
/// (`(key, path)` pairs from `LAYOUT_EVAL_EXTRA`). An extra's format comes from
/// its extension; it is filed as a medium model whose reference (if it ships
/// one) is graded by format, since nothing more is known about it.
pub fn all_specs(extra: &[(String, String)]) -> Vec<ModelSpec> {
    let mut specs: Vec<ModelSpec> = CORPUS
        .iter()
        .map(|e| ModelSpec {
            key: e.key.to_string(),
            path: e.rel_path.to_string(),
            format: e.format,
            tier: e.tier,
            reference: e.reference,
        })
        .collect();
    for (key, path) in extra {
        let format = if path.to_ascii_lowercase().ends_with(".mdl") {
            Format::Vensim
        } else {
            Format::Xmile
        };
        specs.push(ModelSpec {
            key: key.clone(),
            path: path.clone(),
            format,
            tier: Tier::Medium,
            reference: match format {
                Format::Xmile => Reference::Curated,
                Format::Vensim => Reference::Imported,
            },
        });
    }
    specs
}

/// Resolve a spec path: absolute paths as given, relative ones against the
/// crate manifest dir.
fn abs_path(path: &str) -> String {
    if std::path::Path::new(path).is_absolute() {
        path.to_string()
    } else {
        format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path)
    }
}

/// Load one corpus model, dispatching on its declared format. Returns a
/// human-readable error on any I/O or parse failure so the caller can
/// WARN-and-skip.
pub fn load_model(spec: &ModelSpec) -> Result<datamodel::Project, String> {
    let path = abs_path(&spec.path);
    match spec.format {
        Format::Xmile => {
            let file =
                std::fs::File::open(&path).map_err(|e| format!("failed to open {path}: {e}"))?;
            let mut reader = BufReader::new(file);
            open_xmile(&mut reader).map_err(|e| format!("failed to parse {path}: {e:?}"))
        }
        Format::Vensim => {
            let contents = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {path}: {e}"))?;
            open_vensim(&contents).map_err(|e| format!("failed to parse {path}: {e:?}"))
        }
    }
}

/// Borrow the model's as-loaded main `StockFlow` view if it is a non-empty
/// hand-authored diagram; `None` when the model ships no diagram.
pub fn reference_view(project: &datamodel::Project) -> Option<&datamodel::StockFlow> {
    let model = project.get_model(MAIN_MODEL)?;
    match model.views.first() {
        Some(datamodel::View::StockFlow(sf)) if !sf.elements.is_empty() => Some(sf),
        _ => None,
    }
}

/// Number of model variables, the size the tiers describe.
pub fn variable_count(project: &datamodel::Project) -> usize {
    project
        .get_model(MAIN_MODEL)
        .map(|m| m.variables.len())
        .unwrap_or(0)
}
