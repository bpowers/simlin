// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// The unit-test twin of e2e/harness/fake-anywidget-model.js: anywidget's
// AnyModel proxy surface with Backbone-style change events, recording every
// set/send/save_changes, plus a kernelPush() that applies a state update the
// way ipywidgets does (all keys set, then one change event per key).

import type { AnyModel } from '../anywidget-model';

type Listener = (...args: unknown[]) => void;

export class FakeModel implements AnyModel {
  state: Record<string, unknown>;
  readonly sets: Array<{ key: string; value: unknown }> = [];
  readonly sent: unknown[] = [];
  saveChangesCount = 0;
  private listeners = new Map<string, Set<Listener>>();

  constructor(initial: Record<string, unknown>) {
    this.state = { ...initial };
  }

  get(key: string): unknown {
    return this.state[key];
  }

  set(key: string, value: unknown): void {
    const prev = this.state[key];
    this.state[key] = value;
    this.sets.push({ key, value });
    if (prev !== value) {
      this.trigger(`change:${key}`);
    }
  }

  save_changes(): void {
    this.saveChangesCount += 1;
  }

  send(content: unknown): void {
    this.sent.push(content);
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

  lastSet(key: string): unknown {
    for (let i = this.sets.length - 1; i >= 0; i--) {
      if (this.sets[i].key === key) {
        return this.sets[i].value;
      }
    }
    return undefined;
  }
}

export function defaultState(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    project_json: '{"name":"p"}',
    revision: 3,
    pending_base: 0,
    selection: [],
    height: 400,
    theme: 'light',
    notice: '',
    read_only: false,
    ...overrides,
  };
}
