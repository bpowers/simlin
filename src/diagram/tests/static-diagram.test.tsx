// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// jsdom does not provide TextEncoder/TextDecoder, but the engine's memory
// module (pulled in transitively via StaticDiagram's engine import) uses them
// at import time. Polyfill from Node's util before importing anything
// engine-backed.
import { TextEncoder, TextDecoder } from 'util';
Object.assign(globalThis, { TextEncoder, TextDecoder });

import * as React from 'react';
import { render, act, waitFor } from '@testing-library/react';

import { projectFromJson, Project } from '@simlin/core/datamodel';
import type { JsonProject } from '@simlin/engine';

import { validProjectJson } from './fake-engine';

// The no-SSR-project path calls EngineProject.openProtobuf() and awaits it. We
// never want the real WASM engine in this unit test, so mock openProtobuf with
// a controllable deferred: tests that exercise the resolved-load path settle it
// with a fake engine-project (serializeJson + dispose); tests that exercise the
// pending / cancelled-guard path leave it unsettled. A module-level `pending`
// box lets each test resolve the in-flight open explicitly.
interface FakeEngineProject {
  serializeJson(): Promise<string>;
  dispose(): Promise<void>;
}
let pendingOpen: { resolve: (p: FakeEngineProject) => void } | undefined;
let disposeCalls = 0;

jest.mock('@simlin/engine', () => {
  const actual = jest.requireActual('@simlin/engine');
  return {
    ...actual,
    Project: {
      ...actual.Project,
      openProtobuf: jest.fn(
        () =>
          new Promise<FakeEngineProject>((resolve) => {
            pendingOpen = { resolve };
          }),
      ),
    },
  };
});

function makeFakeEngineProject(): FakeEngineProject {
  return {
    serializeJson: () => Promise.resolve(validProjectJson()),
    dispose: () => {
      disposeCalls += 1;
      return Promise.resolve();
    },
  };
}

// StaticDiagram's job is choosing what to render (null until a project exists,
// the SSR data-attach, the dark-theme wrapper) -- not the heavyweight Canvas,
// which needs WASM-backed data and a ResizeObserver jsdom lacks. Stub the
// Canvas so the test exercises StaticDiagram's own branches; record the
// `project` it receives so we can assert the data-attach behavior.
let lastCanvasProject: Project | undefined;
jest.mock('../drawing/Canvas', () => ({
  Canvas: (props: { project: Project }): React.ReactElement => {
    lastCanvasProject = props.project;
    return <svg data-testid="canvas-stub" />;
  },
}));

import { StaticDiagram } from '../StaticDiagram';

function makeProject(): Project {
  return projectFromJson(JSON.parse(validProjectJson()) as JsonProject);
}

beforeEach(() => {
  lastCanvasProject = undefined;
  pendingOpen = undefined;
  disposeCalls = 0;
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
    const errorSpy = jest.spyOn(console, 'error').mockImplementation(() => {});
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
});
