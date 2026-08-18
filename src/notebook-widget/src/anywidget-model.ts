// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * The slice of anywidget's `AnyModel` this widget uses, spelled out locally so
 * the shell and its tests share one contract without a dependency on
 * `@anywidget/types`. Shape verified against anywidget 0.11.0
 * (packages/anywidget/src/model-proxy.ts, binding.ts, and the AFM spec at
 * https://anywidget.dev/en/afm/): `send(content, callbacks?, buffers?)`;
 * `on('msg:custom', (msg, buffers) => ...)` where the buffers arrive as
 * `DataView`s in JupyterLab (ipywidgets' `_handle_comm_msg`) but the spec
 * types them loosely, so `parseWasmReply` accepts any ArrayBuffer view.
 */
export interface AnyModel {
  get(key: string): unknown;
  set(key: string, value: unknown): void;
  save_changes(): void;
  send(content: unknown, callbacks?: unknown, buffers?: ArrayBuffer[] | ArrayBufferView[]): void;
  on(eventName: string, callback: (...args: unknown[]) => void): void;
  off(eventName?: string | null, callback?: ((...args: unknown[]) => void) | null): void;
}

/**
 * The AFM lifecycle hooks (anywidget >= 0.9 default-export shape). `signal`
 * aborts when the model (initialize) or view (render) is destroyed.
 */
export interface RenderContext {
  model: AnyModel;
  el: HTMLElement;
  signal?: AbortSignal;
}

export interface InitializeContext {
  model: AnyModel;
  signal?: AbortSignal;
}
