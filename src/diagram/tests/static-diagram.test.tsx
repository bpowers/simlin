// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, test, expect, beforeEach, rs } from '@rstest/core';

import * as React from 'react';
import { render, act, waitFor } from '@testing-library/react';

import { projectFromJson, Project } from '@simlin/core/datamodel';
import type { JsonProject } from '@simlin/engine';

// The mock factory is hoisted above the imports, so it cannot close over an
// ordinary import binding. This attribute is rstest's synchronous stand-in for
// jest.requireActual: it resolves to the unmocked module and hoists with the
// factory, letting us keep every export but openProtobuf.
import * as actualEngine from '@simlin/engine' with { rstest: 'importActual' };

import { validProjectJson } from './fake-engine';

// The no-SSR-project path calls EngineProject.openProtobuf() (or openJson())
// and awaits it. We never want the real WASM engine in this unit test, so mock
// both open functions with a controllable deferred: tests that exercise the
// resolved-load path settle it with a fake engine-project (serializeJson +
// dispose); tests that exercise the pending / cancelled-guard path leave it
// unsettled. A module-level `pending` box lets each test resolve the in-flight
// open explicitly, and records which open function was invoked.
interface FakeEngineProject {
  serializeJson(): Promise<string>;
  mainModel(): Promise<{ run(): Promise<{ results: ReadonlyMap<string, Float64Array> }> }>;
  dispose(): Promise<void>;
}
let pendingOpen: { via: 'protobuf' | 'json'; resolve: (p: FakeEngineProject) => void } | undefined;
let disposeCalls = 0;

rs.mock('@simlin/engine', () => ({
  ...actualEngine,
  Project: {
    ...actualEngine.Project,
    openProtobuf: rs.fn(
      () =>
        new Promise<FakeEngineProject>((resolve) => {
          pendingOpen = { via: 'protobuf', resolve };
        }),
    ),
    openJson: rs.fn(
      () =>
        new Promise<FakeEngineProject>((resolve) => {
          pendingOpen = { via: 'json', resolve };
        }),
    ),
  },
}));

let runCalls = 0;

function makeFakeEngineProject(opts: { json?: string; runResults?: ReadonlyMap<string, Float64Array> } = {}) {
  const project: FakeEngineProject = {
    serializeJson: () => Promise.resolve(opts.json ?? validProjectJson()),
    mainModel: () =>
      Promise.resolve({
        run: () => {
          runCalls += 1;
          return Promise.resolve({ results: opts.runResults ?? new Map<string, Float64Array>() });
        },
      }),
    dispose: () => {
      disposeCalls += 1;
      return Promise.resolve();
    },
  };
  return project;
}

// StaticDiagram's job is choosing what to render (null until a project exists,
// the SSR data-attach, the dark-theme wrapper) -- not the heavyweight Canvas,
// which needs WASM-backed data and a ResizeObserver jsdom lacks. Stub the
// Canvas so the test exercises StaticDiagram's own branches; record the
// `project` it receives so we can assert the data-attach behavior.
let lastCanvasProject: Project | undefined;
rs.mock('../drawing/Canvas', () => ({
  Canvas: (props: { project: Project }): React.ReactElement => {
    lastCanvasProject = props.project;
    return <svg data-testid="canvas-stub" />;
  },
}));

import { StaticDiagram, runResultsToSeries } from '../StaticDiagram';

function makeProject(): Project {
  return projectFromJson(JSON.parse(validProjectJson()) as JsonProject);
}

// A minimal project whose main model has one auxiliary, for asserting that
// simulate-attached series land on the variable Canvas renders.
function auxProjectJson(): string {
  return JSON.stringify({
    name: 'sim-test',
    simSpecs: { startTime: 0, endTime: 2, dt: '1' },
    models: [
      {
        name: 'main',
        stocks: [],
        flows: [],
        auxiliaries: [{ name: 'growth rate', equation: '1' }],
        views: [{ elements: [] }],
      },
    ],
  });
}

