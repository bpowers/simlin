// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';

import * as React from 'react';
import { act, waitFor } from '@testing-library/react';

// Observation point for the editor's React lifecycle: the sd-model element
// renders into a closed shadow root, so the mounted tree is not reachable via
// DOM queries. The mock instead records props at render time and counts
// mount/cleanup effect runs, which is exactly what the custom element's
// connect/disconnect handling must drive. rs.hoisted lifts this binding
// alongside the hoisted rs.mock call so the factory can name it (a plain
// module-scope const would still be in its temporal dead zone when the factory
// first runs; see docs/dev/typescript.md).
const editorSpy = rs.hoisted(() => ({
  mounts: 0,
  cleanups: 0,
  lastProps: undefined as undefined | { username: string; projectName: string; embedded: boolean; baseURL: string },
}));

rs.mock('@simlin/diagram/HostedWebEditor', () => {
  function HostedWebEditor(props: { username: string; projectName: string; embedded: boolean; baseURL: string }) {
    editorSpy.lastProps = props;
    React.useEffect(() => {
      editorSpy.mounts += 1;
      return () => {
        editorSpy.cleanups += 1;
      };
    }, []);
    return React.createElement('div', { 'data-testid': 'hosted-editor' }, `${props.username}/${props.projectName}`);
  }
  return { HostedWebEditor };
});

import '../index-component';

// Replaces the element's closed shadow roots with plain divs (one per
// attachShadow call) so tests can query what got rendered. Tests that exercise
// the real "attachShadow throws when a shadow root already exists" behavior
// must NOT use this helper.
function mockShadowRoots(): { fakeRoots: HTMLDivElement[]; attachShadow: ReturnType<typeof rs.spyOn> } {
  const fakeRoots: HTMLDivElement[] = [];
  const attachShadow = rs.spyOn(HTMLElement.prototype, 'attachShadow').mockImplementation(function (this: HTMLElement) {
    const fake = document.createElement('div');
    fakeRoots.push(fake);
    return fake as unknown as ShadowRoot;
  });
  return { fakeRoots, attachShadow };
}

