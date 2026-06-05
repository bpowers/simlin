// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Vendored from https://github.com/bpowers/seshcookie-js at commit
// 46aef15d1bb267dd17a680b5dbf657c12fbddad1 ("make framework-agnostic
// with zero dependencies"), relicensed from ISC to Apache-2.0 with the
// explicit permission of its sole author (Bobby Powers, also the
// Simlin author). simlin is the only consumer of seshcookie, so the
// library lives here instead of going through an npm publish cycle;
// keep diffs against upstream minimal so the two stay easy to
// reconcile.

import * as crypto from 'crypto';
import { IncomingMessage, ServerResponse } from 'http';

// we use AES128 in Galois Counter Mode; with this GCM instantiation
// the size of the nonce is 12 bytes
const algorithm = 'aes-128-gcm';
const gcmNonceSize = 12;

export type SameSite = boolean | 'lax' | 'strict' | 'none';

export interface Options {
  key: string; // key used for encrypting + decrypting sessions
  cookieName: string;
  cookiePath: string;
  httpOnly: boolean;
  secure: boolean;
  maxAgeInSeconds?: number;
  sameSite?: SameSite;
}

export interface SessionData {
  [key: string]: any;
}

// the request object seen by downstream handlers: the standard node
// request extended with the decrypted session.
export interface SessionRequest extends IncomingMessage {
  session: SessionData;
}

export type NextFunction = (err?: unknown) => void;

export type RequestHandler = (
  req: IncomingMessage,
  res: ServerResponse,
  next: NextFunction,
) => void;

// when used with express, downstream handlers see the session on
// express's Request type without needing a cast.
declare global {
  // eslint-disable-next-line @typescript-eslint/no-namespace
  namespace Express {
    interface Request {
      session: SessionData;
    }

    interface SessionData {
      [key: string]: any;
    }
  }
}

export function encrypt(plaintext: Buffer, encKey: Buffer): string {
  const nonce = crypto.randomBytes(gcmNonceSize);
  const cipher = crypto.createCipheriv(algorithm, encKey, nonce);

  cipher.setAAD(nonce);
  let ciphertext = cipher.update(plaintext);
  ciphertext = Buffer.concat([ciphertext, cipher.final()]);

  const tag = cipher.getAuthTag();

  return `${nonce.toString('base64')}-${ciphertext.toString('base64')}-${tag.toString('base64')}`;
}

export function decrypt(content: string, encKey: Buffer): string {
  const parts = content.split('-');
  if (parts.length !== 3) {
    throw new Error(`expected 3 parts, got ${parts.length}`);
  }

  const [encodedNonce, encodedCiphertext, encodedTag] = parts;
  const nonce = Buffer.from(encodedNonce, 'base64');
  const ciphertext = Buffer.from(encodedCiphertext, 'base64');
  const tag = Buffer.from(encodedTag, 'base64');

  const cipher = crypto.createDecipheriv(algorithm, encKey, nonce);

  cipher.setAAD(nonce);
  cipher.setAuthTag(tag);

  let plaintext = cipher.update(ciphertext);
  plaintext = Buffer.concat([plaintext, cipher.final()]);

  return plaintext.toString();
}

// turn a user-provided string into a key of the proper length for our AEAD key
function deriveKey(input: string): Buffer {
  return crypto.createHash('sha256').update(input).digest().slice(0, 16);
}

// returns the value of the named cookie from a Cookie request header,
// or undefined.  values are URL-decoded for compatibility with cookies
// written by express's res.cookie (as previous versions of this
// library did).
export function getCookie(header: string | undefined, name: string): string | undefined {
  if (!header) {
    return undefined;
  }

  for (const part of header.split(';')) {
    const i = part.indexOf('=');
    if (i < 0) {
      continue;
    }
    if (part.slice(0, i).trim() !== name) {
      continue;
    }

    let value = part.slice(i + 1).trim();
    if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
      value = value.slice(1, -1);
    }
    try {
      return decodeURIComponent(value);
    } catch (_err) {
      return undefined;
    }
  }

  return undefined;
}

interface CookieAttributes {
  path: string;
  httpOnly: boolean;
  secure: boolean;
  maxAgeInSeconds?: number;
  expires?: Date;
  sameSite?: SameSite;
}

