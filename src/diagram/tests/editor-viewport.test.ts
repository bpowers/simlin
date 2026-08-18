// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The Editor's viewport contract with a host that remounts it on new project
// bytes while the user is looking (the notebook widget on a kernel push): the
// host reads the committed viewport through `onViewportChange` and hands it
// back to the next mount through `initialViewport`.
//
// `onViewportChange` is a post-commit effect keyed on the controller snapshot,
// so it is driven here the way the real controller drives it -- by publishing
// snapshots through the subscription -- and asserted on the host callback: it
// fires for the first snapshot that has a view, again whenever the viewed
// model's viewBox/zoom changes by value (a settled pan, the mount fit, module
// navigation), and NOT for a republished content-equal view (a content edit, a
// save ack) -- the widget would otherwise chase every snapshot. `initialViewport`
// is asserted as the Editor's controller-config wiring; what the controller
// does with it is project-controller.test.ts's.

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, render } from '@testing-library/react';

import { projectFromJson, type JsonProject, type Project, type StockFlowView } from '@simlin/core/datamodel';
import { mapSet } from '@simlin/core/common';
import * as ProjectControllerModule from '../project-controller';
import { ProjectController, type ProjectSnapshot, type Viewport } from '../project-controller';

// Capture the Canvas props: the offscreen-recenter opt-out is decided here
// from `initialViewport` and asserted below.
let capturedCanvasProps: { recenterOffscreenOnMount?: boolean } | undefined;
rs.mock('../drawing/Canvas', () => ({
  __esModule: true,
  Canvas: (p: { recenterOffscreenOnMount?: boolean }) => {
    capturedCanvasProps = p;
    return null;
  },
  inCreationUid: -2,
}));

import { Editor, type EditorProps } from '../Editor';

const projectJson = JSON.stringify({
  name: 'test',
  simSpecs: { startTime: 0, endTime: 10, dt: '1' },
  models: [
    {
      name: 'main',
      stocks: [],
      flows: [],
      auxiliaries: [{ name: 'a', equation: '1' }],
      views: [
        {
          elements: [{ type: 'aux', uid: 1, name: 'a', x: 0, y: 0 }],
          viewBox: { x: 0, y: 0, width: 800, height: 600 },
          zoom: 1,
        },
      ],
    },
    {
      name: 'child',
      stocks: [],
      flows: [],
      auxiliaries: [],
      views: [{ elements: [], viewBox: { x: 5, y: 6, width: 300, height: 200 }, zoom: 2 }],
    },
  ],
});

function baseProject(): Project {
  return projectFromJson(JSON.parse(projectJson) as JsonProject);
}

function withViewport(project: Project, modelName: string, viewport: Viewport): Project {
  const model = project.models.get(modelName)!;
  const view: StockFlowView = { ...model.views[0], viewBox: viewport.viewBox, zoom: viewport.zoom };
  return { ...project, models: mapSet(project.models, modelName, { ...model, views: [view] }) };
}

function makeSnapshot(project: Project | undefined, overrides: Partial<ProjectSnapshot> = {}): ProjectSnapshot {
  return {
    project,
    projectVersion: 1,
    serverVersion: 1,
    projectGeneration: 0,
    status: 'ok',
    cachedErrors: { simError: undefined, modelErrors: [], varErrors: new Map(), unitErrors: new Map() },
    data: new Map(),
    modelName: 'main',
    modelStack: [],
    canUndo: false,
    canRedo: false,
    navResetSeq: 0,
    ...overrides,
  } as unknown as ProjectSnapshot;
}

function makeProps(overrides: Partial<EditorProps> = {}): EditorProps {
  return {
    inputFormat: 'json',
    initialProjectJson: projectJson,
    initialProjectVersion: 1,
    name: 'test',
    onSave: async () => 1,
    ...overrides,
  } as EditorProps;
}

