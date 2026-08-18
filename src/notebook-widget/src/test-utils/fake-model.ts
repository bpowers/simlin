// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A transport-faithful stand-in for anywidget's AnyModel proxy over an
// ipywidgets DOMWidgetModel, plus the kernel-side half of the protocol.
//
// Frontend transport rules reproduced (jupyter-widgets/base widget.ts):
//   - `set()` fires Backbone `change:<key>` synchronously and buffers the key;
//   - `save_changes()` sends the buffered keys as ONE `patch` message -- but
//     only if no sync is in flight; while one is, further `save_changes()`
//     calls MERGE their keys into a single pending patch (assign semantics:
//     a later value for the same key overwrites the earlier one), which goes
//     out as one message when the in-flight one is acknowledged. This is the
//     collapse that forbids putting snapshots in traits;
//   - `send(content)` queues a custom message; custom messages are delivered
//     in order and never merged.
// Kernel side: `busyKernel` holds every inbound message (patches and customs)
// until `releaseKernel()`, which delivers them in order -- a long-running cell.
// The kernel handler applies the protocol: a `snapshot` whose base equals the
// current revision is accepted (traits pushed in one hold_sync, then a
// `saved` reply), otherwise `rejected`.

import type { AnyModel } from '../anywidget-model';

type Listener = (...args: unknown[]) => void;

export interface Delivered {
  kind: 'patch' | 'custom';
  content: Record<string, unknown>;
}

export class FakeModel implements AnyModel {
  state: Record<string, unknown>;
  /** Every set() the widget performed, in order (Backbone-side). */
  readonly sets: Array<{ key: string; value: unknown }> = [];
  /** Every custom message the widget sent, in order (before transport). */
  readonly sent: unknown[] = [];
  /** What actually reached the kernel handler, in order. */
  readonly delivered: Delivered[] = [];
  saveChangesCount = 0;
  /**
   * Kernel-side authoritative state. `applyFails` makes the fake kernel
   * reject a well-based snapshot WITHOUT advancing its revision -- the
   * kernel raised before applying anything (a parse failure, a handler
   * bug). A write failure AFTER applying is not a reject in pysimlin: the
   * revision has advanced and dirty is set, so the kernel replies `saved`
   * plus a warning notice.
   */
  kernel: { revision: number; projectJson: string; applyFails: boolean };
  /** While true, inbound messages queue instead of being handled. */
  busyKernel = false;
  private inbound: Delivered[] = [];
  private bufferedKeys = new Set<string>();
  private pendingPatch: Record<string, unknown> | null = null;
  private syncInFlight = false;
  private listeners = new Map<string, Set<Listener>>();

  constructor(initial: Record<string, unknown>) {
    this.state = { ...initial };
    this.kernel = {
      revision: typeof initial.revision === 'number' ? initial.revision : 0,
      projectJson: typeof initial.project_json === 'string' ? initial.project_json : '',
      applyFails: false,
    };
  }

  // ---- AnyModel surface ----

  get(key: string): unknown {
    return this.state[key];
  }

  set(key: string, value: unknown): void {
    const prev = this.state[key];
    this.state[key] = value;
    this.sets.push({ key, value });
    this.bufferedKeys.add(key);
    // Backbone gates `change:` events on deep equality (_.isEqual), so two
    // equal-by-value arrays would NOT fire there. Identity is used here
    // because every value the widget sets is a fresh array/string whose
    // identity change coincides with a value change in these tests; a test
    // that relied on equal-value suppression would need _.isEqual semantics.
    if (prev !== value) {
      this.trigger(`change:${key}`);
    }
  }

  save_changes(): void {
    this.saveChangesCount += 1;
    const patch: Record<string, unknown> = {};
    for (const key of this.bufferedKeys) {
      patch[key] = this.state[key];
    }
    this.bufferedKeys.clear();
    if (this.syncInFlight) {
      // Merge into the one pending patch (assign semantics).
      this.pendingPatch = { ...(this.pendingPatch ?? {}), ...patch };
      return;
    }
    this.syncInFlight = true;
    this.deliver({ kind: 'patch', content: patch });
  }

  send(content: unknown): void {
    this.sent.push(content);
    this.deliver({ kind: 'custom', content: content as Record<string, unknown> });
  }

