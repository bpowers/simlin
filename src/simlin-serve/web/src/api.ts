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
// onSave handler without re-importing from @simlin/diagram (which would make
// the test mock leak into the Imperative Shell).
export type JsonProjectData = {
  format: 'json';
  data: string;
};

export type SaveResponse = {
  version: number;
  path: string;
};

// `format` is the subset of ProjectFormat the server's create endpoint
// accepts: stmx (canonical native XMILE) or sd_json (canonical AI/SD-AI
// format). The server rejects mdl and xmile for new files.
export type CreateProjectFormat = 'stmx' | 'sd_json';

export type CreateProjectResponse = {
  path: string;
  version: number;
};

// Mirror of `simlin-serve::handlers::ValidationError` (camelCase via serde).
export type ServerValidationError = {
  code: string;
  message: string;
  modelName?: string;
  variableName?: string;
  kind: string;
};

/**
 * Thrown when a save POST returns 409 Conflict because the client's
 * `version` is stale relative to the server's current version. The
 * `actualVersion` field carries the current server version so the caller
 * can refetch and present the up-to-date state.
 */
export class VersionConflictError extends Error {
  readonly actualVersion: number;

  constructor(actualVersion: number) {
    super(`version conflict: server has version ${actualVersion}`);
    this.name = 'VersionConflictError';
    this.actualVersion = actualVersion;
  }
}

/**
 * Thrown when a save POST returns 422 Unprocessable Entity because the
 * incoming JSON would introduce new validation errors not present in the
 * pre-edit baseline. `errors` is the list of *new* errors only.
 */
export class ValidationError extends Error {
  readonly errors: ReadonlyArray<ServerValidationError>;

  constructor(errors: ReadonlyArray<ServerValidationError>) {
    super(`validation failed: ${errors.length} error(s)`);
    this.name = 'ValidationError';
    this.errors = errors;
  }
}

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

/**
 * POST a saved model state to the server. The server holds the registry
 * write lock across the optimistic version check + increment, so two
 * concurrent saves with the same `version` cannot both win; the loser
 * receives a 409 and must refetch.
 *
 * @throws VersionConflictError on 409 (stale version).
 * @throws ValidationError on 422 (new validation errors introduced).
 * @throws Error on other non-OK responses (carrying the HTTP status).
 */
export async function saveProject(path: string, json: string, version: number): Promise<SaveResponse> {
  const response = await fetch(`/api/projects/${encodeProjectPath(path)}`, {
    method: 'POST',
    headers: {
      ...buildAuthHeaders(),
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ json, version }),
  });

  if (response.status === 409) {
    const body = (await response.json().catch(() => ({}))) as { actual?: number };
    const actualVersion = typeof body.actual === 'number' ? body.actual : version;
    throw new VersionConflictError(actualVersion);
  }
  if (response.status === 422) {
    const body = (await response.json().catch(() => ({}))) as {
      details?: ReadonlyArray<ServerValidationError>;
    };
    throw new ValidationError(body.details ?? []);
  }
  if (!response.ok) {
    let message = `failed to save ${path}: HTTP ${response.status}`;
    try {
      const body = (await response.json()) as { error?: string };
      if (body && body.error) {
        message = `${body.error} (HTTP ${response.status})`;
      }
    } catch {
      // The body wasn't JSON; fall through to the generic message above.
    }
    throw new Error(message);
  }

  return (await response.json()) as SaveResponse;
}

/**
 * Create a new empty model file via `POST /api/projects/new`. The server
 * builds a minimal datamodel::Project with one empty `main` model and
 * the canonical default sim-specs.
 *
 * `name` is the bare filename (no extension); the server appends the
 * extension implied by `format`. `parentDir` is an optional
 * forward-slash relative path under the scan root.
 *
 * @throws Error on any non-OK response (carrying the server message
 *  when one is provided).
 */
export async function createProject(
  name: string,
  format: CreateProjectFormat,
  parentDir?: string,
): Promise<CreateProjectResponse> {
  // Use a discriminated body so the server-side serde definition (with
  // `parent_dir: Option<String>`) sees an absent field rather than a
  // null when the caller doesn't pass parentDir; this mirrors the
  // skip_serializing_if behavior on Rust's side.
  const body: Record<string, unknown> = { name, format };
  if (parentDir !== undefined) {
    body.parent_dir = parentDir;
  }
  const response = await fetch('/api/projects/new', {
    method: 'POST',
    headers: {
      ...buildAuthHeaders(),
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });

  if (!response.ok) {
    let message = `failed to create project: HTTP ${response.status}`;
    try {
      const errBody = (await response.json()) as { error?: string };
      if (errBody && errBody.error) {
        message = `${errBody.error} (HTTP ${response.status})`;
      }
    } catch {
      // The body wasn't JSON; fall through to the generic message above.
    }
    throw new Error(message);
  }

  return (await response.json()) as CreateProjectResponse;
}
