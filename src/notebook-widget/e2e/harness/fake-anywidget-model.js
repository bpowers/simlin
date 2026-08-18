// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// A stand-in for anywidget's `AnyModel` proxy (packages/anywidget/src/
// model-proxy.ts over a Backbone DOMWidgetModel) with the pieces this widget
// uses: get/set/save_changes, Backbone-style `change:<key>` events, and the
// custom-message channel (`send` out, `msg:custom` in). It also plays the
// kernel side of the protocol: it answers `{type:'wasm'}` with the artifact
// as a DataView buffer (exactly how ipywidgets hands binary buffers to
// `msg:custom` listeners), and it handles `{type:'snapshot', base, json}` the
// way pysimlin's ModelWidget must -- accept when base equals its revision
// (push project_json + revision, then reply `saved`), else reply `rejected`
// -- and records everything so the Playwright spec can assert on it.
export class FakeAnyModel {
  constructor(initialState, wasmUrl) {
    this.state = { ...initialState };
    this.listeners = new Map();
    this.sets = [];
    this.saveChangesCount = 0;
    this.sent = [];
    this.wasmRequests = 0;
    this.wasmUrl = wasmUrl;
    this.wasmReplyDelayMs = 0;
    this.kernel = {
      revision: typeof initialState.revision === 'number' ? initialState.revision : 0,
      projectJson: typeof initialState.project_json === 'string' ? initialState.project_json : '',
    };
    this.snapshots = [];
  }

  get(key) {
    return this.state[key];
  }

  set(key, value) {
    const prev = this.state[key];
    this.state[key] = value;
    this.sets.push({ key, value });
    if (prev !== value) {
      this.trigger(`change:${key}`);
    }
  }

  save_changes() {
    this.saveChangesCount += 1;
  }

  on(name, callback) {
    if (!this.listeners.has(name)) {
      this.listeners.set(name, new Set());
    }
    this.listeners.get(name).add(callback);
  }

  off(name, callback) {
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

  trigger(name, ...args) {
    const set = this.listeners.get(name);
    if (!set) {
      return;
    }
    for (const cb of [...set]) {
      cb(...args);
    }
  }

  // Kernel-side behaviour: the pysimlin ModelWidget answers a wasm request
  // with a custom message whose first binary buffer is the artifact, and a
  // snapshot with saved/rejected. Replies are asynchronous (a real comm
  // round trip never resolves within the sending call).
  send(content, _callbacks, _buffers) {
    this.sent.push(content);
    if (content && content.type === 'snapshot') {
      this.snapshots.push({ base: content.base, json: content.json });
      setTimeout(() => {
        if (content.base !== this.kernel.revision) {
          this.trigger('msg:custom', { type: 'rejected', revision: this.kernel.revision }, []);
          return;
        }
        this.kernel.revision += 1;
        this.kernel.projectJson = content.json;
        this.kernelPush({ project_json: content.json, revision: this.kernel.revision });
        this.trigger('msg:custom', { type: 'saved', revision: this.kernel.revision }, []);
      }, 0);
      return;
    }
    if (content && content.type === 'wasm') {
      this.wasmRequests += 1;
      const reply = async () => {
        const bytes = await (await fetch(this.wasmUrl)).arrayBuffer();
        if (this.wasmReplyDelayMs > 0) {
          await new Promise((r) => setTimeout(r, this.wasmReplyDelayMs));
        }
        this.trigger('msg:custom', { type: 'wasm' }, [new DataView(bytes)]);
      };
      void reply();
    }
  }

  // Kernel-side behaviour: a custom message with no buffers (notices).
  kernelSend(content) {
    this.trigger('msg:custom', content, []);
  }

  // Kernel-side behaviour: the kernel's own state changed (Python edit(),
  // disk reload): bump revision, push both traits, optionally notice.
  kernelChange(projectJson, notice) {
    this.kernel.revision += 1;
    this.kernel.projectJson = projectJson;
    this.kernelPush({ project_json: projectJson, revision: this.kernel.revision });
    if (notice !== undefined) {
      this.kernelSend({ type: 'notice', text: notice });
    }
  }

  // Kernel-side behaviour: push new state as one message (all keys set, then
  // events fire), which is how ipywidgets applies a state update.
  kernelPush(patch) {
    const changed = [];
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
}
