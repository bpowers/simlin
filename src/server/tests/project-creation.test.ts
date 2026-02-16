// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Project as Project } from '@simlin/engine';
import type { JsonProject } from '@simlin/engine';
import { emptyProject } from '../project-creation';

describe('emptyProject', () => {
  it('creates a valid project that can be deserialized', async () => {
    const projectName = 'Test Project';
    const protobuf = await emptyProject(projectName, 'testuser');

    expect(protobuf).toBeInstanceOf(Uint8Array);
    expect(protobuf.length).toBeGreaterThan(0);

    const project = await Project.openProtobuf(protobuf);
    const json = JSON.parse(await project.serializeJson()) as JsonProject;
    await project.dispose();

    expect(json.name).toBe(projectName);
  });

  it('creates a project with correct simulation specs', async () => {
    const protobuf = await emptyProject('SimSpecs Test', 'testuser');

    // Verify the round-trip: protobuf -> engine -> JSON produces expected structure
    const project = await Project.openProtobuf(protobuf);
    const json = JSON.parse(await project.serializeJson()) as JsonProject;
    await project.dispose();

    expect(json.simSpecs.startTime).toBe(0);
    expect(json.simSpecs.endTime).toBe(100);
    // Engine2 omits dt from JSON output when it equals the default value of 1.
    // We verify this behavior to ensure the project was created with dt=1.
    expect(json.simSpecs.dt).toBeUndefined();
  });

  it('creates a project with a main model', async () => {
    const protobuf = await emptyProject('Model Test', 'testuser');

    const project = await Project.openProtobuf(protobuf);
    const json = JSON.parse(await project.serializeJson()) as JsonProject;
    await project.dispose();

    expect(json.models).toHaveLength(1);
    expect(json.models[0].name).toBe('main');
  });

  it('creates a main model with empty variables', async () => {
    const protobuf = await emptyProject('Variables Test', 'testuser');

    const project = await Project.openProtobuf(protobuf);
    const json = JSON.parse(await project.serializeJson()) as JsonProject;
    await project.dispose();

    // The engine's JSON serializer omits empty arrays, so these are undefined
    // rather than []. Verify no variables are present either way.
    const mainModel = json.models[0];
    expect(mainModel.stocks ?? []).toEqual([]);
    expect(mainModel.flows ?? []).toEqual([]);
    expect(mainModel.auxiliaries ?? []).toEqual([]);
  });

  it('creates a main model with a stock-flow view', async () => {
    const protobuf = await emptyProject('View Test', 'testuser');

    const project = await Project.openProtobuf(protobuf);
    const json = JSON.parse(await project.serializeJson()) as JsonProject;
    await project.dispose();

    const mainModel = json.models[0];
    expect(mainModel.views).toBeDefined();
    expect(mainModel.views).toHaveLength(1);
    expect(mainModel.views![0].kind).toBe('stock_flow');
    expect(mainModel.views![0].elements).toEqual([]);
  });
});
