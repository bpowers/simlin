// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { RequestHandler } from 'express';

export interface HealthResponse {
  readonly status: number;
  readonly body: string;
}

/**
 * Decide the health-check response for the current engine state.
 *
 * The WASM engine preload is the only explicit boot-readiness step the
 * server has (see server-init.ts). In practice a preload failure aborts
 * boot before any route is mounted (initializeServerDependencies()
 * throws and index.ts exits non-zero), so a broken instance surfaces as
 * a connection failure rather than a 503; the 503 branch is cheap
 * defense-in-depth in case that boot ordering ever changes.
 */
export function healthResponse(engineReady: boolean): HealthResponse {
  return engineReady ? { status: 200, body: 'ok' } : { status: 503, body: 'engine not ready' };
}

/**
 * Unauthenticated GET /healthz handler for Cloud Monitoring uptime checks.
 *
 * app.yaml serves '/' as a GAE static file, so '/' stays green even when
 * every Express instance is crash-looping; this route is what proves the
 * Node process itself is up. It must stay cheap (polled frequently) and
 * must never touch Firestore, the session, or any other external service.
 * The readiness probe is injected so the decision logic stays pure.
 */
export function healthz(isEngineReady: () => boolean): RequestHandler {
  return (_req, res) => {
    const { status, body } = healthResponse(isEngineReady());
    // no-store: a cached "ok" from any intermediary would mask an outage
    res.status(status).set('Cache-Control', 'no-store').type('text/plain').send(body);
  };
}
