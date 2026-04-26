// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { UpdatesSocket } from './ws';
import type { WsMessage } from './ws';

// Hand-rolled WebSocket double. We avoid `jest-websocket-mock` so the test
// suite stays dependency-light and so we have direct control over the timing
// of `onopen`/`onmessage`/`onclose`/`onerror` from individual tests. Each
// MockWebSocket records its construction URL so `UpdatesSocket`'s URL
// formation (token query-param encoding) is observable.
class MockWebSocket {
  static CONNECTING = 0 as const;
  static OPEN = 1 as const;
  static CLOSING = 2 as const;
  static CLOSED = 3 as const;

  readonly url: string;
  readyState: number = MockWebSocket.CONNECTING;
  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  closeArgs: { code?: number; reason?: string } | null = null;
  // Recorded payloads from `send()`. We record the raw stringified
  // frame (matching what the production WebSocket would have written
  // to the wire) so tests can assert the exact JSON envelope.
  sentFrames: Array<string> = [];

  static instances: Array<MockWebSocket> = [];

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  open(): void {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event('open'));
  }

  emitMessage(data: string): void {
    this.onmessage?.(new MessageEvent('message', { data }));
  }

  emitClose(code = 1006): void {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent('close', { code }));
  }

  emitError(): void {
    this.onerror?.(new Event('error'));
  }

  close(code?: number, reason?: string): void {
    this.closeArgs = { code, reason };
    this.readyState = MockWebSocket.CLOSED;
  }

  send(data: string): void {
    this.sentFrames.push(data);
  }
}

let originalWebSocket: typeof globalThis.WebSocket | undefined;

beforeEach(() => {
  MockWebSocket.instances = [];
  originalWebSocket = globalThis.WebSocket;
  globalThis.WebSocket = MockWebSocket as unknown as typeof globalThis.WebSocket;
});

afterEach(() => {
  jest.useRealTimers();
  if (originalWebSocket) {
    globalThis.WebSocket = originalWebSocket;
  } else {
    delete (globalThis as Partial<typeof globalThis>).WebSocket;
  }
});

