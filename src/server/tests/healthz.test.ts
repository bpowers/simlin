// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as http from 'http';

import express from 'express';

import { healthResponse, healthz } from '../healthz';

function request(
  server: http.Server,
  method: string,
  reqPath: string,
): Promise<{ status: number; headers: http.IncomingHttpHeaders; body: string }> {
  return new Promise((resolve, reject) => {
    const addr = server.address();
    if (!addr || typeof addr === 'string') {
      return reject(new Error('server not listening'));
    }
    const req = http.request({ hostname: '127.0.0.1', port: addr.port, path: reqPath, method }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => {
        resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf-8') });
      });
    });
    req.on('error', reject);
    req.end();
  });
}

describe('healthResponse', () => {
  it('is 200 ok when the engine is ready', () => {
    expect(healthResponse(true)).toEqual({ status: 200, body: 'ok' });
  });

  it('is 503 when the engine is not ready', () => {
    expect(healthResponse(false)).toEqual({ status: 503, body: 'engine not ready' });
  });
});

describe('healthz route', () => {
  let server: http.Server;
  let engineReady: boolean;

  beforeAll(() => {
    engineReady = true;

    const app = express();
    app.get(
      '/healthz',
      healthz(() => engineReady),
    );
    server = app.listen(0);
  });

  afterAll(async () => {
    await new Promise<void>((resolve, reject) => {
      server.close((err) => (err ? reject(err) : resolve()));
    });
  });

  it('returns 200 ok when the readiness probe passes', async () => {
    engineReady = true;
    const res = await request(server, 'GET', '/healthz');
    expect(res.status).toBe(200);
    expect(res.body).toBe('ok');
    expect(res.headers['content-type']).toContain('text/plain');
    // uptime checks poll this endpoint; a cached response would mask an outage
    expect(res.headers['cache-control']).toBe('no-store');
  });

  it('returns 503 when the readiness probe fails', async () => {
    engineReady = false;
    const res = await request(server, 'GET', '/healthz');
    expect(res.status).toBe(503);
    expect(res.body).toBe('engine not ready');
    expect(res.headers['cache-control']).toBe('no-store');
  });

  it('answers HEAD requests with the same status and no body', async () => {
    engineReady = true;
    const res = await request(server, 'HEAD', '/healthz');
    expect(res.status).toBe(200);
    expect(res.body).toBe('');
  });

  it('is scoped to GET/HEAD: POST is not handled', async () => {
    const res = await request(server, 'POST', '/healthz');
    expect(res.status).toBe(404);
  });
});
