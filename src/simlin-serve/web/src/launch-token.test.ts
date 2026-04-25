// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { TOKEN_STORAGE_KEY, captureLaunchToken } from './launch-token';

describe('captureLaunchToken', () => {
  beforeEach(() => {
    sessionStorage.clear();
    window.history.replaceState(null, '', '/');
  });

  test('parses ?token=... from the URL into sessionStorage', () => {
    window.history.replaceState(null, '', '/?token=abc123');
    captureLaunchToken();
    expect(sessionStorage.getItem(TOKEN_STORAGE_KEY)).toBe('abc123');
  });

  test('rewrites the URL to remove the token query parameter', () => {
    window.history.replaceState(null, '', '/?token=topsecret&other=keep');
    captureLaunchToken();
    expect(window.location.search).not.toContain('token=');
    // Other params should be preserved on the rewritten URL.
    expect(window.location.search).toContain('other=keep');
  });

  test('is a no-op when no token is present', () => {
    window.history.replaceState(null, '', '/');
    captureLaunchToken();
    expect(sessionStorage.getItem(TOKEN_STORAGE_KEY)).toBeNull();
  });

  test('does not overwrite an existing token when none is in the URL', () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'previously-stored');
    window.history.replaceState(null, '', '/');
    captureLaunchToken();
    expect(sessionStorage.getItem(TOKEN_STORAGE_KEY)).toBe('previously-stored');
  });

  test('the new URL token wins when both URL and storage have one', () => {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, 'old');
    window.history.replaceState(null, '', '/?token=new');
    captureLaunchToken();
    expect(sessionStorage.getItem(TOKEN_STORAGE_KEY)).toBe('new');
  });
});
