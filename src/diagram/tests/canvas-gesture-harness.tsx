// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/**
 * Reconciler-level test harness for the React `Canvas` component
 * (`drawing/Canvas.tsx`). It renders a real `<Canvas>` through
 * @testing-library/react and drives it with real PointerEvent sequences,
 * asserting only on (a) prop-callback invocations/payloads and (b) rendered
 * DOM. It NEVER reaches into Canvas instance internals (no `new Canvas`, no
 * `.state`, no `setState` shims), so the same suite must survive Canvas
 * becoming a function component. This is the migration gate for tech-debt #65.
 *
 * jsdom gaps polyfilled HERE (never in production code), per the design plan:
 *  - `PointerEvent` (jsdom has none; without it RTL drops pointerId/pointerType/
 *    isPrimary/buttons/clientX-Y/modifiers).
 *  - `DOMMatrix` / `DOMPoint` (used by drawing/common.screenToCanvasPoint).
 *  - `Element.prototype.setPointerCapture` / `releasePointerCapture` /
 *    `hasPointerCapture` (Canvas captures the pointer on press).
 *  - `SVGSVGElement.prototype.createSVGPoint` / `SVGGraphicsElement.prototype.
 *    getScreenCTM` (Flow.handlePointerDown converts the click to model coords
 *    to find the dragged segment index).
 *  - `ResizeObserver` (Canvas observes its container in componentDidMount).
 *  - a fixed `getBoundingClientRect` on the canvas container so clientX/Y map
 *    predictably to canvas coords.
 *
 * Coordinate model: the container bounding rect is the origin, the fixture
 * viewBox is at the origin, and zoom is 1, so screen client coords map 1:1 to
 * model coords. That keeps every gesture's expected geometry trivial: pressing
 * client (x, y) presses model (x, y); a press at A then move to B yields
 * moveDelta = A - B (see Canvas.handleSelectionMove).
 */

import * as React from 'react';
import { act, fireEvent, render, RenderResult } from '@testing-library/react';

import {
  AuxViewElement,
  CloudViewElement,
  FlowViewElement,
  LinkViewElement,
  Model,
  ModuleViewElement,
  Point as DataPoint,
  Project,
  Rect,
  StockViewElement,
  StockFlowView,
  UID,
  Variable,
  ViewElement,
} from '@simlin/core/datamodel';
import { canonicalize } from '@simlin/core/canonicalize';

import { Canvas, CanvasProps } from '../drawing/Canvas';
import type { Point } from '../drawing/common';

// ---------------------------------------------------------------------------
// jsdom polyfills (test-only)
// ---------------------------------------------------------------------------

// A minimal DOMMatrix/DOMPoint sufficient for screenToCanvasPoint, which builds
// `new DOMMatrix([zoom,0,0,zoom,0,0]).inverse()` and applies it to a DOMPoint.
// Only the affine 2x3 subset matters here. `any`-free where possible; the few
// casts below are unavoidable when installing onto the global object.
class FakeDOMMatrix {
  a: number;
  b: number;
  c: number;
  d: number;
  e: number;
  f: number;
  constructor(init?: number[]) {
    const [a = 1, b = 0, c = 0, d = 1, e = 0, f = 0] = init ?? [];
    this.a = a;
    this.b = b;
    this.c = c;
    this.d = d;
    this.e = e;
    this.f = f;
  }
  inverse(): FakeDOMMatrix {
    const det = this.a * this.d - this.b * this.c;
    if (det === 0) {
      throw new Error('non-invertible matrix');
    }
    const ia = this.d / det;
    const ib = -this.b / det;
    const ic = -this.c / det;
    const id = this.a / det;
    const ie = -(ia * this.e + ic * this.f);
    const if_ = -(ib * this.e + id * this.f);
    return new FakeDOMMatrix([ia, ib, ic, id, ie, if_]);
  }
}

class FakeDOMPoint {
  x: number;
  y: number;
  constructor(x = 0, y = 0) {
    this.x = x;
    this.y = y;
  }
  matrixTransform(m: FakeDOMMatrix): FakeDOMPoint {
    return new FakeDOMPoint(m.a * this.x + m.c * this.y + m.e, m.b * this.x + m.d * this.y + m.f);
  }
}

