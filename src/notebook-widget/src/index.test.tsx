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
import { formatSize, withEditorView } from './widget-core';
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
    // The wrapper paints the themed page background itself, so a forced theme
    // never leaves the transparent canvas on the notebook cell's colour.
    expect(wrapper.style.background).toBe('var(--color-background)');
    expect(wrapper.getAttribute('data-theme')).toBe('light');
    // The first mount opens at the project's stored viewport (nothing to carry).
    expect(mounts[0].props.initialViewport).toBeUndefined();
    expect(mounts).toHaveLength(1);
    expect(mounts[0].props.name).toBe('model');
    expect(mounts[0].props.inputFormat).toBe('json');
    // The Editor's overlay surfaces (drawer, dialogs, menus, listbox) render
    // inside the wrapper -- the element carrying the scoped tokens, data-theme
    // and data-lm-suppress-shortcuts -- not on document.body; and the drawer
    // shows no Exit link, since a notebook page has no "/" to go to.
    expect(mounts[0].props.portalContainer).toBe(wrapper);
    expect(mounts[0].props.showHomeLink).toBe(false);

    // Every model listener the view added is registered...
    for (const name of [
      'change:revision',
      'change:project_json',
      'change:height',
      'change:theme',
      'change:read_only',
      'msg:custom',
    ]) {
      expect(model.listenerCount(name)).toBe(1);
    }
    cleanup();
    expect(el.childElementCount).toBe(0);
    // ...and every one of them is gone after cleanup.
    for (const name of [
      'change:revision',
      'change:project_json',
      'change:height',
      'change:theme',
      'change:read_only',
      'msg:custom',
    ]) {
      expect(model.listenerCount(name)).toBe(0);
    }
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

  it("a remount while a snapshot is in flight frees the slot: the new Editor's first edit is SENT with the new base and accepted; the stale reply is consumed and ignored", async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(1);
    // Disk change lands kernel-side (revision 4) while our snapshot (base 3)
    // waits at the busy kernel; the push remounts the Editor.
    act(() => {
      model.kernelChange('{"name":"disk"}');
    });
    expect(mounts).toHaveLength(2);
    // The old Editor is gone, so its save resolves undefined NOW (nothing may
    // wait on a reply for a mount that no longer exists) and the in-flight
    // slot is free for the new Editor.
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined]));
    // The new Editor's FIRST edit goes out immediately with the new base --
    // it is not refused as "one already in flight" (that lost the edit until
    // a second edit came along).
    fireEvent.click(screen.getByText('edit'));
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(2);
    expect(model.sent[model.sent.length - 1]).toEqual({
      type: 'snapshot',
      id: model.lastSentSnapshotId(),
      base: 4,
      json: localJson('{"name":"disk"}', 1),
    });
    // Each request carries its own id.
    const ids = model.sent
      .filter((m) => (m as { type: string }).type === 'snapshot')
      .map((m) => (m as { id: string }).id);
    expect(new Set(ids).size).toBe(2);
    // The kernel now handles both in order: the stale snapshot (base 3) is
    // rejected -- that reply carries the freed snapshot's id, matches nothing
    // in flight and is ignored: no second remount, the seed stays at the
    // kernel's state -- and the new one (base 4) is accepted.
    await act(async () => {
      model.releaseKernel();
    });
    await waitFor(() => expect(mounts[1].saveResults).toEqual([5]));
    expect(mounts).toHaveLength(2);
    expect(mounts[0].saveResults).toEqual([undefined]);
    expect(model.kernel.revision).toBe(5);
    expect(model.kernel.projectJson).toBe(localJson('{"name":"disk"}', 1));
    expect(mounts[1].serverVersion).toBe(5);
    // And the accepted state is the seed: its own push is `none` (no
    // remount), a further edit chains from 5.
    act(() => {
      model.kernelPush({ project_json: localJson('{"name":"disk"}', 1), revision: 5 });
    });
    expect(mounts).toHaveLength(2);
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[1].saveResults).toEqual([5, 6]));
    expect(model.lastSnapshot()).toEqual({ base: 5, json: localJson('{"name":"disk"}', 2) });
  });

  it('a stale reply that is a `saved` (the kernel accepted the old snapshot, then moved on) does not regress the seed', async () => {
    // Order on the wire: the accept's state push, then a further kernel
    // change, then the `saved` custom message dispatching late (a host that
    // dispatches customs after states, or a slow reply). By the time `saved`
    // arrives the Editor has been remounted on the later state; adopting the
    // reply's (4, json1) as the seed would step the seed backwards and the
    // next kernel push of revision 5 would remount again for nothing.
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    const oldId = model.lastSentSnapshotId();
    const json1 = localJson(SEED, 1);
    act(() => {
      model.kernelPush({ project_json: json1, revision: 4 });
    });
    // own-ack: no remount, save still in flight (no reply yet).
    expect(mounts).toHaveLength(1);
    expect(mounts[0].saveResults).toEqual([]);
    // Kernel-side that accept happened (revision 4, our bytes); now it moves on.
    model.kernel.revision = 4;
    model.kernel.projectJson = json1;
    act(() => {
      model.kernelChange('{"name":"later"}');
    });
    // Remount on the later state; the old save resolves undefined.
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialProjectVersion).toBe(5);
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined]));
    // The late `saved` for the old snapshot (its id echoed): ignored.
    act(() => {
      model.kernelSend({ type: 'saved', revision: 4, id: oldId });
    });
    expect(mounts).toHaveLength(2);
    // The seed is still (5, later): its own re-push is `none`...
    act(() => {
      model.kernelPush({ project_json: '{"name":"later"}', revision: 5 });
    });
    expect(mounts).toHaveLength(2);
    // ...and the next edit carries base 5.
    model.busyKernel = false;
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[1].saveResults).toEqual([6]));
    expect(model.lastSnapshot()).toEqual({ base: 5, json: localJson('{"name":"later"}', 1) });
  });

  it('a reject at an unchanged revision (kernel failed before applying) does not remount and does not loop', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.kernel.applyFails = true;
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
    // Once the kernel can apply again, the next edit goes out against the
    // same base and is accepted.
    model.kernel.applyFails = false;
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined, 4]));
    expect(model.lastSnapshot()).toEqual({ base: 3, json: localJson(SEED, 2) });
    // The accept adopted (4, edit2) as the seed: an idempotent re-push of that
    // pair, and an unchanged-revision reject right after it, both leave the Editor
    // mounted (no remount to stale content, no lost edit).
    act(() => {
      model.kernelPush({ project_json: localJson(SEED, 2), revision: 4 });
    });
    expect(mounts).toHaveLength(1);
    model.kernel.applyFails = true;
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined, 4, undefined]));
    expect(mounts).toHaveLength(1);
    expect(mounts[0].serverVersion).toBe(4);
  });

  it("a `saved` dispatched BEFORE the accept's state push keeps the Editor (order-independent)", async () => {
    // Some hosts may dispatch the custom message before applying the state
    // update, or a kernel may send `saved` early. Model that by holding the
    // kernel busy, then delivering saved first and the push second.
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    const json = localJson(SEED, 1);
    act(() => {
      model.kernelSend({ type: 'saved', revision: 4, id: model.lastSentSnapshotId() });
    });
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4]));
    expect(mounts).toHaveLength(1);
    act(() => {
      model.kernelPush({ project_json: json, revision: 4 });
    });
    // The push is the state we already adopted from `saved`: none, no remount.
    expect(mounts).toHaveLength(1);
    expect(mounts[0].serverVersion).toBe(4);
    // And the reverse order (push first, then saved) is the normal path,
    // covered by the first test; both leave exactly one mount.
  });

  it('a reply that does not name the in-flight request is ignored: another id, or no id at all', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    const id = model.lastSentSnapshotId();
    expect(id).toBeDefined();
    act(() => {
      model.kernelSend({ type: 'saved', revision: 4, id: `${id}-not-ours` });
      model.kernelSend({ type: 'rejected', revision: 3, id: 'someone-else:1' });
      model.kernelSend({ type: 'saved', revision: 4 });
      model.kernelSend({ type: 'rejected', revision: 3 });
    });
    // Still in flight: none of those was the answer.
    expect(mounts[0].saveResults).toEqual([]);
    expect(mounts).toHaveLength(1);
    // The real answer (the kernel echoing our id) resolves it.
    await act(async () => {
      model.releaseKernel();
    });
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4]));
  });

  it('two views of one model (the same widget displayed twice) each resolve only their own replies', async () => {
    // Every custom message reaches every view. View A and view B both edit
    // against base 3 while the kernel is busy; the kernel accepts A's and
    // rejects B's (stale). B's Editor is remounted by A's accept push (its
    // in-flight save resolves undefined, slot freed) and B edits again with
    // the new base BEFORE its own rejection arrives. Without request ids B
    // would take A's `saved` as the reply owed to its freed snapshot and then
    // its own late `rejected` as the answer to the NEW edit, resolving that
    // save unsaved; with ids, each reply answers exactly the request it names.
    // View B's snapshots are driven through onSave directly so its bytes
    // differ from A's (the mock Editor would produce identical ones).
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    const a = await mount(model);
    const b = await mount(model);
    expect(mounts).toHaveLength(2);
    fireEvent.click(screen.getAllByText('edit')[0]); // view A: snapshot A1 (base 3)
    const idA1 = model.lastSentSnapshotId();
    const saveB1 = mounts[1].props.onSave({ format: 'json', data: '{"name":"B1"}' }, 3);
    const idB1 = model.lastSentSnapshotId();
    expect(idA1).not.toBe(idB1);
    // The kernel handles A1 only: accept -> push (4, jsonA) + saved{A1}.
    await act(async () => {
      model.releaseOne();
    });
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4]));
    // A: own-ack, live Editor kept. B: the push is a foreign change -> remount;
    // B1's save resolved undefined, its slot freed.
    await expect(saveB1).resolves.toBeUndefined();
    expect(mounts).toHaveLength(3);
    expect(b.el.querySelector('[data-initial-version="4"]')).not.toBeNull();
    expect(a.el.querySelectorAll('[data-testid="editor-mock"]')).toHaveLength(1);
    // B's new Editor edits against base 4 before B1's rejection arrives.
    const saveB2 = mounts[2].props.onSave({ format: 'json', data: '{"name":"B2"}' }, 4);
    const idB2 = model.lastSentSnapshotId();
    expect(idB2).not.toBe(idB1);
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(3);
    // Now B1: rejected{B1} (stale) -- it names no in-flight request (B has B2
    // in flight, A nothing): ignored by both, no remount, B2 still waiting.
    let b2Settled = false;
    void saveB2.then(() => {
      b2Settled = true;
    });
    await act(async () => {
      model.releaseOne();
    });
    expect(mounts).toHaveLength(3);
    expect(b2Settled).toBe(false);
    // Then B2: accepted -> push (5, jsonB2) + saved{B2}. B: own-ack, resolves
    // 5; A: foreign change -> remount (A had nothing in flight).
    await act(async () => {
      model.releaseOne();
    });
    await expect(saveB2).resolves.toBe(5);
    expect(mounts).toHaveLength(4);
    expect(mounts[0].saveResults).toEqual([4]);
    expect(mounts[3].props.initialProjectVersion).toBe(5);
    expect(mounts[3].props.initialProjectJson).toBe('{"name":"B2"}');
    expect(model.kernel.revision).toBe(5);
    a.cleanup();
    b.cleanup();
  });

  it('two views sending byte-identical snapshots: the loser is remounted by its reject, not wedged by a false own-ack', async () => {
    // A's accept pushes (4, bytes) that equal what B has in flight at base+1,
    // so B classifies the push `own-ack` -- but the kernel then rejects B's
    // (stale). Had B adopted the pushed pair as its seed on that own-ack, the
    // reject would find the pair already seeded and remount nothing, leaving
    // B's Editor acknowledged at 3 with every later save stale. The seed is
    // adopted only on `saved`, so the reject remounts B on (4, bytes) and its
    // next edit goes out with base 4.
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    await mount(model);
    const editButtons = screen.getAllByText('edit');
    fireEvent.click(editButtons[0]); // A1: localJson(SEED, 1) at base 3
    fireEvent.click(editButtons[1]); // B1: the SAME bytes at base 3
    await act(async () => {
      model.releaseOne(); // A1 accepted: push (4, bytes) + saved{A1}
    });
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4]));
    // B: own-ack (no remount), its save still in flight.
    expect(mounts).toHaveLength(2);
    expect(mounts[1].saveResults).toEqual([]);
    await act(async () => {
      model.releaseOne(); // B1 rejected (stale), id B1
    });
    await waitFor(() => expect(mounts[1].saveResults).toEqual([undefined]));
    // The reject remounted B on the kernel's pair.
    expect(mounts).toHaveLength(3);
    expect(mounts[2].props.initialProjectVersion).toBe(4);
    // B's next edit chains from 4 and is accepted.
    model.busyKernel = false;
    fireEvent.click(screen.getAllByText('edit')[1]);
    await waitFor(() => expect(mounts[2].saveResults).toEqual([5]));
    expect(model.lastSnapshot()?.base).toBe(4);
  });

  it('a malformed reply while a snapshot is in flight is treated as a reject, not ignored', async () => {
    // A kernel whose reply lost its revision (say a serialization bug) but
    // still names the request: the reply consumes the one answer the snapshot
    // is owed.
    const model = new FakeModel(defaultState({ revision: 3 }));
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    act(() => {
      model.kernelSend({ type: 'saved', id: model.lastSentSnapshotId() });
    });
    // Resolved undefined (the controller's save queue is not stuck), no
    // trait moved so no remount, and the Editor keeps its local edit.
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined]));
    expect(mounts).toHaveLength(1);
    expect(mounts[0].serverVersion).toBe(3);
    // The next edit sends a fresh snapshot (base still 3) -- the widget is
    // not wedged waiting on the broken reply.
    fireEvent.click(screen.getByText('edit'));
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(2);
    expect(model.sent[model.sent.length - 1]).toEqual({
      type: 'snapshot',
      id: model.lastSentSnapshotId(),
      base: 3,
      json: localJson(SEED, 2),
    });
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

  it('a project whose root model has no view is seeded with an empty view, and its first save carries it', async () => {
    // Defence in depth: the kernel lays a viewless model out before seeding
    // (pysimlin `_ensure_view`), but if that failed the Editor must not mount
    // dead on views[0] === undefined.
    const viewless = '{"name":"p","models":[{"name":"main","auxiliaries":[{"name":"a","equation":"1"}]}]}';
    const model = new FakeModel(defaultState({ project_json: viewless, revision: 2 }));
    const { el } = await mount(model);
    const editor = el.querySelector('[data-testid="editor-mock"]');
    const seeded = JSON.parse(editor?.getAttribute('data-initial-json') ?? '') as {
      models: Array<{ name: string; views?: Array<{ kind: string; elements: unknown[] }> }>;
    };
    expect(seeded.models[0].views).toHaveLength(1);
    expect(seeded.models[0].views?.[0]).toMatchObject({ kind: 'stock_flow', elements: [] });
    expect(seeded.models[0].name).toBe('main');
    // The kernel's own trait is untouched (only the kernel writes it) ...
    expect(model.get('project_json')).toBe(viewless);
    // ... and an edit saves the repaired project against the seeded base as
    // an own-ack (no remount): the kernel echoes our bytes, which have the view.
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([3]));
    expect(mounts).toHaveLength(1);
    expect(model.lastSnapshot()?.base).toBe(2);
    expect(model.lastSnapshot()?.json).toBe(localJson(withEditorView(viewless), 1));
    // A later viewless push from the kernel remounts on the repaired text once.
    act(() => {
      model.kernelChange('{"name":"p","models":[{"name":"main"}]}');
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialProjectJson).toBe(withEditorView('{"name":"p","models":[{"name":"main"}]}'));
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

  it('a save in flight at unmount resolves undefined, a late reply is ignored, a late onSave is refused', async () => {
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
    // A queued controller flush racing the dispose: refused, nothing sent.
    const late = await mounts[0].props.onSave({ format: 'json', data: 'LATE' }, 4);
    expect(late).toBeUndefined();
    expect(model.sent.filter((m) => (m as { type: string }).type === 'snapshot')).toHaveLength(1);
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

  it('an edit whose snapshot exceeds max_snapshot_bytes is refused up front: oversize report, toast, unsaved, no hang', async () => {
    // A tiny cap stands in for a huge model. The kernel would never see a
    // snapshot above tornado's limit (the server closes the socket), so the
    // widget must not send one: it reports `oversize`, resolves the save
    // unsaved, and tells the user -- instead of waiting forever.
    const model = new FakeModel(defaultState({ revision: 3, max_snapshot_bytes: 16 }));
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined]));
    // The reported size is the WIRE size: the snapshot JSON-escaped, which
    // is what the frame carries (every quote in it costs an extra byte).
    const bytes = new TextEncoder().encode(JSON.stringify(localJson(SEED, 1))).byteLength;
    expect(bytes).toBeGreaterThan(new TextEncoder().encode(localJson(SEED, 1)).byteLength);
    expect(bytes).toBeGreaterThan(16);
    expect(model.snapshotsDelivered()).toEqual([]);
    expect(model.sent).toEqual([{ type: 'oversize', bytes }]);
    // The toast is the widget's own; the kernel's echoed notice has the same
    // text, so the two collapse into one visible message.
    const text = `Edit not saved: the model is too large for the notebook connection (${formatSize(bytes)} > 0 KiB limit); edit it from Python instead.`;
    expect(screen.getByRole('status').textContent).toBe(text);
    expect(screen.getAllByRole('status')).toHaveLength(1);
    // Nothing moved kernel-side, no remount, the Editor keeps its local
    // edit and its acknowledged version.
    expect(model.kernel.revision).toBe(3);
    expect(model.sets).toEqual([]);
    expect(mounts).toHaveLength(1);
    expect(mounts[0].serverVersion).toBe(3);
    // A later edit is refused the same way (no in-flight slot was left behind).
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined, undefined]));
    expect(model.sent.filter((m) => (m as { type: string }).type === 'oversize')).toHaveLength(2);
    // The kernel raises the cap (a redisplay with max_snapshot_bytes=...):
    // the next edit goes out as a normal snapshot against the same base and
    // is accepted -- the cap is read live, not captured at mount.
    act(() => {
      model.kernelPush({ max_snapshot_bytes: 1024 * 1024 });
    });
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([undefined, undefined, 4]));
    expect(model.snapshotsDelivered()).toEqual([{ base: 3, json: localJson(SEED, 3) }]);
    expect(mounts).toHaveLength(1);
  });

  it('a snapshot exactly at max_snapshot_bytes (wire size) is sent', async () => {
    const json = localJson(SEED, 1);
    const bytes = new TextEncoder().encode(JSON.stringify(json)).byteLength;
    const model = new FakeModel(defaultState({ revision: 3, max_snapshot_bytes: bytes }));
    await mount(model);
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4]));
    expect(model.snapshotsDelivered()).toEqual([{ base: 3, json }]);
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('every element that takes focus inside the widget carries data-lm-suppress-shortcuts (Lumino matches the focused target before walking up)', async () => {
    const model = new FakeModel(defaultState());
    const { el } = await mount(model);
    const wrapper = el.querySelector('[data-lm-suppress-shortcuts]') as HTMLElement;
    expect(wrapper.classList.contains('simlin-notebook-widget')).toBe(true);
    // A control inside the Editor tree that did not carry the attribute
    // itself gets it the moment it is focused -- before any keydown can be
    // dispatched at it.
    const edit = screen.getByText('edit');
    expect(edit.hasAttribute('data-lm-suppress-shortcuts')).toBe(false);
    act(() => {
      edit.focus();
    });
    expect(document.activeElement).toBe(edit);
    expect(edit.hasAttribute('data-lm-suppress-shortcuts')).toBe(true);
    // Elements outside the widget are untouched.
    const outside = document.createElement('button');
    document.body.appendChild(outside);
    act(() => {
      outside.focus();
    });
    expect(outside.hasAttribute('data-lm-suppress-shortcuts')).toBe(false);
    outside.remove();
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

  it('two selection syncs while a sync is in flight (busy kernel) collapse into one merged patch on release', async () => {
    // Pins the transport rule the design leans on for the trait-vs-custom
    // decision (see FakeModel): ipywidgets allows one in-flight sync per
    // model and assign-merges the patches buffered behind it. `selection` is
    // the widget's only trait write, so it is where the collapse is observed:
    // two debounced selection syncs during a busy kernel reach it as ONE patch
    // carrying the LAST value. Harmless for selection (latest wins is the
    // right answer); fatal for snapshots -- which is why they are messages.
    const model = new FakeModel(defaultState());
    model.busyKernel = true;
    await mount(model);
    fireEvent.click(screen.getByText('select'));
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    // The first sync went out (queued at the busy kernel) and is in flight.
    expect(model.saveChangesCount).toBe(1);
    // A second selection later: its patch merges behind the in-flight one.
    mounts[0].props.onSelectionChanged?.(['c']);
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    expect(model.saveChangesCount).toBe(2);
    model.releaseKernel();
    const patches = model.delivered.filter((d) => d.kind === 'patch');
    // First in-flight patch, then exactly one merged patch -- not two.
    expect(patches).toHaveLength(2);
    expect(patches[0].content).toEqual({ selection: ['a', 'b'] });
    expect(patches[1].content).toEqual({ selection: ['c'] });
    // A third sync after both were acknowledged goes out on its own.
    mounts[0].props.onSelectionChanged?.(['d']);
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    expect(model.delivered.filter((d) => d.kind === 'patch')).toHaveLength(3);
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

  it('a kernel push that remounts the Editor publishes an empty selection (the new Editor starts with none)', async () => {
    // The Editor suppresses onSelectionChanged on its initial mount, so a
    // remount on a kernel push would leave the trait -- and Model.selection
    // -- naming whatever the OLD Editor had selected, possibly variables the
    // push removed, while the UI shows nothing selected.
    const model = new FakeModel(defaultState({ revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('select'));
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    expect(model.lastSet('selection')).toEqual(['a', 'b']);
    act(() => {
      model.kernelChange('{"name":"from-python"}');
    });
    expect(mounts).toHaveLength(2);
    // Debounced like any selection change, so a burst of pushes is one sync.
    expect(model.sets.filter((s) => s.key === 'selection')).toHaveLength(1);
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    expect(model.sets.filter((s) => s.key === 'selection')).toHaveLength(2);
    expect(model.lastSet('selection')).toEqual([]);
    expect(model.saveChangesCount).toBe(2);
  });

  it('a selection still pending when a remount lands is superseded by the empty one (never published stale)', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('select'));
    // Debounce still running when the kernel pushes.
    act(() => {
      model.kernelChange('{"name":"from-python"}');
    });
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    // Exactly one sync, and it carries the new Editor's (empty) selection,
    // not the old Editor's pending one.
    expect(model.sets.filter((s) => s.key === 'selection')).toEqual([{ key: 'selection', value: [] }]);
  });

  it('an own-ack push and an idempotent re-push do not touch the selection (no remount happened)', async () => {
    const model = new FakeModel(defaultState({ revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('select'));
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    fireEvent.click(screen.getByText('edit'));
    await waitFor(() => expect(mounts[0].saveResults).toEqual([4]));
    act(() => {
      model.kernelPush({ project_json: localJson(SEED, 1), revision: 4 });
    });
    act(() => {
      rs.advanceTimersByTime(SELECTION_DEBOUNCE_MS);
    });
    expect(mounts).toHaveLength(1);
    expect(model.sets.filter((s) => s.key === 'selection')).toHaveLength(1);
    expect(model.lastSet('selection')).toEqual(['a', 'b']);
  });
});

describe('viewport carried across a kernel-originated remount', () => {
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

  const projectWith = (viewBox: Record<string, number> | undefined, extra: Record<string, unknown> = {}): string =>
    JSON.stringify({
      name: 'p',
      models: [
        { name: 'main', views: [{ elements: [], ...(viewBox === undefined ? {} : { viewBox, zoom: 1 }) }], ...extra },
      ],
    });
  const box = { x: 10, y: 20, width: 800, height: 400 };
  // What the mock's first "pan" reports (editor-mock.tsx).
  const firstPan = { viewBox: { x: -10, y: 5, width: 800, height: 400 }, zoom: 1.25 };
  const secondPan = { viewBox: { x: -20, y: 10, width: 800, height: 400 }, zoom: 1.5 };

  it('a kernel push that left the stored viewport unchanged remounts on the LIVE viewport (the last committed pan)', async () => {
    const model = new FakeModel(defaultState({ project_json: projectWith(box), revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('pan'));
    fireEvent.click(screen.getByText('pan'));
    // A Python edit adds a variable; the stored viewBox/zoom are the same.
    act(() => {
      model.kernelChange(
        projectWith(box, { auxiliaries: [{ name: 'from_python', equation: '1' }] }),
        'Updated from Python',
      );
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialViewport).toEqual(secondPan);
    // The new Editor is seeded from the kernel's bytes; only the viewport rides along.
    expect(mounts[1].props.initialProjectJson).toBe(
      projectWith(box, { auxiliaries: [{ name: 'from_python', equation: '1' }] }),
    );
  });

  it("a kernel push that moved the stored viewport remounts on the kernel's (nothing carried)", async () => {
    const model = new FakeModel(defaultState({ project_json: projectWith(box), revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('pan'));
    act(() => {
      model.kernelChange(projectWith({ ...box, x: 300 }));
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialViewport).toBeUndefined();
  });

  it('a stored viewport that is still unset (a converted model) keeps the live framing across kernel pushes', async () => {
    const model = new FakeModel(defaultState({ project_json: projectWith(undefined), revision: 1 }));
    await mount(model);
    // The real Editor reports the mount-time fit as its first committed viewport.
    fireEvent.click(screen.getByText('pan'));
    act(() => {
      model.kernelChange(projectWith(undefined, { auxiliaries: [{ name: 'a', equation: '1' }] }));
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialViewport).toEqual(firstPan);
    // The carried viewport is compared against the OUTGOING seed on the next
    // push too: with the second Editor's own pan reported, a further push
    // carries that one.
    fireEvent.click(screen.getAllByText('pan')[0]);
    act(() => {
      model.kernelChange(
        projectWith(undefined, {
          auxiliaries: [
            { name: 'a', equation: '1' },
            { name: 'b', equation: '2' },
          ],
        }),
      );
    });
    expect(mounts).toHaveLength(3);
    // The second mount's mock counts its own pans from 1.
    expect(mounts[2].props.initialViewport).toEqual(firstPan);
  });

  it('a live viewport of a drilled-into module is not carried onto the root the remount opens', async () => {
    const model = new FakeModel(defaultState({ project_json: projectWith(box), revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('pan'));
    fireEvent.click(screen.getByText('pan-child'));
    act(() => {
      model.kernelChange(projectWith(box, { auxiliaries: [] }));
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialViewport).toBeUndefined();
  });

  it('a reject that re-seeds onto moved kernel state also carries the live viewport (same remount path)', async () => {
    const model = new FakeModel(defaultState({ project_json: projectWith(box), revision: 3 }));
    await mount(model);
    fireEvent.click(screen.getByText('pan'));
    model.busyKernel = true;
    fireEvent.click(screen.getByText('edit'));
    // A foreign change lands while the snapshot waits: its push remounts (with
    // the pan carried); the following rejected is a no-op on the pair.
    act(() => {
      model.kernelChange(projectWith(box, { auxiliaries: [{ name: 'x', equation: '1' }] }));
    });
    expect(mounts).toHaveLength(2);
    expect(mounts[1].props.initialViewport).toEqual(firstPan);
    await act(async () => {
      model.releaseKernel();
    });
    expect(mounts).toHaveLength(2);
  });
});
