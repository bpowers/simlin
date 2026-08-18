// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The imperative shell: the AFM lifecycle (initialize/render) and WidgetApp,
 * driven against a fake anywidget model with the Editor and the engine
 * mocked (see rstest.config.mts). The real Editor + engine + bundle run in
 * the Playwright journey under e2e/.
 */

import { describe, it, expect, beforeEach, afterEach, rs } from '@rstest/core';
import { act, fireEvent, screen, waitFor } from '@testing-library/react';

import widget from './index';
import { GLOBAL_KEY, resetEngineBootstrapForTests } from './engine-bootstrap';
import { NOTICE_TIMEOUT_MS, SELECTION_DEBOUNCE_MS } from './WidgetApp';
import { mounts, resetEditorMock } from './test-utils/editor-mock';
import { readyCalls, resetEngineMock } from './test-utils/engine-mock';
import { FakeModel, defaultState } from './test-utils/fake-model';

const EMPTY_WASM = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);

async function seedSharedModule(): Promise<void> {
  const module = await WebAssembly.compile(EMPTY_WASM.buffer.slice(0));
  (globalThis as Record<string, unknown>)[GLOBAL_KEY] = Promise.resolve(module);
}

async function mount(model: FakeModel): Promise<{ el: HTMLElement; cleanup: () => void }> {
  const el = document.createElement('div');
  document.body.appendChild(el);
  let cleanup: () => void = () => undefined;
  await act(async () => {
    cleanup = await widget.render({ model, el });
  });
  return { el, cleanup };
}

describe('AFM lifecycle', () => {
  beforeEach(async () => {
    resetEngineBootstrapForTests();
    resetEngineMock();
    resetEditorMock();
    await seedSharedModule();
  });
  afterEach(() => {
    resetEngineBootstrapForTests();
    document.body.innerHTML = '';
  });

  it('exports the anywidget default-export shape', () => {
    expect(typeof widget.initialize).toBe('function');
    expect(typeof widget.render).toBe('function');
  });

  it('initialize starts the engine bootstrap without blocking; render mounts the Editor into el', async () => {
    const model = new FakeModel(defaultState());
    const result = widget.initialize({ model });
    expect(result).toBeUndefined();
    const { el, cleanup } = await mount(model);

    const editor = el.querySelector('[data-testid="editor-mock"]');
    expect(editor).not.toBeNull();
    expect(editor?.getAttribute('data-initial-json')).toBe('{"name":"p"}');
    expect(editor?.getAttribute('data-initial-version')).toBe('3');
    expect(readyCalls).toHaveLength(1);
    // Nothing was requested from the kernel: the page-wide module was cached.
    expect(model.sent).toEqual([]);

    const wrapper = el.querySelector('[data-lm-suppress-shortcuts]') as HTMLElement;
    expect(wrapper).not.toBeNull();
    expect(wrapper.style.height).toBe('400px');
    expect(wrapper.style.width).toBe('100%');
    expect(wrapper.style.position).toBe('relative');
    expect(wrapper.getAttribute('data-theme')).toBe('light');
    expect(mounts).toHaveLength(1);
    expect(mounts[0].props.name).toBe('model');
    expect(mounts[0].props.inputFormat).toBe('json');

    cleanup();
    expect(el.childElementCount).toBe(0);
    // Every model listener the view added is gone.
    expect(model.listenerCount('change:revision')).toBe(0);
    expect(model.listenerCount('change:notice')).toBe(0);
  });

  it('render shows the failure in the cell when the engine cannot be obtained', async () => {
    resetEngineBootstrapForTests();
    const model = new FakeModel(defaultState());
    const el = document.createElement('div');
    document.body.appendChild(el);
    let rendering: Promise<() => void> | undefined;
    await act(async () => {
      rendering = widget.render({ model, el });
      // The request went out; the kernel says no.
      expect(model.sent).toEqual([{ type: 'wasm' }]);
      model.trigger('msg:custom', { type: 'wasm', error: 'wheel is missing the asset' }, []);
      await rendering;
    });
    const status = el.querySelector('[role="status"]');
    expect(status?.textContent).toContain('wheel is missing the asset');
    expect(el.querySelector('[data-testid="editor-mock"]')).toBeNull();
  });

  it('render does not mount when the view was aborted while the engine loaded', async () => {
    const model = new FakeModel(defaultState());
    const el = document.createElement('div');
    document.body.appendChild(el);
    const controller = new AbortController();
    controller.abort();
    await act(async () => {
      await widget.render({ model, el, signal: controller.signal });
    });
    expect(el.childElementCount).toBe(0);
  });
});