// PointerEvent built on MouseEvent so clientX/Y and modifier keys flow through
// jsdom's existing MouseEvent plumbing; we only add the pointer-specific fields.
class FakePointerEvent extends MouseEvent {
  readonly pointerId: number;
  readonly pointerType: string;
  readonly isPrimary: boolean;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(type: string, params: any = {}) {
    super(type, params);
    this.pointerId = params.pointerId ?? 0;
    this.pointerType = params.pointerType ?? 'mouse';
    this.isPrimary = params.isPrimary ?? false;
  }
}

/** Fixed canvas-container rect: the origin, so client coords == canvas coords. */
const CANVAS_RECT: DOMRect = {
  x: 0,
  y: 0,
  top: 0,
  left: 0,
  right: 1000,
  bottom: 1000,
  width: 1000,
  height: 1000,
  toJSON: () => ({}),
};

// A ResizeObserver that records its callback + observed targets so a test can
// synthesize a resize (jsdom never fires real ones). The Canvas reads the new
// size from `entry.target.clientWidth/Height`, so `triggerResize` sets those on
// each observed element before invoking the callback.
const liveResizeObservers = new Set<FakeResizeObserver>();

class FakeResizeObserver {
  private readonly callback: ResizeObserverCallback;
  readonly targets = new Set<Element>();
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
  }
  observe(target: Element): void {
    this.targets.add(target);
    liveResizeObservers.add(this);
  }
  unobserve(target: Element): void {
    this.targets.delete(target);
  }
  disconnect(): void {
    this.targets.clear();
    liveResizeObservers.delete(this);
  }
  fire(width: number, height: number): void {
    for (const target of this.targets) {
      Object.defineProperty(target, 'clientWidth', { configurable: true, value: width });
      Object.defineProperty(target, 'clientHeight', { configurable: true, value: height });
      const entry = { target, contentRect: { width, height } } as unknown as ResizeObserverEntry;
      this.callback([entry], this as unknown as ResizeObserver);
    }
  }
}

/** Synthesize a resize to `width`x`height` on every live observed element. */
export function triggerResize(width: number, height: number): void {
  act(() => {
    for (const obs of liveResizeObservers) {
      obs.fire(width, height);
    }
  });
}

let polyfillsInstalled = false;

/**
 * Install the jsdom polyfills exactly once. Idempotent so multiple test files
 * importing the harness don't clobber each other. Everything here is global
 * shimming that is harmless to leave installed across the suite.
 */
export function installCanvasPolyfills(): void {
  if (polyfillsInstalled) {
    return;
  }
  polyfillsInstalled = true;

  const g = globalThis as unknown as Record<string, unknown>;
  if (typeof g.PointerEvent !== 'function') {
    g.PointerEvent = FakePointerEvent as unknown as typeof PointerEvent;
  }
  if (typeof g.DOMMatrix !== 'function') {
    g.DOMMatrix = FakeDOMMatrix as unknown as typeof DOMMatrix;
  }
  if (typeof g.DOMPoint !== 'function') {
    g.DOMPoint = FakeDOMPoint as unknown as typeof DOMPoint;
  }
  if (typeof g.ResizeObserver !== 'function') {
    g.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
  }

  // Pointer capture is a no-op in jsdom; Canvas calls it on press.
  const elProto = Element.prototype as unknown as Record<string, unknown>;
  if (typeof elProto.setPointerCapture !== 'function') {
    elProto.setPointerCapture = function setPointerCapture(): void {};
  }
  if (typeof elProto.releasePointerCapture !== 'function') {
    elProto.releasePointerCapture = function releasePointerCapture(): void {};
  }
  if (typeof elProto.hasPointerCapture !== 'function') {
    elProto.hasPointerCapture = function hasPointerCapture(): boolean {
      return false;
    };
  }

  // Flow.handlePointerDown maps the click to model coords via the element's
  // screen CTM. With our origin rect, zoom 1, and viewBox at origin, the CTM is
  // the identity, so model coords == client coords. jsdom's `<path>` extends
  // SVGElement directly (NOT SVGGraphicsElement), so getScreenCTM must live on
  // SVGElement.prototype to be visible to the flow-path node Flow reads.
  const svgSvgProto = SVGSVGElement.prototype as unknown as Record<string, unknown>;
  if (typeof svgSvgProto.createSVGPoint !== 'function') {
    svgSvgProto.createSVGPoint = function createSVGPoint(): FakeDOMPoint {
      return new FakeDOMPoint(0, 0);
    };
  }
  const svgElProto = SVGElement.prototype as unknown as Record<string, unknown>;
  if (typeof svgElProto.getScreenCTM !== 'function') {
    svgElProto.getScreenCTM = function getScreenCTM(): FakeDOMMatrix {
      return new FakeDOMMatrix([1, 0, 0, 1, 0, 0]);
    };
  }
}

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