  on(name: string, callback: Listener): void {
    let set = this.listeners.get(name);
    if (!set) {
      set = new Set();
      this.listeners.set(name, set);
    }
    set.add(callback);
  }

  off(name?: string | null, callback?: Listener | null): void {
    if (name === undefined || name === null) {
      this.listeners.clear();
      return;
    }
    const set = this.listeners.get(name);
    if (!set) {
      return;
    }
    if (callback) {
      set.delete(callback);
    } else {
      set.clear();
    }
  }

  // ---- test surface ----

  listenerCount(name: string): number {
    return this.listeners.get(name)?.size ?? 0;
  }

  trigger(name: string, ...args: unknown[]): void {
    const set = this.listeners.get(name);
    if (!set) {
      return;
    }
    for (const cb of [...set]) {
      cb(...args);
    }
  }

  /** Kernel -> frontend: one state message (all keys set, then one change event per changed key). */
  kernelPush(patch: Record<string, unknown>): void {
    const changed: string[] = [];
    for (const [key, value] of Object.entries(patch)) {
      if (this.state[key] !== value) {
        this.state[key] = value;
        changed.push(key);
      }
    }
    for (const key of changed) {
      this.trigger(`change:${key}`);
    }
  }

  /** Kernel -> frontend: a custom message with no buffers. */
  kernelSend(content: unknown): void {
    this.trigger('msg:custom', content, []);
  }

  /** The kernel edits its own state (Python edit() / disk reload). */
  kernelChange(projectJson: string, notice?: string): void {
    this.kernel.revision += 1;
    this.kernel.projectJson = projectJson;
    this.kernelPush({ project_json: projectJson, revision: this.kernel.revision });
    if (notice !== undefined) {
      this.kernelSend({ type: 'notice', text: notice });
    }
  }

  releaseKernel(): void {
    this.busyKernel = false;
    const queued = this.inbound;
    this.inbound = [];
    for (const msg of queued) {
      this.handle(msg);
    }
  }

  lastSnapshot(): { base: number; json: string } | undefined {
    for (let i = this.delivered.length - 1; i >= 0; i--) {
      const d = this.delivered[i];
      if (d.kind === 'custom' && d.content.type === 'snapshot') {
        return { base: d.content.base as number, json: d.content.json as string };
      }
    }
    return undefined;
  }

  snapshotsDelivered(): Array<{ base: number; json: string }> {
    return this.delivered
      .filter((d) => d.kind === 'custom' && d.content.type === 'snapshot')
      .map((d) => ({ base: d.content.base as number, json: d.content.json as string }));
  }

  lastSet(key: string): unknown {
    for (let i = this.sets.length - 1; i >= 0; i--) {
      if (this.sets[i].key === key) {
        return this.sets[i].value;
      }
    }
    return undefined;
  }

  // ---- transport + kernel ----

  private deliver(msg: Delivered): void {
    if (this.busyKernel) {
      this.inbound.push(msg);
      return;
    }
    this.handle(msg);
  }

  private handle(msg: Delivered): void {
    this.delivered.push(msg);
    if (msg.kind === 'patch') {
      // Kernel applied it; ack. The frontend then flushes its merged
      // pending patch (if any) as the next single in-flight sync.
      this.syncInFlight = false;
      if (this.pendingPatch !== null) {
        const next = this.pendingPatch;
        this.pendingPatch = null;
        this.syncInFlight = true;
        this.deliver({ kind: 'patch', content: next });
      }
      return;
    }
    if (msg.content.type === 'snapshot') {
      const base = msg.content.base as number;
      const json = msg.content.json as string;
      if (base !== this.kernel.revision || this.kernel.applyFails) {
        this.kernelSend({ type: 'rejected', revision: this.kernel.revision });
        return;
      }
      this.kernel.revision += 1;
      this.kernel.projectJson = json;
      this.kernelPush({ project_json: json, revision: this.kernel.revision });
      this.kernelSend({ type: 'saved', revision: this.kernel.revision });
    }
    // `wasm` and unknown types are left to the individual test.
  }
}

export function defaultState(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    project_json: '{"name":"p"}',
    revision: 3,
    selection: [],
    height: 400,
    theme: 'light',
    read_only: false,
    ...overrides,
  };
}