describe('sd-model web component', () => {
  beforeEach(() => {
    editorSpy.mounts = 0;
    editorSpy.cleanups = 0;
    editorSpy.lastProps = undefined;
  });

  afterEach(async () => {
    // Clearing the body disconnects any still-attached sd-model elements,
    // which (post-fix) unmounts their React roots; run it inside act so those
    // teardown-driven updates are flushed rather than warned about.
    await act(async () => {
      document.body.innerHTML = '';
    });
    document.head.innerHTML = '';
    rs.restoreAllMocks();
  });

  it('loads its component stylesheet inside the shadow tree', async () => {
    const { fakeRoots } = mockShadowRoots();

    const element = document.createElement('sd-model');
    await act(async () => {
      document.body.appendChild(element);
    });

    await waitFor(() => {
      expect(
        fakeRoots[0]?.querySelector('link[href="https://app.simlin.com/static/css/sd-component.css"]'),
      ).not.toBeNull();
    });
  });

  it('renders the editor with the element attribute values on connect', async () => {
    mockShadowRoots();

    const element = document.createElement('sd-model');
    element.setAttribute('username', 'alice');
    element.setAttribute('projectName', 'growth');
    await act(async () => {
      document.body.appendChild(element);
    });

    await waitFor(() => {
      expect(editorSpy.lastProps).toMatchObject({ username: 'alice', projectName: 'growth', embedded: true });
    });
  });

  it('unmounts the React tree when the element is disconnected', async () => {
    mockShadowRoots();

    const element = document.createElement('sd-model');
    await act(async () => {
      document.body.appendChild(element);
    });
    await waitFor(() => {
      expect(editorSpy.mounts).toBe(1);
    });

    await act(async () => {
      element.remove();
    });

    // Without a disconnectedCallback the root (and every resource behind the
    // editor) stays alive forever after the host removes the element.
    expect(editorSpy.cleanups).toBe(1);
  });

  it('survives disconnect followed by reconnect of the same element', async () => {
    // Real attachShadow: reconnect must not call it a second time (that throws
    // NotSupportedError -- the crash from issue #931).
    const element = document.createElement('sd-model');
    element.setAttribute('username', 'alice');
    await act(async () => {
      document.body.appendChild(element);
    });
    await waitFor(() => {
      expect(editorSpy.mounts).toBe(1);
    });

    await act(async () => {
      element.remove();
    });
    await act(async () => {
      document.body.appendChild(element);
    });

    await waitFor(() => {
      expect(editorSpy.mounts).toBe(2);
    });
    expect(editorSpy.cleanups).toBe(1);
  });

  it('reparenting a connected element (synchronous disconnect + connect) remounts cleanly', async () => {
    // appendChild of an already-connected node fires disconnectedCallback and
    // then connectedCallback synchronously within the one DOM operation -- the
    // SPA "move/reorder" case from issue #931.
    const element = document.createElement('sd-model');
    await act(async () => {
      document.body.appendChild(element);
    });
    await waitFor(() => {
      expect(editorSpy.mounts).toBe(1);
    });

    const container = document.createElement('div');
    document.body.appendChild(container);
    await act(async () => {
      container.appendChild(element);
    });

    await waitFor(() => {
      expect(editorSpy.mounts).toBe(2);
    });
    expect(editorSpy.cleanups).toBe(1);
  });

  it('reconnect renders with attribute values changed while detached', async () => {
    const element = document.createElement('sd-model');
    element.setAttribute('username', 'alice');
    element.setAttribute('projectName', 'growth');
    await act(async () => {
      document.body.appendChild(element);
    });
    await waitFor(() => {
      expect(editorSpy.lastProps?.username).toBe('alice');
    });

    await act(async () => {
      element.remove();
    });
    element.setAttribute('username', 'bob');
    element.setAttribute('projectName', 'decay');
    await act(async () => {
      document.body.appendChild(element);
    });

    await waitFor(() => {
      expect(editorSpy.lastProps).toMatchObject({ username: 'bob', projectName: 'decay' });
    });
    // Exactly one mount per connect: the detached setAttribute calls must not
    // have mounted an editor into the detached shadow root (which the
    // reconnect's own mount would then leak).
    expect(editorSpy.mounts).toBe(2);
    expect(editorSpy.cleanups).toBe(1);
  });

  it('ignores same-value attribute writes while connected', async () => {
    mockShadowRoots();

    const element = document.createElement('sd-model');
    element.setAttribute('username', 'alice');
    await act(async () => {
      document.body.appendChild(element);
    });
    await waitFor(() => {
      expect(editorSpy.mounts).toBe(1);
    });

    // Per spec attributeChangedCallback fires even when the value did not
    // change; remounting there would tear down the editor and refetch the
    // identical project for a no-op write.
    await act(async () => {
      element.setAttribute('username', 'alice');
    });

    expect(editorSpy.mounts).toBe(1);
    expect(editorSpy.cleanups).toBe(0);
  });

  it('attaches the shadow root only once across reconnects', async () => {
    const { attachShadow } = mockShadowRoots();

    const element = document.createElement('sd-model');
    await act(async () => {
      document.body.appendChild(element);
    });
    await act(async () => {
      element.remove();
    });
    await act(async () => {
      document.body.appendChild(element);
    });

    expect(attachShadow).toHaveBeenCalledTimes(1);
  });

  it('remounts the editor when an attribute changes while connected', async () => {
    // An attribute change is a project SWAP, so it must REMOUNT (old tree torn
    // down, fresh mount): HostedWebEditor loads its project once per mount, so
    // a bare re-render would keep showing the old project's data while
    // save/delete silently target the new identity.
    mockShadowRoots();

    const element = document.createElement('sd-model');
    element.setAttribute('username', 'alice');
    element.setAttribute('projectName', 'growth');
    await act(async () => {
      document.body.appendChild(element);
    });
    await waitFor(() => {
      expect(editorSpy.lastProps?.username).toBe('alice');
    });

    await act(async () => {
      element.setAttribute('username', 'bob');
    });
    await waitFor(() => {
      expect(editorSpy.lastProps?.username).toBe('bob');
    });
    expect(editorSpy.mounts).toBe(2);
    expect(editorSpy.cleanups).toBe(1);

    // projectName exercises the case-insensitivity of HTML attributes: the
    // observed attribute list must use the lowercased spelling for this to fire.
    await act(async () => {
      element.setAttribute('projectName', 'decay');
    });
    await waitFor(() => {
      expect(editorSpy.lastProps?.projectName).toBe('decay');
    });
    expect(editorSpy.mounts).toBe(3);
    expect(editorSpy.cleanups).toBe(2);
  });

  it('keeps multiple instances independent', async () => {
    const { fakeRoots } = mockShadowRoots();

    const first = document.createElement('sd-model');
    first.setAttribute('username', 'alice');
    first.setAttribute('projectName', 'growth');
    const second = document.createElement('sd-model');
    second.setAttribute('username', 'bob');
    second.setAttribute('projectName', 'decay');
    await act(async () => {
      document.body.appendChild(first);
      document.body.appendChild(second);
    });

    await waitFor(() => {
      expect(fakeRoots[0]?.textContent).toContain('alice/growth');
      expect(fakeRoots[1]?.textContent).toContain('bob/decay');
    });

    await act(async () => {
      first.remove();
    });

    // Only the removed instance tears down; the other keeps rendering.
    expect(editorSpy.cleanups).toBe(1);
    expect(fakeRoots[1]?.textContent).toContain('bob/decay');
  });

  it('tolerates the module being evaluated twice (duplicate custom element registration)', async () => {
    // SPA hosts can inject the embed script more than once (e.g. a route
    // remount); a second unguarded customElements.define('sd-model', ...)
    // throws NotSupportedError during script evaluation. resetModules + a
    // dynamic import re-runs the module body against the same window registry.
    rs.resetModules();
    await expect(import('../index-component')).resolves.toBeDefined();
  });
});