export function makeAux(uid: number, name: string, x: number, y: number): AuxViewElement {
  return {
    type: 'aux',
    uid,
    var: undefined,
    x,
    y,
    name,
    ident: canonicalize(name),
    labelSide: 'right',
    isZeroRadius: false,
  };
}

export function makeStock(uid: number, name: string, x: number, y: number): StockViewElement {
  return {
    type: 'stock',
    uid,
    var: undefined,
    x,
    y,
    name,
    ident: canonicalize(name),
    labelSide: 'bottom',
    isZeroRadius: false,
    inflows: [],
    outflows: [],
  };
}

export function makeModule(uid: number, name: string, x: number, y: number): ModuleViewElement {
  return {
    type: 'module',
    uid,
    var: undefined,
    x,
    y,
    name,
    ident: canonicalize(name),
    labelSide: 'bottom',
    isZeroRadius: false,
  };
}

export function makeCloud(uid: number, flowUid: number, x: number, y: number): CloudViewElement {
  return {
    type: 'cloud',
    uid,
    flowUid,
    x,
    y,
    isZeroRadius: false,
    ident: undefined,
  };
}

export function makeLink(uid: number, fromUid: number, toUid: number, arc = 0): LinkViewElement {
  return {
    type: 'link',
    uid,
    fromUid,
    toUid,
    arc,
    isStraight: false,
    multiPoint: undefined,
    polarity: undefined,
    x: 0,
    y: 0,
    isZeroRadius: false,
    ident: undefined,
  };
}

export function makeFlow(
  uid: number,
  name: string,
  points: readonly DataPoint[],
  valve: { x: number; y: number },
): FlowViewElement {
  return {
    type: 'flow',
    uid,
    var: undefined,
    name,
    ident: canonicalize(name),
    x: valve.x,
    y: valve.y,
    labelSide: 'bottom',
    points,
    isZeroRadius: false,
  };
}

function makeView(elements: readonly ViewElement[]): StockFlowView {
  const viewBox: Rect = { x: 0, y: 0, width: 1000, height: 1000 };
  const maxUid = elements.reduce((m, e) => Math.max(m, e.uid), 0);
  return {
    nextUid: maxUid + 1,
    elements,
    viewBox,
    zoom: 1,
    useLetteredPolarity: false,
  };
}

function makeModel(view: StockFlowView, variables?: ReadonlyMap<string, Variable>): Model {
  return {
    name: 'main',
    variables: variables ?? new Map<string, Variable>(),
    views: [view],
    loopMetadata: [],
    groups: [],
  };
}

function makeProject(model: Model): Project {
  return {
    name: 'test',
    simSpecs: {
      start: 0,
      stop: 10,
      dt: { value: 1, isReciprocal: false },
      saveStep: undefined,
      simMethod: 'euler',
      timeUnits: undefined,
    },
    models: new Map([[model.name, model]]),
    dimensions: new Map(),
    hasNoEquations: false,
    source: undefined,
  };
}

