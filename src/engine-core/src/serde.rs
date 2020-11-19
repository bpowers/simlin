// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::{Dt, SimMethod, SimSpecs};
use crate::project_io;

impl From<Dt> for project_io::Dt {
    fn from(dt: Dt) -> Self {
        match dt {
            Dt::Dt(value) => project_io::Dt {
                value,
                is_reciprocal: false,
            },
            Dt::Reciprocal(value) => project_io::Dt {
                value,
                is_reciprocal: true,
            },
        }
    }
}

impl From<project_io::Dt> for Dt {
    fn from(dt: project_io::Dt) -> Self {
        if dt.is_reciprocal {
            Dt::Reciprocal(dt.value)
        } else {
            Dt::Dt(dt.value)
        }
    }
}

#[test]
fn test_dt_roundtrip() {
    let cases: &[Dt] = &[Dt::Dt(7.7), Dt::Reciprocal(7.7)];
    for expected in cases {
        let expected = expected.clone();
        let actual = Dt::from(project_io::Dt::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<i32> for project_io::SimMethod {
    fn from(value: i32) -> Self {
        match value {
            0 => project_io::SimMethod::Euler,
            1 => project_io::SimMethod::RungeKutta4,
            _ => project_io::SimMethod::Euler,
        }
    }
}

impl From<SimMethod> for project_io::SimMethod {
    fn from(sim_method: SimMethod) -> Self {
        match sim_method {
            SimMethod::Euler => project_io::SimMethod::Euler,
            SimMethod::RungeKutta4 => project_io::SimMethod::RungeKutta4,
        }
    }
}

impl From<project_io::SimMethod> for SimMethod {
    fn from(sim_method: project_io::SimMethod) -> Self {
        match sim_method {
            project_io::SimMethod::Euler => SimMethod::Euler,
            project_io::SimMethod::RungeKutta4 => SimMethod::RungeKutta4,
        }
    }
}

#[test]
fn test_sim_method_roundtrip() {
    let cases: &[SimMethod] = &[SimMethod::Euler, SimMethod::RungeKutta4];
    for expected in cases {
        let expected = expected.clone();
        let actual = SimMethod::from(project_io::SimMethod::from(expected.clone()));
        assert_eq!(expected, actual);
    }

    // protobuf enums are open, which we should just treat as Euler
    assert_eq!(
        SimMethod::Euler,
        SimMethod::from(project_io::SimMethod::from(666))
    );
}

impl From<SimSpecs> for project_io::SimSpecs {
    fn from(sim_specs: SimSpecs) -> Self {
        project_io::SimSpecs {
            start: sim_specs.start,
            stop: sim_specs.stop,
            dt: Some(project_io::Dt::from(sim_specs.dt)),
            save_step: match sim_specs.save_step {
                None => None,
                Some(dt) => Some(project_io::Dt::from(dt)),
            },
            sim_method: project_io::SimMethod::from(sim_specs.sim_method) as i32,
            time_units: sim_specs.time_units,
        }
    }
}

impl From<project_io::SimSpecs> for SimSpecs {
    fn from(sim_specs: project_io::SimSpecs) -> Self {
        SimSpecs {
            start: sim_specs.start,
            stop: sim_specs.stop,
            dt: Dt::from(sim_specs.dt.unwrap_or(project_io::Dt {
                value: 1.0,
                is_reciprocal: false,
            })),
            save_step: match sim_specs.save_step {
                Some(dt) => Some(Dt::from(dt)),
                None => None,
            },
            sim_method: SimMethod::from(project_io::SimMethod::from(sim_specs.sim_method)),
            time_units: sim_specs.time_units,
        }
    }
}

#[test]
fn test_sim_specs_roundtrip() {
    let cases: &[SimSpecs] = &[
        SimSpecs {
            start: 127.0,
            stop: 129.9,
            dt: Dt::Reciprocal(4.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("years".to_string()),
        },
        SimSpecs {
            start: 127.0,
            stop: 129.9,
            dt: Dt::Dt(5.0),
            save_step: None,
            sim_method: SimMethod::RungeKutta4,
            time_units: None,
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = SimSpecs::from(project_io::SimSpecs::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}
