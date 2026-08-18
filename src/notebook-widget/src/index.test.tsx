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
import { localJson, mounts, resetEditorMock } from './test-utils/editor-mock';
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
    expect(model.listenerCount('change:project_json')).toBe(0);
    expect(model.listenerCount('msg:custom')).toBe(0);
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

  const SEED = '{"name":"p"}';

  it('an edit sends ONE snapshot custom message with the acknowledged base and never sets project_json', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    const { el } = await mount(model);
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4]));
    expect(model.snapshotsDelivered()).toEqual([{ base: 3, json: localJson(SEED, 1) }]);
    expect(model.sets.filter((s) => s.key === 'project_json')).toEqual([]);
    expect(model.saveChangesCount).toBe(0);
    // The kernel accepted: its traits carry our bytes at revision 4 and the
    // Editor was NOT remounted (own ack).
    expect(model.get('project_json')).toBe(localJson(SEED, 1));
    expect(model.get('revision')).toBe(4);
    expect(el.querySelectorAll('[data-testid="editor-mock"]')).toHaveLength(1);
    expect(mounts).toHaveLength(1);
    expect(mounts[0].serverVersion).toBe(4);
    // A second edit chains from the acknowledged revision.
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4, 5]));
    expect(model.lastSnapshot()).toEqual({ base: 4, json: localJson(SEED, 2) });
  });

  it('a burst of 3 edits during a busy kernel yields exactly one snapshot of the latest state, accepted', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    fireEvent.click(screen.getByText('edit'));
    fireEvent.click(screen.getByText('edit'));
    // One snapshot is in flight (edit 1); the controller queued one flush.
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(1);
    await act(async () => {
      model.releaseKernel();
    });
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4, 5]));
    // The queued flush carried the LATEST state (3 edits) with the base the
    // kernel had just acknowledged, and was accepted -- nothing lost, nothing
    // rejected.
    expect(model.snapshotsDelivered()).toEqual([
      { base: 3, json: localJson(SEED, 1) },
      { base: 4, json: localJson(SEED, 3) },
    ]);
    expect(model.kernel.revision).toBe(5);
    expect(model.kernel.projectJson).toBe(localJson(SEED, 3));
    expect(mounts).toHaveLength(1);
  });

  it('a kernel change while a snapshot is in flight: rejected -> remount from authoritative -> next save carries the new base', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(1);
    // Disk change lands kernel-side (revision 4) before our snapshot (base 3)
    // is handled; the kernel pushes it and notices us.
    act(() => {
      model.kernelChange('{"name":"disk"}', 'Updated on disk');
    });
    // The push remounted the Editor on the disk state.
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialProjectJson).toBe('{"name":"disk"}');
    expect(mounts[1].props.initialProjectVersion).toBe(4);
    expect(screen.getByRole('status').textContent).toBe('Updated on disk');
    // Now the kernel gets to our stale snapshot: rejected. The first mount's
    // save resolves undefined; no second remount (idempotent on the pair).
    await act(async () => {
      model.releaseKernel();
    });
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined]));
    expect(mounts).toHaveLength(2);
    expect(model.kernel.projectJson).toBe('{"name":"disk"}');
    // The next edit (in the new mount) carries base 4 and is accepted.
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[1].saveResults).toEqual([5]));
    expect(model.lastSnapshot()).toEqual({ base: 4, json: localJson('{"name":"disk"}', 1) });
  });

  it('an equal-bytes reject (kernel write failed, revision unchanged) remounts once and does not loop', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.kernel.writeFails = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined]));
    // The kernel touched no trait; the seed pair is unchanged, so remountFrom
    // is a no-op -- the Editor keeps its local edit and version 3.
    expect(model.sets).toEqual([]);
    expect(mounts).toHaveLength(1);
    expect(mounts[0].serverVersion).toBe(3);
    // Nothing else was sent: no retry storm.
    expect(model.snapshotsDelivered()).toHaveLength(1);
    // Once the kernel can write again, the next edit goes out against the
    // same base and is accepted.
    model.kernel.writeFails = false;
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined, 4]));
    expect(model.lastSnapshot()).toEqual({ base: 3, json: localJson(SEED, 2) });
  });

  it('a kernel-originated snapshot remounts the Editor on the new JSON and revision, exactly once', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    const { el } = await mount(model);
    act(() => {
      model.kernelChange('{"name":"from-python"}');
    });
    expect(mounts).toHaveLength(2);
    const editor = el.querySelector('[data-testid="editor-mock"]');
    expect(editor?.getAttribute('data-initial-json')).toBe('{"name":"from-python"}');
    expect(editor?.getAttribute('data-initial-version')).toBe('4');
    // The same pair pushed again (idempotent re-push) does nothing.
    act(() => {
      model.kernelPush({ project_json: '{"name":"from-python"}', revision: 4 });
    });
    expect(mounts).toHaveLength(2);
    // Two change events, either key order, remount once.
    act(() => {
      model.kernelPush({ revision: 5, project_json: '{"name":"a"}' });
    });
    expect(mounts).toHaveLength(3);
    act(() => {
      model.kernelPush({ project_json: '{"name":"b"}', revision: 6 });
    });
    expect(mounts).toHaveLength(4);
    expect(mounts[3].props.initialProjectVersion).toBe(6);
  });

  it('a revision that goes backwards with the same bytes still remounts (generation bump)', async () => {
    const model = new FakeModel(defaultState({ revision: 3, project_json: 'X' }));
    await mount(model);
    act(() => {
      model.kernelPush({ revision: 0 });
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialProjectVersion).toBe(0);
    expect(mounts[1].props.initialProjectJson).toBe('X');
  });

  it('a save in flight at unmount resolves undefined and a late reply is ignored', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    const { cleanup } = await mount(model);
    fireEvent.click(screen.getByText('edit'));
    expect(mounts[0].saveResults).toEqual([]);
    cleanup();
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined]));
    // The kernel answers later: no listener, no throw.
    model.releaseKernel();
    expect(model.kernel.revision).toBe(4);
  });

  it('a second onSave while one is in flight is refused (defense: the controller never does this)', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    const first = mounts[0].props.onSave({ format: 'json', data: 'A' }, 3);
    const second = await mounts[0].props.onSave({ format: 'json', data: 'B' }, 3);
    expect(second).toBeUndefined();
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(1);
    await act(async () => {
      model.releaseKernel();
    });
    await expect(first).resolves.toBe(4);
  });

  it('a notice custom message shows, restarts its timer on a repeat, then auto-hides', async () => {
    const model = new FakeModel(defaultState());
    await mount(model);
    expect(screen.queryByRole('status')).toBeNull();
    act(() => {
      model.trigger('msg:custom', { type: 'notice', text: 'Updated on disk' }, []);
    });
    expect(screen.getByRole('status').textContent).toBe('Updated on disk');
    act(() => {
      rs.advanceTimersByTime(NOTICE_TIMEOUT_MS - 1);
    });
    // The same text again (a second disk reload) is a new event: it shows and
    // restarts the timer -- a trait could not have expressed this.
    act(() => {
      model.trigger('msg:custom', { type: 'notice', text: 'Updated on disk' }, []);
    });
    act(() => {
      rs.advanceTimersByTime(NOTICE_TIMEOUT_MS - 1);
    });
    expect(screen.getByRole('status').textContent).toBe('Updated on disk');
    act(() => {
      rs.advanceTimersByTime(1);
    });
    expect(screen.queryByRole('status')).toBeNull();
    // Non-notice custom messages are ignored.
    act(() => {
      model.trigger('msg:custom', { type: 'wasm' }, []);
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

  it('theme auto follows the JupyterLab body attribute, live, and disconnects on unmount', async () => {
    // Stub MutationObserver so the test can assert what is observed and that
    // the observer is disconnected on cleanup (jsdom's real one is opaque).
    const observers: Array<{
      target: Node | null;
      options: MutationObserverInit | null;
      disconnected: boolean;
      cb: MutationCallback;
    }> = [];
    class FakeMutationObserver {
      private rec: (typeof observers)[number];
      constructor(cb: MutationCallback) {
        this.rec = { target: null, options: null, disconnected: false, cb };
        observers.push(this.rec);
      }
      observe(target: Node, options?: MutationObserverInit): void {
        this.rec.target = target;
        this.rec.options = options ?? null;
      }
      disconnect(): void {
        this.rec.disconnected = true;
      }
      takeRecords(): MutationRecord[] {
        return [];
      }
    }
    rs.stubGlobal('MutationObserver', FakeMutationObserver);
    document.body.dataset.jpThemeLight = 'false';
    const model = new FakeModel(defaultState({ theme: 'auto' }));
    const { el, cleanup } = await mount(model);
    const wrapper = el.querySelector('[data-lm-suppress-shortcuts]')!;
    expect(wrapper.getAttribute('data-theme')).toBe('dark');
    expect(observers).toHaveLength(1);
    expect(observers[0].target).toBe(document.body);
    expect(observers[0].options).toEqual({ attributes: true, attributeFilter: ['data-jp-theme-light'] });
    // JupyterLab switches theme after mount.
    act(() => {
      document.body.dataset.jpThemeLight = 'true';
      observers[0].cb([], observers[0] as unknown as MutationObserver);
    });
    expect(wrapper.getAttribute('data-theme')).toBe('light');
    expect(observers[0].disconnected).toBe(false);
    cleanup();
    expect(observers[0].disconnected).toBe(true);
    delete document.body.dataset.jpThemeLight;
    rs.unstubAllGlobals();
  });

  it('theme auto follows the OS color scheme when JupyterLab gives no signal', async () => {
    delete document.body.dataset.jpThemeLight;
    let matches = false;
    const listeners = new Set<() => void>();
    const mql = {
      get matches() {
        return matches;
      },
      addEventListener: (_: string, cb: () => void) => listeners.add(cb),
      removeEventListener: (_: string, cb: () => void) => listeners.delete(cb),
    };
    rs.stubGlobal(
      'matchMedia',
      rs.fn(() => mql),
    );
    const model = new FakeModel(defaultState({ theme: 'auto' }));
    const { el, cleanup } = await mount(model);
    const wrapper = el.querySelector('[data-lm-suppress-shortcuts]')!;
    expect(wrapper.getAttribute('data-theme')).toBe('light');
    expect(listeners.size).toBe(1);
    act(() => {
      matches = true;
      for (const cb of listeners) {
        cb();
      }
    });
    expect(wrapper.getAttribute('data-theme')).toBe('dark');
    cleanup();
    expect(listeners.size).toBe(0);
    rs.unstubAllGlobals();
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