describe('UpdatesSocket', () => {
  test('opens the WebSocket against /api/updates with a URL-encoded token', () => {
    const socket = new UpdatesSocket('tok with/space&plus', () => {
      // unused
    });

    expect(MockWebSocket.instances).toHaveLength(1);
    const url = MockWebSocket.instances[0].url;
    // URL form: ws://<host>/api/updates?token=<encoded>. We assert on the
    // suffix because jsdom's location.host varies but the path + query
    // should be deterministic across test environments.
    expect(url).toMatch(/^ws:\/\/[^/]+\/api\/updates\?token=/);
    expect(url).toContain(`token=${encodeURIComponent('tok with/space&plus')}`);
    socket.close();
  });

  test('parses incoming messages and forwards them to onMessage', () => {
    const onMessage = jest.fn<void, [WsMessage]>();
    const socket = new UpdatesSocket('t', onMessage);
    const ws = MockWebSocket.instances[0];

    ws.open();
    ws.emitMessage(
      JSON.stringify({
        type: 'projectChanged',
        path: 'a.stmx',
        version: 5,
        source: 'user',
      }),
    );

    expect(onMessage).toHaveBeenCalledTimes(1);
    expect(onMessage).toHaveBeenCalledWith({
      type: 'projectChanged',
      path: 'a.stmx',
      version: 5,
      source: 'user',
    });
    socket.close();
  });

  test('parses projectRemoved frames and forwards them to onMessage', () => {
    const onMessage = jest.fn<void, [WsMessage]>();
    const socket = new UpdatesSocket('t', onMessage);
    const ws = MockWebSocket.instances[0];

    ws.open();
    ws.emitMessage(
      JSON.stringify({
        type: 'projectRemoved',
        path: 'a.stmx',
      }),
    );

    expect(onMessage).toHaveBeenCalledTimes(1);
    expect(onMessage).toHaveBeenCalledWith({
      type: 'projectRemoved',
      path: 'a.stmx',
    });
    socket.close();
  });

  test('drops projectRemoved frames missing the path field', () => {
    const onMessage = jest.fn<void, [WsMessage]>();
    const socket = new UpdatesSocket('t', onMessage);
    const ws = MockWebSocket.instances[0];

    ws.open();
    ws.emitMessage(JSON.stringify({ type: 'projectRemoved' }));

    expect(onMessage).not.toHaveBeenCalled();
    socket.close();
  });

  test('ignores message frames whose body is not valid JSON without throwing', () => {
    const onMessage = jest.fn<void, [WsMessage]>();
    const socket = new UpdatesSocket('t', onMessage);
    const ws = MockWebSocket.instances[0];

    ws.open();
    ws.emitMessage('this is not json');

    expect(onMessage).not.toHaveBeenCalled();
    socket.close();
  });

  test('reconnects with 1s/2s/5s backoff after consecutive close events', () => {
    jest.useFakeTimers();

    const onMessage = jest.fn<void, [WsMessage]>();
    const socket = new UpdatesSocket('t', onMessage);
    expect(MockWebSocket.instances).toHaveLength(1);

    // First close: should schedule a reconnect at 1s.
    MockWebSocket.instances[0].emitClose();
    jest.advanceTimersByTime(999);
    expect(MockWebSocket.instances).toHaveLength(1);
    jest.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(2);

    // Second consecutive close (no successful message in between): 2s backoff.
    MockWebSocket.instances[1].emitClose();
    jest.advanceTimersByTime(1999);
    expect(MockWebSocket.instances).toHaveLength(2);
    jest.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(3);

    // Third consecutive close: capped at 5s.
    MockWebSocket.instances[2].emitClose();
    jest.advanceTimersByTime(4999);
    expect(MockWebSocket.instances).toHaveLength(3);
    jest.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(4);

    // Fourth consecutive close: still capped at 5s.
    MockWebSocket.instances[3].emitClose();
    jest.advanceTimersByTime(5000);
    expect(MockWebSocket.instances).toHaveLength(5);

    socket.close();
  });

  test('resets backoff after a successful message before the next close', () => {
    jest.useFakeTimers();

    const onMessage = jest.fn<void, [WsMessage]>();
    const socket = new UpdatesSocket('t', onMessage);

    // Drive backoff up via two consecutive closes.
    MockWebSocket.instances[0].emitClose();
    jest.advanceTimersByTime(1000);
    expect(MockWebSocket.instances).toHaveLength(2);
    MockWebSocket.instances[1].emitClose();
    jest.advanceTimersByTime(2000);
    expect(MockWebSocket.instances).toHaveLength(3);

    // Receive a successful message: backoff resets.
    MockWebSocket.instances[2].open();
    MockWebSocket.instances[2].emitMessage(
      JSON.stringify({ type: 'projectChanged', path: 'p', version: 1, source: 'user' }),
    );

    // Next close should schedule at 1s, not 5s.
    MockWebSocket.instances[2].emitClose();
    jest.advanceTimersByTime(999);
    expect(MockWebSocket.instances).toHaveLength(3);
    jest.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(4);

    socket.close();
  });

  test('error events also trigger backoff reconnect', () => {
    jest.useFakeTimers();

    const socket = new UpdatesSocket('t', () => {
      // unused
    });
    MockWebSocket.instances[0].emitError();
    MockWebSocket.instances[0].emitClose();
    jest.advanceTimersByTime(1000);
    expect(MockWebSocket.instances).toHaveLength(2);

    socket.close();
  });

  test('close() prevents further reconnect attempts', () => {
    jest.useFakeTimers();

    const socket = new UpdatesSocket('t', () => {
      // unused
    });
    socket.close();

    // Even if the underlying socket emits a close, no new connection
    // should be created.
    MockWebSocket.instances[0].emitClose();
    jest.advanceTimersByTime(10_000);
    expect(MockWebSocket.instances).toHaveLength(1);
  });

  test('close() closes the underlying WebSocket', () => {
    const socket = new UpdatesSocket('t', () => {
      // unused
    });
    const ws = MockWebSocket.instances[0];
    expect(ws.closeArgs).toBeNull();
    socket.close();
    expect(ws.closeArgs).not.toBeNull();
  });

  test('send() writes the JSON-encoded ClientWsMessage when the socket is open', () => {
    const socket = new UpdatesSocket('t', () => {
      // unused
    });
    const ws = MockWebSocket.instances[0];
    ws.open();

    socket.send({ type: 'projectFocused', path: 'a.stmx' });

    expect(ws.sentFrames).toHaveLength(1);
    expect(JSON.parse(ws.sentFrames[0])).toEqual({
      type: 'projectFocused',
      path: 'a.stmx',
    });

    socket.close();
  });

  test('send() encodes selectionChanged frames with variableIdents', () => {
    const socket = new UpdatesSocket('t', () => {
      // unused
    });
    const ws = MockWebSocket.instances[0];
    ws.open();

    socket.send({
      type: 'selectionChanged',
      path: 'a.stmx',
      variableIdents: ['teacup_temperature', 'ambient'],
    });

    expect(ws.sentFrames).toHaveLength(1);
    expect(JSON.parse(ws.sentFrames[0])).toEqual({
      type: 'selectionChanged',
      path: 'a.stmx',
      variableIdents: ['teacup_temperature', 'ambient'],
    });

    socket.close();
  });

  test('send() drops frames when the socket is not yet open', () => {
    // The socket is in CONNECTING state until `open()` is invoked. Sending
    // before the connection is up would throw on a real WebSocket; the
    // public contract here is to drop silently so transient timing windows
    // around mount/unmount don't tear down the app.
    const socket = new UpdatesSocket('t', () => {
      // unused
    });
    const ws = MockWebSocket.instances[0];

    socket.send({ type: 'projectFocused', path: 'a.stmx' });

    expect(ws.sentFrames).toHaveLength(0);
    socket.close();
  });

  test('send() drops frames after close() has been called', () => {
    const socket = new UpdatesSocket('t', () => {
      // unused
    });
    const ws = MockWebSocket.instances[0];
    ws.open();
    socket.close();

    socket.send({ type: 'projectFocused', path: 'a.stmx' });

    expect(ws.sentFrames).toHaveLength(0);
  });
});
