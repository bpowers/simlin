// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Wire types mirroring the simlin-serve Rust handlers; the Imperative Shell
// layer that talks HTTP. Pure helpers (path encoding) live in api-utils.ts so
// tests can exercise them without mocking fetch.

import { readLaunchToken } from './launch-token';

export type ProjectFormat = 'stmx' | 'xmile' | 'mdl' | 'sd_json';

export type GitState = { kind: 'tracked'; dirty: boolean } | { kind: 'untracked' } | { kind: 'unavailable' };

export type ProjectMeta = {
  path: string;
  format: ProjectFormat;
  mtime: string;
  size: number;
  git: GitState;
  version: number;
};

export type ListProjectsResponse = {
  projects: Array<ProjectMeta>;
  git_available: boolean;
};

export type GetProjectResponse = {
  json: string;
  version: number;
  source_format: ProjectFormat;
};

// Mirror of the Editor's `JsonProjectData` shape so EditorHost can type its
// no-op onSave handler without re-importing from @simlin/diagram (which would
// make the test mock leak into the Imperative Shell).
export type JsonProjectData = {
  format: 'json';
  data: string;
};

/**
 * Encode a forward-slashed relative path into a URL path while preserving the
 * separator. The server returns paths with `/` even on Windows clients
 * (`path_to_forward_slash` in handlers.rs), so the SPA can rely on that
 * invariant when constructing URLs.
 */
export function encodeProjectPath(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/');
}

// Build the headers object for a /api/* request. Returns an empty record when
// no launch token is stored so callers can still pass the result to fetch
// without a conditional.
function buildAuthHeaders(): Record<string, string> {
  const token = readLaunchToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

export async function fetchProjects(): Promise<ListProjectsResponse> {
  const response = await fetch('/api/projects', { headers: buildAuthHeaders() });
  if (!response.ok) {
    throw new Error(`failed to fetch projects: HTTP ${response.status}`);
  }
  return (await response.json()) as ListProjectsResponse;
}

export async function fetchProject(path: string): Promise<GetProjectResponse> {
  const response = await fetch(`/api/projects/${encodeProjectPath(path)}`, {
    headers: buildAuthHeaders(),
  });
  if (!response.ok) {
    let message = `failed to fetch ${path}: HTTP ${response.status}`;
    try {
      const body = (await response.json()) as { error?: string };
      if (body && body.error) {
        message = body.error;
      }
    } catch {
      // The body wasn't JSON; fall through to the generic message above.
    }
    throw new Error(message);
  }
  return (await response.json()) as GetProjectResponse;
}