export function serializeCookie(name: string, value: string, attrs: CookieAttributes): string {
  let cookie = `${name}=${encodeURIComponent(value)}`;

  if (attrs.maxAgeInSeconds !== undefined) {
    cookie += `; Max-Age=${Math.floor(attrs.maxAgeInSeconds)}`;
  }
  if (attrs.expires !== undefined) {
    cookie += `; Expires=${attrs.expires.toUTCString()}`;
  }
  cookie += `; Path=${attrs.path}`;
  if (attrs.httpOnly) {
    cookie += '; HttpOnly';
  }
  if (attrs.secure) {
    cookie += '; Secure';
  }
  if (attrs.sameSite !== undefined && attrs.sameSite !== false) {
    const sameSite = attrs.sameSite === true ? 'strict' : attrs.sameSite;
    cookie += `; SameSite=${sameSite.charAt(0).toUpperCase()}${sameSite.slice(1)}`;
  }

  return cookie;
}

function appendSetCookie(res: ServerResponse, cookie: string): void {
  const prev = res.getHeader('Set-Cookie');
  if (prev === undefined) {
    res.setHeader('Set-Cookie', cookie);
  } else if (Array.isArray(prev)) {
    res.setHeader('Set-Cookie', [...prev, cookie]);
  } else {
    res.setHeader('Set-Cookie', [String(prev), cookie]);
  }
}

class SeshCookie {
  readonly key: Buffer;
  readonly cookieName: string;
  readonly cookiePath: string;
  readonly httpOnly: boolean;
  readonly secure: boolean;
  readonly maxAge?: number;
  readonly sameSite?: SameSite;

  constructor(options: Options) {
    this.key = deriveKey(options.key);
    this.cookieName = options.cookieName;
    this.cookiePath = options.cookiePath ? options.cookiePath : '/';
    this.httpOnly = options.httpOnly;
    this.secure = options.secure;
    this.maxAge = options.maxAgeInSeconds;
    this.sameSite = options.sameSite;
  }

  private setCookie(res: ServerResponse, value: string, expire?: boolean): void {
    const attrs: CookieAttributes = {
      httpOnly: this.httpOnly,
      path: this.cookiePath,
      secure: this.secure,
    };

    if (expire) {
      attrs.expires = new Date(0);
      attrs.maxAgeInSeconds = 0;
    } else if (this.maxAge !== undefined) {
      attrs.maxAgeInSeconds = this.maxAge;
    }

    if (this.sameSite !== undefined) {
      attrs.sameSite = this.sameSite;
    }

    appendSetCookie(res, serializeCookie(this.cookieName, value, attrs));
  }

  interceptWriteHeaders(res: ServerResponse, callback: () => void): void {
    const realWriteHead = res.writeHead.bind(res);

    res.writeHead = ((...args: Parameters<ServerResponse['writeHead']>) => {
      // set our encrypted cookie, if necessary
      callback();

      return realWriteHead(...args);
    }) as ServerResponse['writeHead'];
  }

  handle = (req: IncomingMessage, res: ServerResponse, next: NextFunction): void => {
    const sessionReq = req as SessionRequest;
    if (sessionReq.session !== undefined) {
      throw new Error('WARNING: session not empty; check your middleware stack.');
    }

    let hadCookie = false;
    let originalSerializedSession: undefined | string;

    const cookie = getCookie(req.headers.cookie, this.cookieName);
    if (cookie) {
      hadCookie = true;

      try {
        const plaintext = decrypt(cookie, this.key);
        originalSerializedSession = plaintext;
        sessionReq.session = JSON.parse(plaintext) as SessionData;
      } catch (_err) {
        // ignore: a cookie we can't decrypt is treated as no session
      }
    }

    if (sessionReq.session === undefined) {
      sessionReq.session = {};
    }

    this.interceptWriteHeaders(res, () => {
      if (Object.keys(sessionReq.session).length === 0) {
        if (hadCookie) {
          // session has been emptied out; need to delete cookie.
          this.setCookie(res, '', true);
        }

        return;
      }

      const contents = JSON.stringify(sessionReq.session);
      if (contents === originalSerializedSession) {
        // session hasn't changed; don't re-set the cookie
        return;
      }

      const ciphertext = encrypt(Buffer.from(contents), this.key);
      this.setCookie(res, ciphertext);
    });

    next();
  };
}

export function seshcookie(options: Options): RequestHandler {
  const seshCookie = new SeshCookie(options);
  return seshCookie.handle;
}
