// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Functional core for HostedWebEditor's project HTTP operations: load, save, and
// delete. Each function is framework-free (no React) and calls the global `fetch`
// directly. Tests stub `globalThis.fetch` rather than injecting it: the native
// `fetch` throws "Illegal invocation" when called as a method of any object other
// than the global (`endpoint.fetch(...)` rebinds `this` to `endpoint`), so the
// dependency-injection variant worked under test mocks but broke in real browsers.
// The component shell in HostedWebEditor.tsx maps these results onto React state
// and the window navigation.

import { fromUint8Array, toUint8Array } from '@simlin/core/base64';

import { ProtobufProjectData } from './Editor';

// Extends the built-in Error so instances carry a stack trace and satisfy
// `instanceof Error`. The explicit name assignment survives minification.
export class HostedWebEditorError extends Error {
  constructor(msg: string) {
    super(msg);
    this.name = 'HostedWebEditorError';
  }
}

export interface ProjectEndpoint {
  base: string;
  username: string;
  projectName: string;
}

// The result of loadProject(). loadProject never rejects: a network error, a
// non-JSON body, or a response missing pb/version is reported as an `error`
// result so the caller can surface it without an unhandled rejection (which used
// to leave the editor permanently blank).
export type LoadResult =
  | { kind: 'loaded'; projectBinary: Readonly<Uint8Array>; projectVersion: number }
  | { kind: 'error'; message: string };

export function projectApiPath(endpoint: ProjectEndpoint): string {
  return `${endpoint.base}/api/projects/${endpoint.username}/${endpoint.projectName}`;
}

export async function loadProject(endpoint: ProjectEndpoint): Promise<LoadResult> {
  const apiPath = projectApiPath(endpoint);
  try {
    const response = await fetch(apiPath);
    if (response.status >= 400) {
      return { kind: 'error', message: `unable to load ${apiPath}` };
    }

    const projectResponse = (await response.json()) as { pb?: unknown; version?: unknown };
    if (typeof projectResponse?.pb !== 'string' || typeof projectResponse?.version !== 'number') {
      return { kind: 'error', message: `malformed project response from ${apiPath}` };
    }

    const projectBinary = toUint8Array(projectResponse.pb);
    return { kind: 'loaded', projectBinary, projectVersion: projectResponse.version };
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    return { kind: 'error', message: `unable to load ${apiPath}: ${msg}` };
  }
}

// Failure classification for saveProject(), so the shell can offer the right
// recovery: `conflict` (409 -- the server's currVersion check failed because the
// project changed in another tab/session), `unauthorized` (401/403 -- an
// expired/cleared session; see src/server/authz.ts), and `other` (everything
// else: 5xx, malformed bodies, network failures) which the next edit retries.
export type SaveErrorReason = 'conflict' | 'unauthorized' | 'other';

// The result of saveProject(). Like loadProject, saveProject never rejects: the
// save queue treats a resolved-undefined as "failed, retry with the next edit",
// but a rejection escaped before the failure was recorded anywhere (issue #928)
// -- a non-JSON error body (proxy HTML page, empty 502) made the unguarded
// response.json() throw out of the non-2xx branch, and a malformed 2xx body
// threw out of the success branch.
export type SaveResult =
  | { kind: 'saved'; version: number }
  | { kind: 'error'; reason: SaveErrorReason; message: string };

function classifySaveStatus(status: number): SaveErrorReason {
  if (status === 409) {
    return 'conflict';
  }
  if (status === 401 || status === 403) {
    return 'unauthorized';
  }
  return 'other';
}

export async function saveProject(
  endpoint: ProjectEndpoint,
  project: ProtobufProjectData,
  currVersion: number,
): Promise<SaveResult> {
  const apiPath = projectApiPath(endpoint);
  try {
    const bodyContents = {
      currVersion,
      projectPB: fromUint8Array(project.data as Uint8Array),
    };

    const response = await fetch(apiPath, {
      credentials: 'same-origin',
      method: 'POST',
      cache: 'no-cache',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(bodyContents),
    });

    const status = response.status;
    if (!(status >= 200 && status < 400)) {
      // The error body is best-effort: a proxy or load balancer can answer with
      // HTML or an empty body, so a JSON parse failure keeps the status-bearing
      // fallback instead of escaping as a rejection.
      let message = `HTTP ${status} while saving`;
      try {
        const body = (await response.json()) as { error?: unknown };
        if (typeof body?.error === 'string' && body.error !== '') {
          message = body.error;
        }
      } catch {
        // keep the status-bearing fallback
      }
      return { kind: 'error', reason: classifySaveStatus(status), message };
    }

    const projectResponse = (await response.json()) as { version?: unknown };
    if (typeof projectResponse?.version !== 'number') {
      return { kind: 'error', reason: 'other', message: `malformed save response from ${apiPath}` };
    }
    return { kind: 'saved', version: projectResponse.version };
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    return { kind: 'error', reason: 'other', message: `unable to save ${apiPath}: ${msg}` };
  }
}

// Issues the DELETE and returns the URL the caller should navigate to on
// success. On any non-2xx/3xx response it throws with the server-provided message
// (or a status-bearing fallback) so the in-editor confirmation dialog can surface
// it and stay open for a retry.
export async function deleteProject(endpoint: ProjectEndpoint): Promise<string> {
  const apiPath = projectApiPath(endpoint);
  const response = await fetch(apiPath, {
    credentials: 'same-origin',
    method: 'DELETE',
    cache: 'no-cache',
  });

  const status = response.status;
  if (!(status >= 200 && status < 400)) {
    let errorMsg = `HTTP ${status} while deleting project`;
    try {
      const body = await response.json();
      if (body && typeof body.error === 'string') {
        errorMsg = body.error as string;
      }
    } catch {
      // keep the status-bearing fallback
    }
    throw new Error(errorMsg);
  }

  // The project list lives at the base root; navigating there refetches without
  // the just-deleted project.
  return `${endpoint.base}/`;
}

// Full-page navigation to `url`. This is the one DOM side effect of the delete
// flow; it lives here (rather than inline in the shell) as a named, mockable
// seam so a test can observe the post-delete navigation without driving a real
// page transition (jsdom's window.location.assign is non-configurable and cannot
// be spied directly). The shell calls it through the module namespace so a spy
// installed on this export is observed.
export function redirectToHome(url: string): void {
  window.location.assign(url);
}

// Full-page reload, the recovery for a version conflict: it discards the
// in-memory edits and picks up the server's newer version. A seam for the same
// reason as redirectToHome (jsdom's window.location.reload is unimplemented and
// non-spyable); the shell calls it through the module namespace.
export function reloadPage(): void {
  window.location.reload();
}

// Opens the app's sign-in page (the app root renders Login when the session is
// dead) in a NEW tab: a same-tab navigation would unload the editor and destroy
// the unsaved in-memory work the user is trying to rescue. After signing in
// there, this tab's same-origin session cookie is fresh again and the next
// edit's autosave succeeds.
export function openSignInPage(url: string): void {
  window.open(url, '_blank', 'noopener');
}