describe('WidgetApp <-> model protocol', () => {
  beforeEach(async () => {
    resetEngineBootstrapForTests();
    resetEngineMock();
    resetEditorMock();
    await seedSharedModule();
    rs.useFakeTimers();
  });
  afterEach(() => {
    rs.useRealTimers();
    resetEngineBootstrapForTests();
    document.body.innerHTML = '';
  });

  it('an Editor save sets project_json + pending_base, saves once, and chains the version', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    const { el } = await mount(model);
    fireEvent.click(screen.getByText('save'));
    await waitFor(() => expect(model.saveChangesCount).toBe(1));
    const keys = model.sets.map((s) => s.key);
    expect(keys).toEqual(['project_json', 'pending_base']);
    expect(model.lastSet('pending_base')).toBe(3);
    expect(JSON.parse(model.lastSet('project_json') as string)).toMatchObject({ edited: 1 });
    // The controller adopted base+1 as acknowledged, so the next save sends 4.
    fireEvent.click(screen.getByText('save'));
    await waitFor(() => expect(model.saveChangesCount).toBe(2));
    expect(model.lastSet('pending_base')).toBe(4);
    // Editor was never remounted by our own sets (they do not touch revision).
    expect(el.querySelectorAll('[data-testid="editor-mock"]')).toHaveLength(1);
    expect(mounts).toHaveLength(1);
  });

  it('the kernel echo of an own snapshot (revision+1, same JSON) keeps the Editor mounted', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('save'));
    await waitFor(() => expect(model.saveChangesCount).toBe(1));
    const sentJson = model.lastSet('project_json') as string;
    act(() => {
      model.kernelPush({ project_json: sentJson, revision: 4 });
    });
    expect(mounts).toHaveLength(1);
  });

  it('a kernel-originated snapshot remounts the Editor on the new JSON and revision', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    const { el } = await mount(model);
    act(() => {
      model.kernelPush({ project_json: '{"name":"from-python"}', revision: 4 });
    });
    expect(mounts).toHaveLength(2);
    const editor = el.querySelector('[data-testid="editor-mock"]');
    expect(editor?.getAttribute('data-initial-json')).toBe('{"name":"from-python"}');
    expect(editor?.getAttribute('data-initial-version')).toBe('4');
  });

  it('a stale-snapshot rejection (reseed at a new revision) remounts and forgets pending snapshots', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('save'));
    await waitFor(() => expect(model.saveChangesCount).toBe(1));
    act(() => {
      model.kernelPush({ project_json: '{"name":"kernel-wins"}', revision: 5, notice: 'conflict' });
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialProjectVersion).toBe(5);
    expect(screen.getByRole('status').textContent).toBe('conflict');
  });

  it('a notice shows, then auto-hides; clearing it kernel-side hides it immediately', async () => {
    const model = new FakeModel(defaultState());
    await mount(model);
    expect(screen.queryByRole('status')).toBeNull();
    act(() => {
      model.kernelPush({ notice: 'Updated on disk' });
    });
    expect(screen.getByRole('status').textContent).toBe('Updated on disk');
    act(() => {
      rs.advanceTimersByTime(NOTICE_TIMEOUT_MS);
    });
    expect(screen.queryByRole('status')).toBeNull();
    act(() => {
      model.kernelPush({ notice: 'again' });
    });
    expect(screen.getByRole('status').textContent).toBe('again');
    act(() => {
      model.kernelPush({ notice: '' });
    });
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('height, theme, and read_only changes re-render without remounting', async () => {
    const model = new FakeModel(defaultState({ height: 400, theme: 'light', read_only: false }));
    const { el } = await mount(model);
    act(() => {
      model.kernelPush({ height: 250, theme: 'dark', read_only: true });
    });
    const wrapper = el.querySelector('[data-lm-suppress-shortcuts]') as HTMLElement;
    expect(wrapper.style.height).toBe('250px');
    expect(wrapper.getAttribute('data-theme')).toBe('dark');
    expect(el.querySelector('[data-testid="editor-mock"]')?.getAttribute('data-read-only')).toBe('true');
    expect(mounts).toHaveLength(1);
  });

  it('theme auto follows the JupyterLab body attribute', async () => {
    document.body.dataset.jpThemeLight = 'false';
    const model = new FakeModel(defaultState({ theme: 'auto' }));
    const { el } = await mount(model);
    expect(el.querySelector('[data-lm-suppress-shortcuts]')?.getAttribute('data-theme')).toBe('dark');
    delete document.body.dataset.jpThemeLight;
  });

  it('selection changes are debounced into one selection trait sync', async () => {
    const model = new FakeModel(defaultState());
    await mount(model);
    fireEvent.click(screen.getByText('select'));
    fireEvent.click(screen.getByText('select'));
    expect(model.sets.filter((s) => s.key === 'selection')).toHaveLength(0);
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    expect(model.sets.filter((s) => s.key === 'selection')).toHaveLength(1);
    expect(model.lastSet('selection')).toEqual(['a', 'b']);
    expect(model.saveChangesCount).toBe(1);
  });
});