// Every Canvas prop callback as a jest.fn, so tests assert on exactly which
// fired and with what payload.
export interface CanvasCallbacks {
  onRenameVariable: jest.Mock;
  onSetSelection: jest.Mock;
  onMoveSelection: jest.Mock;
  onMoveFlow: jest.Mock;
  onMoveLabel: jest.Mock;
  onAttachLink: jest.Mock;
  onCreateVariable: jest.Mock;
  onClearSelectedTool: jest.Mock;
  onDeleteSelection: jest.Mock;
  onShowVariableDetails: jest.Mock;
  onViewBoxChange: jest.Mock;
  onDrillIntoModule: jest.Mock;
}

function makeCallbacks(): CanvasCallbacks {
  return {
    onRenameVariable: jest.fn(),
    onSetSelection: jest.fn(),
    onMoveSelection: jest.fn(),
    onMoveFlow: jest.fn(),
    onMoveLabel: jest.fn(),
    onAttachLink: jest.fn(),
    onCreateVariable: jest.fn(),
    onClearSelectedTool: jest.fn(),
    onDeleteSelection: jest.fn(),
    onShowVariableDetails: jest.fn(),
    onViewBoxChange: jest.fn(),
    onDrillIntoModule: jest.fn(),
  };
}

export interface HarnessOptions {
  elements: readonly ViewElement[];
  selection?: ReadonlySet<UID>;
  selectedTool?: CanvasProps['selectedTool'];
  embedded?: boolean;
  variables?: ReadonlyMap<string, Variable>;
  /**
   * When true (the default), `onSetSelection` commits the new selection back
   * into `props.selection` and re-renders -- modeling the real host (Editor),
   * which sets its selection state in the same React event so the resulting
   * re-render sees both the new selection prop AND the new internal interaction
   * state. Several Canvas render paths (e.g. `isValidTarget` doing
   * `only(props.selection)` during an arrowhead drag) assume that batching and
   * would throw if selection lagged behind. Set false only to test a host that
   * deliberately ignores a selection request.
   */
  autoCommitSelection?: boolean;
}

export interface CanvasHarness {
  readonly callbacks: CanvasCallbacks;
  readonly container: HTMLElement;
  readonly svg: SVGSVGElement;
  /** Re-render with updated props (e.g. after the host commits a selection). */
  setProps: (next: Partial<Pick<CanvasProps, 'selection' | 'selectedTool' | 'view'>>) => void;
  rerender: RenderResult['rerender'];
  unmount: RenderResult['unmount'];
  /** Find a rendered element node by the CSS class the element component emits. */
  query: (selector: string) => Element | null;
  queryAll: (selector: string) => Element[];
  /**
   * Clear every callback mock. `componentDidMount` fits the viewBox and so
   * fires `onViewBoxChange` once before any gesture; call this right after
   * render to drop that mount-time noise and assert only on gesture-driven
   * calls.
   */
  clearMountCalls: () => void;
  /**
   * The current `transform` attribute on the canvas content `<g>` -- the live
   * viewport (offset+zoom) the user actually sees. Use this to assert that a
   * gesture updates the view immediately, without (yet) committing through
   * `onViewBoxChange`.
   */
  getTransform: () => string | null;
  /** Synthesize a container resize to `width`x`height` (drives handleSvgResize). */
  resize: (width: number, height: number) => void;
  /**
   * Push a new `props.view` with an overridden viewBox offset / zoom, modeling an
   * EXTERNAL viewport change (centerVariable, module navigation, undo) -- i.e. one
   * that did not originate from a canvas gesture.
   */
  setViewport: (next: { x?: number; y?: number; zoom?: number }) => void;
}

/**
 * Render a `<Canvas>` with the given fixture, returning the harness. The
 * container's bounding rect is pinned to the origin so client coords map
 * directly to canvas coords (see file header).
 */