beforeEach(() => {
  lastCanvasProject = undefined;
  pendingOpen = undefined;
  disposeCalls = 0;
  runCalls = 0;
});

describe('StaticDiagram', () => {
  test('renders nothing while no project is available (no SSR project, async load pending)', () => {
    // With no pre-loaded `project` prop the component starts with an undefined
    // project and kicks off an async load; the synchronous render is null. We
    // pass an empty base64 string so the (async, WASM-backed) load never
    // produces output during this synchronous assertion.
    const { container } = render(<StaticDiagram projectPbBase64="" />);
    expect(container.firstChild).toBeNull();
  });

  test('renders the canvas synchronously when a pre-loaded project is supplied (SSR path)', () => {
    const project = makeProject();
    const { getByTestId } = render(<StaticDiagram projectPbBase64="" project={project} />);
    expect(getByTestId('canvas-stub')).not.toBeNull();
  });

  test('wraps the diagram in a dark-theme container when isDarkTheme is set', () => {
    const project = makeProject();
    const { container } = render(<StaticDiagram projectPbBase64="" project={project} isDarkTheme={true} />);
    expect(container.querySelector('[data-theme="dark"]')).not.toBeNull();
  });

  test('does not set a dark-theme attribute when isDarkTheme is unset', () => {
    const project = makeProject();
    const { container } = render(<StaticDiagram projectPbBase64="" project={project} />);
    expect(container.querySelector('[data-theme="dark"]')).toBeNull();
  });

  test('passes the pre-loaded project straight through to Canvas when no data is attached', () => {
    const project = makeProject();
    render(<StaticDiagram projectPbBase64="" project={project} />);
    // No `data` prop means the project is rendered as-is (referential identity
    // preserved -- projectAttachData is not invoked).
    expect(lastCanvasProject).toBe(project);
  });

  test('attaches series data to the SSR project before rendering Canvas', () => {
    const project = makeProject();
    render(<StaticDiagram projectPbBase64="" project={project} data={new Map()} />);
    // With a `data` prop the constructor runs projectAttachData, which returns a
    // new Project value -- so Canvas sees a different object than the input.
    expect(lastCanvasProject).not.toBe(project);
    expect(lastCanvasProject).not.toBeUndefined();
  });

  test('the async load lands the project in Canvas after the engine open resolves', async () => {
    // No SSR project: the mount effect opens the engine and awaits it. Nothing
    // renders until we settle the deferred open with a fake engine-project.
    const { container } = render(<StaticDiagram projectPbBase64="" />);
    expect(container.firstChild).toBeNull();

    await waitFor(() => expect(pendingOpen).not.toBeUndefined());
    await act(async () => {
      pendingOpen!.resolve(makeFakeEngineProject());
    });

    // The post-await setProject ran: Canvas now has the loaded project and the
    // engine was disposed.
    await waitFor(() => expect(lastCanvasProject).not.toBeUndefined());
    expect(disposeCalls).toBe(1);
  });

  test('unmounting before the load resolves runs the cancelled guard (no setProject warning)', async () => {
    const errorSpy = rs.spyOn(console, 'error').mockImplementation(() => {});
    try {
      const { unmount } = render(<StaticDiagram projectPbBase64="" />);
      await waitFor(() => expect(pendingOpen).not.toBeUndefined());

      // Unmount while the open is still in flight; the effect cleanup sets
      // `cancelled = true`.
      unmount();

      // Now resolve the open. The continuation awaits serializeJson/dispose and
      // then hits the `if (!cancelled)` guard, so setProject is skipped and
      // React logs no "update on an unmounted component" warning.
      await act(async () => {
        pendingOpen!.resolve(makeFakeEngineProject());
      });

      expect(lastCanvasProject).toBeUndefined();
      const sawUnmountWarning = errorSpy.mock.calls.some((args) =>
        args.some((a) => typeof a === 'string' && a.includes('unmounted')),
      );
      expect(sawUnmountWarning).toBe(false);
    } finally {
      errorSpy.mockRestore();
    }
  });

  test('a projectJson source loads through Project.openJson, not openProtobuf', async () => {
    const { container } = render(<StaticDiagram projectJson={validProjectJson()} />);
    expect(container.firstChild).toBeNull();

    await waitFor(() => expect(pendingOpen).not.toBeUndefined());
    expect(pendingOpen!.via).toBe('json');

    await act(async () => {
      pendingOpen!.resolve(makeFakeEngineProject());
    });

    await waitFor(() => expect(lastCanvasProject).not.toBeUndefined());
    expect(disposeCalls).toBe(1);
  });

  test('simulate runs the model and attaches the results as series data', async () => {
    const time = new Float64Array([0, 1, 2]);
    const values = new Float64Array([1, 1, 1]);
    const results = new Map<string, Float64Array>([
      ['time', time],
      ['growth_rate', values],
    ]);

    render(<StaticDiagram projectJson={auxProjectJson()} simulate={true} />);
    await waitFor(() => expect(pendingOpen).not.toBeUndefined());
    await act(async () => {
      pendingOpen!.resolve(makeFakeEngineProject({ json: auxProjectJson(), runResults: results }));
    });

    await waitFor(() => expect(lastCanvasProject).not.toBeUndefined());
    expect(runCalls).toBe(1);
    expect(disposeCalls).toBe(1);

    const model = lastCanvasProject!.models.get('main');
    const variable = model!.variables.get('growth_rate');
    expect(variable).not.toBeUndefined();
    expect(variable!.data).not.toBeUndefined();
    expect(variable!.data!.length).toBe(1);
    expect(Array.from(variable!.data![0].values)).toEqual([1, 1, 1]);
  });

  test('an explicit data prop wins over simulate (the model is not run)', async () => {
    render(<StaticDiagram projectJson={auxProjectJson()} simulate={true} data={new Map()} />);
    await waitFor(() => expect(pendingOpen).not.toBeUndefined());
    await act(async () => {
      pendingOpen!.resolve(makeFakeEngineProject({ json: auxProjectJson() }));
    });

    await waitFor(() => expect(lastCanvasProject).not.toBeUndefined());
    expect(runCalls).toBe(0);
  });

  test('a failed simulation still renders the diagram, without series data', async () => {
    const errorSpy = rs.spyOn(console, 'error').mockImplementation(() => {});
    try {
      const failing = makeFakeEngineProject({ json: auxProjectJson() });
      failing.mainModel = () => Promise.reject(new Error('model has errors'));

      render(<StaticDiagram projectJson={auxProjectJson()} simulate={true} />);
      await waitFor(() => expect(pendingOpen).not.toBeUndefined());
      await act(async () => {
        pendingOpen!.resolve(failing);
      });

      await waitFor(() => expect(lastCanvasProject).not.toBeUndefined());
      expect(disposeCalls).toBe(1);
      const variable = lastCanvasProject!.models.get('main')!.variables.get('growth_rate');
      expect(variable!.data).toBeUndefined();
    } finally {
      errorSpy.mockRestore();
    }
  });
});

describe('runResultsToSeries', () => {
  test('builds a Series per result keyed by name, sharing the time array', () => {
    const time = new Float64Array([0, 1]);
    const population = new Float64Array([5, 6]);
    const series = runResultsToSeries(
      new Map<string, Float64Array>([
        ['time', time],
        ['population', population],
      ]),
    );

    expect(series.size).toBe(2);
    const pop = series.get('population')!;
    expect(pop.name).toBe('population');
    expect(pop.time).toBe(time);
    expect(pop.values).toBe(population);
  });

  test('throws when the results lack a time series', () => {
    expect(() => runResultsToSeries(new Map([['population', new Float64Array([1])]]))).toThrow(/time/);
  });
});
