// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What the tools' tests share: a host's side of a call (a project, the db
//! synced to it the way every host syncs one, a revision) and the fixtures.

use serde_json::Value;

use crate::datamodel::{self, Project};
use crate::db::SimlinDb;
use crate::test_common::TestProject;

use super::{Session, ToolOutput, Workspace};

/// A project as a host holds one for tool calls: the datamodel, a db synced to
/// it with `SimlinDb::sync` (what libsimlin's open functions do), and the
/// revision the host would report.
pub(crate) struct Host {
    pub project: Project,
    pub db: SimlinDb,
    pub revision: u64,
}

impl Host {
    pub fn new(project: Project) -> Host {
        let mut db = SimlinDb::default();
        db.sync(&project);
        Host {
            project,
            db,
            revision: 0,
        }
    }

    pub fn from_test_project(project: &TestProject) -> Host {
        Host::new(project.build_datamodel())
    }

    /// Replace the project, re-sync the db incrementally and advance the
    /// revision: what a host does when an edit lands.
    pub fn edit(&mut self, change: impl FnOnce(&mut Project)) {
        change(&mut self.project);
        self.db.sync(&self.project);
        self.revision += 1;
    }

    /// Call a tool as a host does: with the project's contents held for the
    /// call, and an edit the call made (`ToolOutput::edited`) made the
    /// project's contents in one edit, the db synced to it.
    pub fn call_raw(&mut self, session: &mut Session, tool: &str, input: &str) -> ToolOutput {
        let output = session
            .call(self.workspace(), tool, input)
            .expect("the catalog lists the tool");
        self.commit(&output);
        output
    }

    /// Make the edit a call made, if it made one, the project's contents.
    pub fn commit(&mut self, output: &ToolOutput) {
        if let Some(edited) = &output.edited {
            self.edit(|p| *p = edited.clone());
        }
    }

    /// Call a tool that must succeed, and parse its output.
    pub fn call(&mut self, session: &mut Session, tool: &str, input: Value) -> Value {
        let output = self.call_raw(session, tool, &input.to_string());
        assert!(!output.is_error, "{tool} refused {input}: {}", output.json);
        serde_json::from_str(&output.json).expect("tool output is JSON")
    }

    /// Call a tool while other work waits for the project, as a host whose
    /// person edits during the call reports, and parse the answer.
    pub fn call_waited_on(&mut self, session: &mut Session, tool: &str, input: Value) -> Value {
        let output = self.call_waiting(session, tool, input.clone(), &|| true);
        assert!(output.is_error, "{tool} answered {input}: {}", output.json);
        serde_json::from_str(&output.json).expect("refusals are JSON")
    }

    /// Call a tool with `waiting` as the host's report of other work waiting
    /// for the project.
    pub fn call_waiting(
        &mut self,
        session: &mut Session,
        tool: &str,
        input: Value,
        waiting: &(dyn Fn() -> bool + Sync),
    ) -> ToolOutput {
        let ws = Workspace {
            project: &self.project,
            db: &mut self.db,
            revision: self.revision,
            waiting: Some(waiting),
            cancelled: None,
        };
        let output = session
            .call(ws, tool, &input.to_string())
            .expect("the catalog lists the tool");
        self.commit(&output);
        output
    }

    /// Call a tool with `cancelled` as the host's report that it cancelled
    /// the call, and `waiting` as its report of other work waiting for the
    /// project.
    pub fn call_cancelled(
        &mut self,
        session: &mut Session,
        tool: &str,
        input: Value,
        cancelled: &(dyn Fn() -> bool + Sync),
        waiting: &(dyn Fn() -> bool + Sync),
    ) -> ToolOutput {
        let ws = Workspace {
            cancelled: Some(cancelled),
            waiting: Some(waiting),
            ..self.workspace()
        };
        let output = session
            .call(ws, tool, &input.to_string())
            .expect("the catalog lists the tool");
        self.commit(&output);
        output
    }

    /// A report of other work that begins to wait once the call has asked
    /// `after` times: what a person's edit that arrives mid-call looks like.
    pub fn waiting_after(after: usize) -> impl Fn() -> bool + Sync {
        let asked = std::sync::atomic::AtomicUsize::new(0);
        move || asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= after
    }

    /// Call a tool that must refuse, and parse its refusal.
    pub fn refuse(&mut self, session: &mut Session, tool: &str, input: Value) -> Value {
        let output = self.call_raw(session, tool, &input.to_string());
        assert!(output.is_error, "{tool} answered {input}: {}", output.json);
        serde_json::from_str(&output.json).expect("refusals are JSON")
    }

    pub fn workspace(&mut self) -> Workspace<'_> {
        Workspace {
            project: &self.project,
            db: &mut self.db,
            revision: self.revision,
            waiting: None,
            cancelled: None,
        }
    }
}

/// `project` with a diagram of its first model, laid out by the engine, so a
/// test can make an edit that changes only the diagram.
pub(crate) fn with_diagram(mut project: Project) -> Project {
    let name = project.models[0].name.clone();
    let view = crate::layout::generate_layout(&project, &name, None).expect("the model lays out");
    project.models[0].views = vec![datamodel::View::StockFlow(view)];
    project
}

/// An edit of `project`'s first diagram alone: its zoom, as a pinch leaves it.
pub(crate) fn zoom_the_diagram(project: &mut Project) {
    let datamodel::View::StockFlow(view) = &mut project.models[0].views[0];
    view.zoom = if view.zoom == 2.0 { 1.0 } else { 2.0 };
}

/// An inventory model with a bit of everything an outline lists: stocks with
/// units and flows, a flow with a documented equation, computed auxiliaries, a
/// constant, a lookup-with-input, and a standalone lookup table.
pub(crate) fn inventory() -> TestProject {
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0, 2.0]),
        y_points: vec![0.0, 0.5, 1.0],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    TestProject::new("inventory")
        .with_sim_time(0.0, 20.0, 0.25)
        .with_time_units("month")
        .stock_with_options(
            "Inventory",
            "desired_inventory",
            &["production"],
            &["shipments"],
            Some("widget"),
            "Widgets on hand.",
            true,
            false,
            datamodel::Visibility::Private,
            None,
        )
        .flow(
            "production",
            "MAX(0, orders + (desired_inventory - Inventory) / adjustment_time)",
            Some("widget/month"),
        )
        .flow("shipments", "orders", Some("widget/month"))
        .aux("orders", "10 + STEP(2, 5)", Some("widget/month"))
        .aux("desired_inventory", "orders * coverage", Some("widget"))
        .aux("coverage", "4", Some("month"))
        .aux("adjustment_time", "2", Some("month"))
        .aux_with_gf(
            "effect_of_pressure",
            "Inventory / desired_inventory",
            gf.clone(),
        )
        .aux_with_gf("pressure_table", "", gf)
}

/// A model whose constants are read only as it starts: `s0` by the stock's
/// initial value, `base_rate` inside `INIT`.
pub(crate) fn initial_reads() -> TestProject {
    TestProject::new("initial_reads")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "s0", &["growth"], &[], None)
        .flow("growth", "level * rate", None)
        .aux("rate", "INIT(base_rate * 2)", None)
        .aux("base_rate", "0.05", None)
        .aux("s0", "100", None)
}