export function renderCanvas(opts: HarnessOptions): CanvasHarness {
  installCanvasPolyfills();

  const view = makeView(opts.elements);
  const model = makeModel(view, opts.variables);
  const project = makeProject(model);
  const callbacks = makeCallbacks();

  let selection: ReadonlySet<UID> = opts.selection ?? new Set<UID>();
  let selectedTool: CanvasProps['selectedTool'] = opts.selectedTool;
  let currentView = view;
  let version = 1;

  const buildProps = (): CanvasProps => ({
    embedded: opts.embedded ?? false,
    project,
    model,
    view: currentView,
    version,
    selectedTool,
    selection,
    ...callbacks,
  });

  const autoCommit = opts.autoCommitSelection ?? true;
  if (autoCommit) {
    // The jest.fn still records every call for assertions; its implementation
    // commits the selection and re-renders, mirroring the real host. The commit
    // runs inside the same React batch as the Canvas's own setState (we are
    // already inside the event's act()), so the resulting render sees both.
    callbacks.onSetSelection.mockImplementation((next: ReadonlySet<UID>) => {
      selection = next;
      version += 1;
      result.rerender(<Canvas {...buildProps()} />);
    });
  }

  let result!: RenderResult;
  act(() => {
    result = render(<Canvas {...buildProps()} />);
  });

  // Pin the bounding rect on the canvas container div so getCanvasPoint is
  // deterministic. The container is the outer div with the simlin-canvas class.
  const container = result.container.querySelector('.simlin-canvas') as HTMLElement;
  container.getBoundingClientRect = () => CANVAS_RECT;

  const svg = result.container.querySelector('svg') as SVGSVGElement;

  const setProps = (next: Partial<Pick<CanvasProps, 'selection' | 'selectedTool' | 'view'>>): void => {
    if ('selection' in next && next.selection !== undefined) {
      selection = next.selection;
    }
    if ('selectedTool' in next) {
      selectedTool = next.selectedTool;
    }
    if ('view' in next && next.view !== undefined) {
      currentView = next.view;
    }
    version += 1;
    act(() => {
      result.rerender(<Canvas {...buildProps()} />);
    });
  };

  return {
    callbacks,
    container: result.container,
    svg,
    setProps,
    rerender: result.rerender,
    unmount: result.unmount,
    query: (selector: string) => result.container.querySelector(selector),
    queryAll: (selector: string) => Array.from(result.container.querySelectorAll(selector)),
    clearMountCalls: () => {
      for (const fn of Object.values(callbacks)) {
        fn.mockClear();
      }
    },
    getTransform: () => result.container.querySelector('svg g[transform]')?.getAttribute('transform') ?? null,
    resize: (width: number, height: number) => triggerResize(width, height),
    setViewport: (next: { x?: number; y?: number; zoom?: number }) => {
      const nextView: StockFlowView = {
        ...currentView,
        viewBox: {
          ...currentView.viewBox,
          x: next.x ?? currentView.viewBox.x,
          y: next.y ?? currentView.viewBox.y,
        },
        zoom: next.zoom ?? currentView.zoom,
      };
      setProps({ view: nextView });
    },
  };
}

// ---------------------------------------------------------------------------
// Deterministic clock + animation-frame control
// ---------------------------------------------------------------------------

/**
 * Controls `window.performance.now`, `requestAnimationFrame`, and
 * `cancelAnimationFrame` so momentum (an rAF loop) and velocity estimation
 * (which reads the clock) are deterministic. Opt-in per test: install before the
 * gesture, `restore()` in a finally/afterEach. Tests that don't install it keep
 * jsdom's real timers, so the momentum loop never fires (the historical default).
 *
 *  - `tick(ms)` advances virtual time WITHOUT running frames -- use it between
 *    pointer events to set their timestamps (e.g. tick past 40ms before release
 *    to model a deliberate, non-flick stop that starts no momentum).
 *  - `frame(ms)` advances time and runs the currently-pending rAF callbacks once.
 *  - `flush()` runs frames until the rAF queue drains (momentum coasts to its
 *    natural end), bounded by `maxFrames`.
 */
export interface FakeClock {
  tick: (ms: number) => void;
  frame: (ms?: number) => void;
  flush: (maxFrames?: number, ms?: number) => void;
  now: () => number;
  restore: () => void;
}

