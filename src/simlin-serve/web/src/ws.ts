// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Browser-side client for the simlin-serve `/api/updates` WebSocket.
//
// This module is the Imperative Shell for the live-update channel: it wraps
// the raw WebSocket lifecycle (open, message, close, error) with reconnect
// behavior and surfaces a typed `WsMessage` callback to the caller. It does
// not interpret the message contents — `App.tsx` decides what to do with
// each event.
//
// V1 has no bearer-token gate (see docs/threat-model.md); the host- and
// origin-allowlist on the server is what defends against cross-origin
// browsers reaching the loopback port.

export type ChangeSource = 'user' | 'agent' | 'disk';

// Wire shape mirrors `simlin_serve::events::WsMessage` (camelCase via
// `serde(tag = "type", rename_all = "camelCase")`). Future variants will be
// added here as they ship server-side; the union keeps the parse path
// strongly typed at the call site.
export type WsMessage =
  | {
      readonly type: 'projectChanged';
      readonly path: string;
      readonly version: number;
      readonly source: ChangeSource;
    }
  | {
      readonly type: 'projectRemoved';
      readonly path: string;
    }
  | {
      readonly type: 'projectRenamed';
      readonly from: string;
      readonly to: string;
    };

// Browser-originated frames sent over the same channel. Mirrors
// `simlin_serve::events::ClientWsMessage` (the inbound counterpart parsed
// by `handle_socket`); diagnostics-style events have no client-side
// counterpart and must not appear here.
export type ClientWsMessage =
  | {
      readonly type: 'projectFocused';
      readonly path: string;
    }
  | {
      readonly type: 'selectionChanged';
      readonly path: string;
      readonly variableIdents: ReadonlyArray<string>;
    };

type OnMessageFn = (msg: WsMessage) => void;
type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'dead';
type OnStatusFn = (status: ConnectionStatus) => void;

// Backoff schedule in milliseconds. Index N is the delay before reconnect
// attempt N+1 after consecutive failures. The last value caps the schedule:
// once we hit it, subsequent failures keep that same delay rather than
// growing unboundedly. Reset on first successful message receipt.
const RECONNECT_DELAYS_MS: ReadonlyArray<number> = [1000, 2000, 5000];

// After this many consecutive failures with no successful frame we stop
// reconnecting. This caps infinite retry loops caused by a server that
// stopped responding (process exit, port collision after restart). The
// caller can detect the give-up state via the optional `onStatus`
// callback; it is intentionally left up to the call site to decide
// whether to surface a user-visible indicator or attempt recovery.
const MAX_CONSECUTIVE_FAILURES = 10;

function reconnectDelay(consecutiveFailures: number): number {
  const idx = Math.min(consecutiveFailures, RECONNECT_DELAYS_MS.length - 1);
  return RECONNECT_DELAYS_MS[idx];
}

function buildUrl(): string {
  // location.host carries port + hostname so the dev-mode and bound-port
  // flows both work without extra config.
  return `ws://${location.host}/api/updates`;
}

export class UpdatesSocket {
  private readonly onMessage: OnMessageFn;
  private readonly onStatus: OnStatusFn | undefined;
  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  // Number of consecutive failures since the last successful open. A
  // successful open resets to 0 so a long-lived connection that goes
  // through a quiet period (no broadcast frames) and eventually drops
  // is not punished with the 5s cap on reconnect.
  private consecutiveFailures: number = 0;
  private closed: boolean = false;
  // At most one pending projectFocused frame queued while the socket is
  // still opening. Only projectFocused is buffered: it carries persistent
  // intent (which project is the user looking at) that must reach the
  // server even when the first send() call races with the WS handshake.
  // selectionChanged frames issued before open are stale by the time the
  // socket recovers and are intentionally dropped. Each new projectFocused
  // replaces any previously buffered one (only the latest focus matters).
  private pendingFocusedFrame: ClientWsMessage | null = null;

  constructor(onMessage: OnMessageFn, onStatus?: OnStatusFn) {
    this.onMessage = onMessage;
    this.onStatus = onStatus;
    this.connect();
  }

