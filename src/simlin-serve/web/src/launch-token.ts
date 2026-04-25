// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// One-time launch-token capture: the simlin-serve binary opens
// `http://127.0.0.1:<port>/?token=...` in the user's browser, the SPA reads
// the token from the URL, stashes it in sessionStorage so subsequent navigations
// inside the tab keep it, and rewrites the URL so the token doesn't end up in
// browser history or shoulder-surf-visible. Phase 1 stores the token; the
// server only enforces the bearer in Phase 3 (along with the WebSocket).

export const TOKEN_STORAGE_KEY = 'simlin-serve-token';

const TOKEN_QUERY_PARAM = 'token';

export function captureLaunchToken(): void {
  if (typeof window === 'undefined') {
    return;
  }
  const url = new URL(window.location.href);
  const token = url.searchParams.get(TOKEN_QUERY_PARAM);
  if (!token) {
    return;
  }
  try {
    sessionStorage.setItem(TOKEN_STORAGE_KEY, token);
  } catch {
    // Some browsers throw on sessionStorage access in private mode; the SPA
    // can still function (no auth on the server in Phase 1) so we swallow it.
  }
  url.searchParams.delete(TOKEN_QUERY_PARAM);
  // Preserve any remaining query parameters but drop the token from the
  // visible URL so it doesn't leak through window.location or browser history.
  const rewritten = url.pathname + (url.search ? url.search : '') + url.hash;
  window.history.replaceState(null, '', rewritten);
}

export function readLaunchToken(): string | null {
  if (typeof sessionStorage === 'undefined') {
    return null;
  }
  try {
    return sessionStorage.getItem(TOKEN_STORAGE_KEY);
  } catch {
    return null;
  }
}