export function installFakeClock(start = 1000): FakeClock {
  const origRaf = window.requestAnimationFrame;
  const origCancel = window.cancelAnimationFrame;
  const perf = window.performance;
  const origNow = perf.now.bind(perf);

  let now = start;
  let nextId = 1;
  const pending = new Map<number, FrameRequestCallback>();

  window.requestAnimationFrame = ((cb: FrameRequestCallback): number => {
    const id = nextId++;
    pending.set(id, cb);
    return id;
  }) as typeof window.requestAnimationFrame;
  window.cancelAnimationFrame = ((id: number): void => {
    pending.delete(id);
  }) as typeof window.cancelAnimationFrame;
  Object.defineProperty(perf, 'now', { configurable: true, writable: true, value: () => now });

  const runDue = (): void => {
    const due = Array.from(pending.values());
    pending.clear();
    if (due.length === 0) {
      return;
    }
    act(() => {
      for (const cb of due) {
        cb(now);
      }
    });
  };

  return {
    tick: (ms: number) => {
      now += ms;
    },
    frame: (ms = 16) => {
      now += ms;
      runDue();
    },
    flush: (maxFrames = 1000, ms = 16) => {
      let i = 0;
      while (pending.size > 0 && i < maxFrames) {
        now += ms;
        runDue();
        i++;
      }
    },
    now: () => now,
    restore: () => {
      window.requestAnimationFrame = origRaf;
      window.cancelAnimationFrame = origCancel;
      Object.defineProperty(perf, 'now', { configurable: true, writable: true, value: origNow });
    },
  };
}

/**
 * Dispatch a native `wheel` event (the canvas registers a non-passive native
 * wheel listener on the `<svg>`, so React's synthetic `onWheel` would not reach
 * it). Trackpad pinch-zoom arrives as a wheel event with `ctrlKey`/`metaKey`.
 */
export function dispatchWheel(
  target: Element,
  init: {
    deltaX?: number;
    deltaY?: number;
    deltaMode?: number;
    clientX?: number;
    clientY?: number;
    ctrlKey?: boolean;
    metaKey?: boolean;
  } = {},
): void {
  act(() => {
    fireEvent.wheel(target, {
      deltaX: init.deltaX ?? 0,
      deltaY: init.deltaY ?? 0,
      deltaMode: init.deltaMode ?? 0,
      clientX: init.clientX ?? 0,
      clientY: init.clientY ?? 0,
      ctrlKey: init.ctrlKey ?? false,
      metaKey: init.metaKey ?? false,
    });
  });
}

// ---------------------------------------------------------------------------
// Gesture dispatch helpers
// ---------------------------------------------------------------------------

export interface PointerOpts {
  pointerId?: number;
  pointerType?: string;
  isPrimary?: boolean;
  buttons?: number;
  shiftKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
}

function pointerInit(x: number, y: number, opts: PointerOpts): Record<string, unknown> {
  return {
    pointerId: opts.pointerId ?? 1,
    pointerType: opts.pointerType ?? 'mouse',
    isPrimary: opts.isPrimary ?? true,
    buttons: opts.buttons ?? 1,
    clientX: x,
    clientY: y,
    shiftKey: opts.shiftKey ?? false,
    ctrlKey: opts.ctrlKey ?? false,
    metaKey: opts.metaKey ?? false,
  };
}

export function pointerDown(target: Element, x: number, y: number, opts: PointerOpts = {}): void {
  act(() => {
    fireEvent.pointerDown(target, pointerInit(x, y, opts));
  });
}

export function pointerMove(target: Element, x: number, y: number, opts: PointerOpts = {}): void {
  act(() => {
    fireEvent.pointerMove(target, pointerInit(x, y, opts));
  });
}

export function pointerUp(target: Element, x: number, y: number, opts: PointerOpts = {}): void {
  // buttons drops to 0 on release for a mouse.
  act(() => {
    fireEvent.pointerUp(target, pointerInit(x, y, { ...opts, buttons: opts.buttons ?? 0 }));
  });
}

export function pointerCancel(target: Element, x: number, y: number, opts: PointerOpts = {}): void {
  act(() => {
    fireEvent.pointerCancel(target, pointerInit(x, y, { ...opts, buttons: opts.buttons ?? 0 }));
  });
}

/** Convenience: a click that wobbles less than the 5px drag threshold. */
export const subThresholdDelta: Point = { x: 2, y: 2 };
