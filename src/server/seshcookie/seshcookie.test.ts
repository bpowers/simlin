// Copyright 2018 Bobby Powers. Licensed under the ISC license (see
// LICENSE in this directory).
//
// Vendored from https://github.com/bpowers/seshcookie-js at commit
// 46aef15d1bb267dd17a680b5dbf657c12fbddad1, with the node:test runner
// imports adapted to jest's globals (before/after -> beforeAll/afterAll);
// the node:assert assertions are unchanged.

import * as assert from 'node:assert/strict';
import * as crypto from 'node:crypto';
import * as http from 'node:http';
import { AddressInfo } from 'node:net';

import express from 'express';

import * as seshcookie from './seshcookie';

function newConfig(): seshcookie.Options {
  return {
    key: crypto.randomBytes(8).toString('hex'),
    cookieName: 'unittest',
    cookiePath: '/',
    httpOnly: true,
    secure: false,
    maxAgeInSeconds: 60 * 60, // 1 hour
    sameSite: 'strict',
  };
}

function listen(server: http.Server): Promise<string> {
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address() as AddressInfo;
      resolve(`http://127.0.0.1:${addr.port}`);
    });
  });
}

// a minimal cookie jar: holds the single name=value pair from the most
// recent Set-Cookie response header.
class Jar {
  cookie = '';

  update(res: Response): string | undefined {
    const setCookies = res.headers.getSetCookie();
    if (setCookies.length === 0) {
      return undefined;
    }
    assert.equal(setCookies.length, 1, 'expected at most 1 Set-Cookie header');
    const setCookie = setCookies[0];
    const pair = setCookie.split(';')[0];
    // an empty value or Max-Age=0 means the cookie was cleared
    this.cookie = pair.endsWith('=') || /;\s*Max-Age=0(;|$)/i.test(setCookie) ? '' : pair;
    return setCookie;
  }

  async get(url: string): Promise<{ res: Response; setCookie: string | undefined }> {
    const headers: Record<string, string> = {};
    if (this.cookie) {
      headers.cookie = this.cookie;
    }
    const res = await fetch(url, { headers });
    const setCookie = this.update(res);
    return { res, setCookie };
  }
}

describe('encryption roundtrips', () => {
  it('works', () => {
    const key = crypto.randomBytes(16);
    const value = 'it was the best of times';
    const ciphertext = seshcookie.encrypt(Buffer.from(value), key);
    assert.notEqual(ciphertext, value);
    const plaintext = seshcookie.decrypt(ciphertext, key);
    assert.equal(plaintext, value);
  });
});

describe('getCookie', () => {
  it('finds the named cookie among several', () => {
    assert.equal(seshcookie.getCookie('a=1; b=2; c=3', 'b'), '2');
    assert.equal(seshcookie.getCookie('a=1; b=2; c=3', 'a'), '1');
    assert.equal(seshcookie.getCookie('a=1; b=2; c=3', 'c'), '3');
  });

  it('returns undefined when missing', () => {
    assert.equal(seshcookie.getCookie(undefined, 'a'), undefined);
    assert.equal(seshcookie.getCookie('', 'a'), undefined);
    assert.equal(seshcookie.getCookie('a=1', 'b'), undefined);
    assert.equal(seshcookie.getCookie('garbage', 'garbage'), undefined);
  });

  it('does not match on name prefixes or suffixes', () => {
    assert.equal(seshcookie.getCookie('xa=1', 'a'), undefined);
    assert.equal(seshcookie.getCookie('ax=1', 'a'), undefined);
  });

  it('URL-decodes values', () => {
    assert.equal(seshcookie.getCookie('a=hello%2Fworld%3D', 'a'), 'hello/world=');
  });

  it('strips double quotes', () => {
    assert.equal(seshcookie.getCookie('a="quoted"', 'a'), 'quoted');
  });

  it('tolerates malformed percent-encoding', () => {
    assert.equal(seshcookie.getCookie('a=%zz', 'a'), undefined);
  });
});

describe('serializeCookie', () => {
  it('serializes all attributes', () => {
    const cookie = seshcookie.serializeCookie('name', 'a/b=', {
      path: '/app',
      httpOnly: true,
      secure: true,
      maxAgeInSeconds: 60,
      sameSite: 'lax',
    });
    assert.equal(cookie, 'name=a%2Fb%3D; Max-Age=60; Path=/app; HttpOnly; Secure; SameSite=Lax');
  });

  it('maps sameSite=true to Strict and omits false', () => {
    assert.match(
      seshcookie.serializeCookie('n', 'v', {
        path: '/',
        httpOnly: false,
        secure: false,
        sameSite: true,
      }),
      /; SameSite=Strict$/,
    );
    assert.doesNotMatch(
      seshcookie.serializeCookie('n', 'v', {
        path: '/',
        httpOnly: false,
        secure: false,
        sameSite: false,
      }),
      /SameSite/,
    );
  });
});

interface TestServer {
  url: string;
  server: http.Server;
}