describe('Editor onViewportChange (post-commit effect)', () => {
  let snapshot: ProjectSnapshot;
  let listener: (() => void) | undefined;

  beforeEach(() => {
    listener = undefined;
    // Before the engine has opened there is no project (and so no view).
    snapshot = makeSnapshot(undefined);
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockImplementation(() => snapshot);
    rs.spyOn(ProjectController.prototype, 'subscribe').mockImplementation((l: () => void) => {
      listener = l;
      return () => {
        listener = undefined;
      };
    });
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
  });

  afterEach(() => {
    rs.restoreAllMocks();
  });

  function publish(next: ProjectSnapshot): void {
    snapshot = next;
    act(() => {
      listener?.();
    });
  }

  it('fires once the first snapshot with a view commits (not before), with the model name and its stored viewport', () => {
    const onViewportChange = rs.fn<(modelName: string, viewport: Viewport) => void>();
    act(() => {
      render(React.createElement(Editor, makeProps({ onViewportChange })));
    });
    // No project yet: nothing to report.
    expect(onViewportChange).not.toHaveBeenCalled();

    publish(makeSnapshot(baseProject()));
    expect(onViewportChange).toHaveBeenCalledTimes(1);
    expect(onViewportChange).toHaveBeenCalledWith('main', {
      viewBox: { x: 0, y: 0, width: 800, height: 600 },
      zoom: 1,
    });
  });

  it('fires again for a viewport that changed by value, and stays quiet for a republished content-equal view', () => {
    const onViewportChange = rs.fn<(modelName: string, viewport: Viewport) => void>();
    act(() => {
      render(React.createElement(Editor, makeProps({ onViewportChange })));
    });
    publish(makeSnapshot(baseProject()));
    onViewportChange.mockClear();

    // A content edit / save ack republishes a NEW project object whose view has
    // the same viewport: no notification.
    publish(makeSnapshot(baseProject(), { projectVersion: 1.01, projectGeneration: 1 }));
    publish(makeSnapshot(baseProject(), { projectVersion: 1.02, serverVersion: 2 }));
    expect(onViewportChange).not.toHaveBeenCalled();

    // A settled pan: the offset moved.
    const panned = { viewBox: { x: -40, y: 25, width: 800, height: 600 }, zoom: 1 };
    publish(makeSnapshot(withViewport(baseProject(), 'main', panned), { projectVersion: 1.021 }));
    expect(onViewportChange).toHaveBeenCalledTimes(1);
    expect(onViewportChange).toHaveBeenLastCalledWith('main', panned);

    // A zoom at the same offset.
    const zoomed = { ...panned, zoom: 1.5 };
    publish(makeSnapshot(withViewport(baseProject(), 'main', zoomed), { projectVersion: 1.022 }));
    expect(onViewportChange).toHaveBeenCalledTimes(2);
    expect(onViewportChange).toHaveBeenLastCalledWith('main', zoomed);

    // A resize: only the size changed.
    const resized = { ...zoomed, viewBox: { ...zoomed.viewBox, width: 1000, height: 700 } };
    publish(makeSnapshot(withViewport(baseProject(), 'main', resized), { projectVersion: 1.023 }));
    expect(onViewportChange).toHaveBeenCalledTimes(3);
    expect(onViewportChange).toHaveBeenLastCalledWith('main', resized);

    // The value the host got is a copy: mutating it cannot poison the guard.
    const reported = onViewportChange.mock.calls[2][1];
    (reported.viewBox as { x: number }).x = 999;
    publish(makeSnapshot(withViewport(baseProject(), 'main', resized), { projectVersion: 1.024 }));
    expect(onViewportChange).toHaveBeenCalledTimes(3);
  });

  it('reports the viewed model on module navigation: the child model and its viewport, then the parent again', () => {
    const onViewportChange = rs.fn<(modelName: string, viewport: Viewport) => void>();
    act(() => {
      render(React.createElement(Editor, makeProps({ onViewportChange })));
    });
    publish(makeSnapshot(baseProject()));
    onViewportChange.mockClear();

    publish(makeSnapshot(baseProject(), { modelName: 'child' }));
    expect(onViewportChange).toHaveBeenCalledTimes(1);
    expect(onViewportChange).toHaveBeenLastCalledWith('child', {
      viewBox: { x: 5, y: 6, width: 300, height: 200 },
      zoom: 2,
    });

    // Back to main at its (unchanged) viewport: the model changed, so it fires.
    publish(makeSnapshot(baseProject(), { modelName: 'main' }));
    expect(onViewportChange).toHaveBeenCalledTimes(2);
    expect(onViewportChange).toHaveBeenLastCalledWith('main', {
      viewBox: { x: 0, y: 0, width: 800, height: 600 },
      zoom: 1,
    });
  });

  it('is optional: snapshots with changing viewports commit without a callback', () => {
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    expect(() => {
      publish(makeSnapshot(baseProject()));
      publish(
        makeSnapshot(withViewport(baseProject(), 'main', { viewBox: { x: 1, y: 2, width: 3, height: 4 }, zoom: 2 })),
      );
    }).not.toThrow();
  });
});

describe('Editor initialViewport (controller-config wiring)', () => {
  afterEach(() => {
    rs.restoreAllMocks();
  });

  type ControllerConfig = ConstructorParameters<typeof ProjectControllerModule.ProjectController>[0];

  // Every mount constructs one controller; capture the configs in order.
  function captureConfigs(propsList: EditorProps[]): ControllerConfig[] {
    rs.spyOn(ProjectControllerModule.ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectControllerModule.ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectControllerModule.ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    const captured: ControllerConfig[] = [];
    const real = ProjectControllerModule.ProjectController;
    rs.spyOn(ProjectControllerModule, 'ProjectController').mockImplementation((config: ControllerConfig) => {
      captured.push(config);
      return new real(config);
    });
    for (const props of propsList) {
      act(() => {
        render(React.createElement(Editor, props));
      });
    }
    if (captured.length !== propsList.length) {
      throw new Error(`expected ${propsList.length} controllers, got ${captured.length}`);
    }
    return captured;
  }

  it('forwards initialViewport to the controller, and leaves it unset when the host passes none', () => {
    const viewport = { viewBox: { x: -10, y: 20, width: 640, height: 480 }, zoom: 1.25 };
    const [withViewport, without] = captureConfigs([makeProps({ initialViewport: viewport }), makeProps()]);
    expect(withViewport.initialViewport).toEqual(viewport);
    expect(without.initialViewport).toBeUndefined();
  });

  it('a carried viewport turns the Canvas offscreen re-center off; a stored one keeps it (issue #52 safety net)', () => {
    rs.spyOn(ProjectController.prototype, 'getSnapshot').mockImplementation(() => makeSnapshot(baseProject()));
    rs.spyOn(ProjectController.prototype, 'openInitialProject').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'dispose').mockResolvedValue(undefined);
    rs.spyOn(ProjectController.prototype, 'scheduleSimRun').mockImplementation(() => {});
    rs.spyOn(ProjectController.prototype, 'subscribe').mockReturnValue(() => {});

    capturedCanvasProps = undefined;
    act(() => {
      render(
        React.createElement(
          Editor,
          makeProps({ initialViewport: { viewBox: { x: -5000, y: -5000, width: 800, height: 600 }, zoom: 1 } }),
        ),
      );
    });
    expect(capturedCanvasProps?.recenterOffscreenOnMount).toBe(false);

    capturedCanvasProps = undefined;
    act(() => {
      render(React.createElement(Editor, makeProps()));
    });
    expect(capturedCanvasProps?.recenterOffscreenOnMount).toBe(true);
  });
});
