// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as jose from 'jose';

export interface TokenResponse {
  access_token: string;
  id_token?: string;
  refresh_token?: string;
  expires_in: number;
  token_type: string;
}

export interface GoogleUserInfo {
  sub: string;
  email: string;
  email_verified: boolean;
  name: string;
  picture?: string;
}

export interface AppleIdTokenClaims {
  sub: string;
  email?: string;
  email_verified?: boolean;
  name?: string;
}

export async function exchangeGoogleCode(
  clientId: string,
  clientSecret: string,
  code: string,
  redirectUri: string,
): Promise<TokenResponse> {
  const response = await fetch('https://oauth2.googleapis.com/token', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
    },
    body: new URLSearchParams({
      client_id: clientId,
      client_secret: clientSecret,
      code,
      redirect_uri: redirectUri,
      grant_type: 'authorization_code',
    }).toString(),
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Google token exchange failed: ${error}`);
  }

  return (await response.json()) as TokenResponse;
}

export async function fetchGoogleUserInfo(accessToken: string): Promise<GoogleUserInfo> {
  const response = await fetch('https://www.googleapis.com/oauth2/v3/userinfo', {
    headers: {
      Authorization: `Bearer ${accessToken}`,
    },
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Failed to fetch Google user info: ${error}`);
  }

  return (await response.json()) as GoogleUserInfo;
}

export function generateAppleClientSecret(
  teamId: string,
  clientId: string,
  keyId: string,
  privateKey: string,
): string {
  const now = Math.floor(Date.now() / 1000);
  const expiresIn = 15777000; // ~6 months

  const header = {
    alg: 'ES256',
    kid: keyId,
  };

  const payload = {
    iss: teamId,
    iat: now,
    exp: now + expiresIn,
    aud: 'https://appleid.apple.com',
    sub: clientId,
  };

  const privateKeyObj = require('crypto').createPrivateKey(privateKey);
  const sign = require('crypto').createSign('SHA256');

  const headerB64 = Buffer.from(JSON.stringify(header)).toString('base64url');
  const payloadB64 = Buffer.from(JSON.stringify(payload)).toString('base64url');
  const signingInput = `${headerB64}.${payloadB64}`;

  sign.update(signingInput);
  const signature = sign.sign(privateKeyObj);

  const r = signature.slice(0, 32);
  const s = signature.slice(32, 64);
  const signatureB64 = Buffer.concat([r, s]).toString('base64url');

  return `${signingInput}.${signatureB64}`;
}

export async function exchangeAppleCode(
  clientId: string,
  clientSecret: string,
  code: string,
  redirectUri: string,
): Promise<TokenResponse> {
  const response = await fetch('https://appleid.apple.com/auth/token', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded',
    },
    body: new URLSearchParams({
      client_id: clientId,
      client_secret: clientSecret,
      code,
      redirect_uri: redirectUri,
      grant_type: 'authorization_code',
    }).toString(),
  });

  if (!response.ok) {
    const error = await response.text();
    throw new Error(`Apple token exchange failed: ${error}`);
  }

  return (await response.json()) as TokenResponse;
}

let cachedJwks: jose.JSONWebKeySet | undefined;
let jwksCacheTime = 0;
const JWKS_CACHE_TTL_MS = 60 * 60 * 1000; // 1 hour

async function fetchAppleJwks(): Promise<jose.JSONWebKeySet> {
  const now = Date.now();
  if (cachedJwks && now - jwksCacheTime < JWKS_CACHE_TTL_MS) {
    return cachedJwks;
  }

  const response = await fetch('https://appleid.apple.com/auth/keys');
  if (!response.ok) {
    throw new Error('Failed to fetch Apple JWKS');
  }

  cachedJwks = (await response.json()) as jose.JSONWebKeySet;
  jwksCacheTime = now;
  return cachedJwks;
}

export async function verifyAppleIdToken(
  idToken: string,
  options: { clientId: string },
): Promise<AppleIdTokenClaims> {
  const jwks = await fetchAppleJwks();
  const JWKS = jose.createLocalJWKSet(jwks);

  const { payload } = await jose.jwtVerify(idToken, JWKS, {
    issuer: 'https://appleid.apple.com',
    audience: options.clientId,
  });

  return {
    sub: payload.sub as string,
    email: payload.email as string | undefined,
    email_verified: payload.email_verified as boolean | undefined,
    name: (payload as { name?: string }).name,
  };
}

export function clearJwksCache(): void {
  cachedJwks = undefined;
  jwksCacheTime = 0;
}
