// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as fs from 'fs';
import * as http from 'http';
import * as os from 'os';
import * as path from 'path';

import express from 'express';

import { favicon } from '../favicon';

const ICON_BYTES = Buffer.from([0x00, 0x00, 0x01, 0x00, 0xde, 0xad, 0xbe, 0xef]);

function request(
  server: http.Server,
  method: string,
  reqPath: string,
): Promise<{ status: number; headers: http.IncomingHttpHeaders; body: Buffer }> {
  return new Promise((resolve, reject) => {
    const addr = server.address();
    if (!addr || typeof addr === 'string') {
      return reject(new Error('server not listening'));
    }
    const req = http.request({ hostname: '127.0.0.1', port: addr.port, path: reqPath, method }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => {
        resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks) });
      });
    });
    req.on('error', reject);
    req.end();
  });
}

describe('favicon middleware', () => {
  let tempDir: string;
  let iconPath: string;
  let server: http.Server;

  beforeAll(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'favicon-test-'));
    iconPath = path.join(tempDir, 'favicon.ico');
    fs.writeFileSync(iconPath, ICON_BYTES);

    const app = express();
    app.use(favicon(iconPath));
    app.get('/other', (_req, res) => {
      res.status(200).send('fell through');
    });
    server = app.listen(0);
  });

  afterAll(() => {
    server.close();
    fs.rmSync(tempDir, { recursive: true, force: true });
  });

  it('serves the icon bytes with caching headers on GET /favicon.ico', async () => {
    const res = await request(server, 'GET', '/favicon.ico');
    expect(res.status).toBe(200);
    expect(res.headers['content-type']).toBe('image/x-icon');
    expect(res.headers['cache-control']).toContain('max-age');
    expect(res.body.equals(ICON_BYTES)).toBe(true);
  });

  it('rejects non-GET/HEAD methods with 405 and an Allow header', async () => {
    const res = await request(server, 'POST', '/favicon.ico');
    expect(res.status).toBe(405);
    expect(res.headers['allow']).toBe('GET, HEAD');
  });

  it('defers every other path to the rest of the stack', async () => {
    const res = await request(server, 'GET', '/other');
    expect(res.status).toBe(200);
    expect(res.body.toString()).toBe('fell through');
  });
});
