// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Browser-side client for the simlin-serve `/api/updates` WebSocket. The
// upgrade is gated by a `?token=...` query param (see Phase 3 Note 7: native
// browser `WebSocket` cannot set custom request headers, so the bearer must
// ride in the URL).
//
// This module is the Imperative Shell for the live-update channel: it wraps
// the raw WebSocket lifecycle (open, message, close, error) with reconnect
// behavior and surfaces a typed `WsMessage` callback to the caller. It does
// not interpret the message contents — `App.tsx` decides what to do with
// each event.

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
// reconnecting. This caps infinite retry loops caused by persistent auth
// failures (e.g. a stale token after a server restart). The caller can
// detect the give-up state via the optional `onStatus` callback; it is
// intentionally left up to the call site to decide whether to surface a
// user-visible indicator or attempt recovery (e.g. by constructing a
// new UpdatesSocket with a fresh token).
const MAX_CONSECUTIVE_FAILURES = 10;

function reconnectDelay(consecutiveFailures: number): number {
  const idx = Math.min(consecutiveFailures, RECONNECT_DELAYS_MS.length - 1);
  return RECONNECT_DELAYS_MS[idx];
}

function buildUrl(token: string): string {
  // location.host carries port + hostname so the dev-mode and bound-port
  // flows both work without extra config. The token is URL-encoded so
  // characters like `/` and `&` survive transit intact.
  return `ws://${location.host}/api/updates?token=${encodeURIComponent(token)}`;
}

export class UpdatesSocket {
  private readonly token: string;
  private readonly onMessage: OnMessageFn;
  private readonly onStatus: OnStatusFn | undefined;
  private socket: WebSocket | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  // Number of consecutive failures since the last successful message.
  // A successful message resets to 0 so a stable connection that
  // eventually drops goes through the fast (1s) retry again. This is
  // the loop-prevention requirement called out in the phase plan: a
  // long-lived connection should not be punished with a 5s reconnect
  // when it finally drops.
  private consecutiveFailures: number = 0;
  private closed: boolean = false;

  constructor(token: string, onMessage: OnMessageFn, onStatus?: OnStatusFn) {
    this.token = token;
    this.onMessage = onMessage;
    this.onStatus = onStatus;
    this.connect();
  }

  close(): void {
    this.closed = true;
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
  // WebSocket. Frames issued before the connection finishes opening or
  // after `close()` are dropped silently rather than queued: a focus or
  // selection event from a torn-down editor is stale by the time the
  // socket recovers, and the next mount will emit a fresh one. Replays
  // would also race with the server's view of which session "owns"
  // these intent signals.
  send(msg: ClientWsMessage): void {
    if (this.closed) {
      return;
    }
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
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
      socket = new WebSocket(buildUrl(this.token));
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
    socket.onmessage = (event) => this.handleMessage(event);
    socket.onclose = () => this.handleClose();
    socket.onerror = () => this.handleError();
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
  if (typeof v.path !== 'string') {
    return false;
  }
  if (v.type === 'projectChanged') {
    if (typeof v.version !== 'number') {
      return false;
    }
    if (v.source !== 'user' && v.source !== 'agent' && v.source !== 'disk') {
      return false;
    }
    return true;
  }
  if (v.type === 'projectRemoved') {
    return true;
  }
  return false;
}
