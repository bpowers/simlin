// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import React from 'react';
import { StaticDiagram } from '@simlin/diagram/StaticDiagram';

import populationModel from './population.json';

// The population (logistic growth) model, stored in Simlin's native JSON
// format and simulated in the browser by the WASM engine at view time --
// no precomputed series data. StaticDiagram renders nothing during SSG and
// hydrates client-side once the engine load + base-case run resolve.
export function PopulationModel(): React.ReactElement {
  return <StaticDiagram projectJson={JSON.stringify(populationModel)} simulate={true} />;
}