  close(): void {
    this.closed = true;
    this.pendingFocusedFrame = null;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.socket) {
      this.socket.close();
      this.socket = null;
    }
  }

  // Sends a client-originated frame (focus, selection) over the active
  // WebSocket. If the socket is not yet OPEN:
  // - projectFocused frames are queued (at most one; a newer one replaces
  //   any prior pending one) and flushed when onopen fires. This handles
  //   the race between App.componentDidMount opening the socket and
  //   EditorHost.componentDidMount emitting the initial projectFocused.
  // - selectionChanged frames are dropped silently; a selection during
  //   a reconnect window is stale by the time the socket recovers.
  // Frames after close() are always dropped.
  send(msg: ClientWsMessage): void {
    if (this.closed) {
      return;
    }
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      if (msg.type === 'projectFocused') {
        this.pendingFocusedFrame = msg;
      }
      return;
    }
    socket.send(JSON.stringify(msg));
  }

  private connect(): void {
    if (this.closed) {
      return;
    }
    let socket: WebSocket;
    try {
      socket = new WebSocket(buildUrl());
    } catch (err) {
      // The WebSocket constructor throws on syntactically invalid URLs.
      // Surface to console (a thrown error here is a configuration bug,
      // not transient) and schedule a backoff anyway so a transient
      // env issue doesn't permanently break live updates.
      console.error('UpdatesSocket: failed to construct WebSocket', err);
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;
    socket.onopen = () => this.handleOpen();
    socket.onmessage = (event) => this.handleMessage(event);
    socket.onclose = () => this.handleClose();
    socket.onerror = () => this.handleError();
  }

  private handleOpen(): void {
    const pending = this.pendingFocusedFrame;
    if (pending !== null) {
      this.pendingFocusedFrame = null;
      const socket = this.socket;
      if (socket && socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify(pending));
      }
    }
  }

  private handleMessage(event: MessageEvent): void {
    const data = event.data;
    if (typeof data !== 'string') {
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(data);
    } catch {
      // Drop malformed frames silently. The server only sends JSON; a
      // malformed frame indicates a client/server mismatch we'd rather
      // log-and-continue on than throw and tear down the connection.
      console.warn('UpdatesSocket: dropped non-JSON frame');
      return;
    }
    if (!isWsMessage(parsed)) {
      console.warn('UpdatesSocket: dropped frame with unknown shape', parsed);
      return;
    }
    if (this.consecutiveFailures !== 0) {
      this.consecutiveFailures = 0;
      this.onStatus?.('connected');
    }
    this.onMessage(parsed);
  }

  private handleClose(): void {
    this.socket = null;
    this.scheduleReconnect();
  }

  private handleError(): void {
    // Errors are also followed by a `close` event in the standard
    // browser WebSocket, so we don't schedule from here. We log so a
    // pile-up of error events is visible in DevTools.
    console.warn('UpdatesSocket: WebSocket error');
  }

  private scheduleReconnect(): void {
    if (this.closed) {
      return;
    }
    if (this.reconnectTimer !== null) {
      return;
    }
    if (this.consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) {
      // Persistent failure: give up so we don't loop forever on stale
      // tokens or a server that has restarted with a different auth config.
      // The caller can construct a new UpdatesSocket if/when recovery is
      // appropriate (e.g. after re-authenticating).
      this.closed = true;
      this.onStatus?.('dead');
      return;
    }
    this.onStatus?.('connecting');
    const delay = reconnectDelay(this.consecutiveFailures);
    this.consecutiveFailures += 1;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }
}

export type { ConnectionStatus, OnStatusFn };

function isWsMessage(value: unknown): value is WsMessage {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const v = value as Record<string, unknown>;
  if (v.type === 'projectChanged') {
    if (typeof v.path !== 'string' || typeof v.version !== 'number') {
      return false;
    }
    if (v.source !== 'user' && v.source !== 'agent' && v.source !== 'disk') {
      return false;
    }
    return true;
  }
  if (v.type === 'projectRemoved') {
    return typeof v.path === 'string';
  }
  if (v.type === 'projectRenamed') {
    return typeof v.from === 'string' && typeof v.to === 'string';
  }
  return false;
}