async function startExpressApp(config: seshcookie.Options): Promise<TestServer> {
  const app = express();
  app.use(seshcookie.seshcookie(config));

  app.get('/set/:name', (req: express.Request, res: express.Response) => {
    req.session.name = req.params.name;
    res.status(200).json({ name: req.params.name });
  });

  app.get('/get', (req: express.Request, res: express.Response) => {
    const name = req.session.name as string | undefined;
    const status = name !== undefined ? 200 : 204;
    res.status(status).json({ name });
  });

  app.get('/clear', (req: express.Request, res: express.Response) => {
    req.session = {};
    res.status(200).json({});
  });

  const server = http.createServer(app);
  const url = await listen(server);
  return { url, server };
}

describe('express integration', () => {
  const config = newConfig();
  let ts: TestServer;

  beforeAll(async () => {
    ts = await startExpressApp(config);
  });
  afterAll(() => {
    ts.server.close();
  });

  it('roundtrips a session through the cookie', async () => {
    const jar = new Jar();
    const name = crypto.randomBytes(8).toString('hex');

    const { res, setCookie } = await jar.get(`${ts.url}/set/${name}`);
    assert.equal(res.status, 200);
    assert.ok(setCookie, 'expected a Set-Cookie header');
    assert.match(setCookie, new RegExp(`^${config.cookieName}=.`));
    assert.match(setCookie, /;\s*Max-Age=3600(;|$)/);
    assert.match(setCookie, /;\s*Path=\/(;|$)/);
    assert.match(setCookie, /;\s*HttpOnly(;|$)/);
    assert.match(setCookie, /;\s*SameSite=Strict(;|$)/);

    const { res: res2 } = await jar.get(`${ts.url}/get`);
    assert.equal(res2.status, 200);
    const body = (await res2.json()) as { name?: string };
    assert.equal(body.name, name);
  });

  it('does not re-set an unchanged session', async () => {
    const jar = new Jar();
    const name = crypto.randomBytes(8).toString('hex');

    await jar.get(`${ts.url}/set/${name}`);
    const { res, setCookie } = await jar.get(`${ts.url}/get`);
    assert.equal(res.status, 200);
    assert.equal(setCookie, undefined);
  });

  it('tolerates a corrupted cookie', async () => {
    const jar = new Jar();
    const name = crypto.randomBytes(8).toString('hex');

    await jar.get(`${ts.url}/set/${name}`);
    // delete a char from the encrypted payload
    jar.cookie = jar.cookie.replace(/-./, '-');
    const { res } = await jar.get(`${ts.url}/get`);
    assert.equal(res.status, 204);
  });

  it('clears the cookie for an emptied session', async () => {
    const jar = new Jar();
    const name = crypto.randomBytes(8).toString('hex');

    await jar.get(`${ts.url}/set/${name}`);
    assert.ok(jar.cookie);

    const { res, setCookie } = await jar.get(`${ts.url}/clear`);
    assert.equal(res.status, 200);
    assert.ok(setCookie, 'expected a clearing Set-Cookie header');
    assert.match(setCookie, /;\s*Max-Age=0(;|$)/);
    assert.equal(jar.cookie, '');

    const { res: res2 } = await jar.get(`${ts.url}/get`);
    assert.equal(res2.status, 204);
  });
});

async function startBareHttpServer(config: seshcookie.Options): Promise<TestServer> {
  const handler = seshcookie.seshcookie(config);
  const server = http.createServer((req, res) => {
    handler(req, res, () => {
      const sreq = req as seshcookie.SessionRequest;
      const url = req.url ?? '';

      if (url.startsWith('/set/')) {
        sreq.session.name = url.slice('/set/'.length);
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ name: sreq.session.name }));
      } else if (url === '/get') {
        const name = sreq.session.name as string | undefined;
        res.writeHead(name !== undefined ? 200 : 204, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ name }));
      } else if (url === '/clear') {
        sreq.session = {};
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({}));
      } else {
        res.writeHead(404);
        res.end();
      }
    });
  });

  const url = await listen(server);
  return { url, server };
}

describe('bare node:http integration', () => {
  const config = newConfig();
  let ts: TestServer;

  beforeAll(async () => {
    ts = await startBareHttpServer(config);
  });
  afterAll(() => {
    ts.server.close();
  });

  it('roundtrips a session through the cookie', async () => {
    const jar = new Jar();
    const name = crypto.randomBytes(8).toString('hex');

    const { res, setCookie } = await jar.get(`${ts.url}/set/${name}`);
    assert.equal(res.status, 200);
    assert.ok(setCookie, 'expected a Set-Cookie header');
    assert.match(setCookie, new RegExp(`^${config.cookieName}=.`));

    const { res: res2 } = await jar.get(`${ts.url}/get`);
    assert.equal(res2.status, 200);
    const body = (await res2.json()) as { name?: string };
    assert.equal(body.name, name);
  });

  it('clears the cookie for an emptied session', async () => {
    const jar = new Jar();
    const name = crypto.randomBytes(8).toString('hex');

    await jar.get(`${ts.url}/set/${name}`);
    assert.ok(jar.cookie);

    const { res } = await jar.get(`${ts.url}/clear`);
    assert.equal(res.status, 200);
    assert.equal(jar.cookie, '');

    const { res: res2 } = await jar.get(`${ts.url}/get`);
    assert.equal(res2.status, 204);
  });
});
