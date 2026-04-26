// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Editor } from '@simlin/diagram';

import { fetchProject, saveProject, ValidationError, VersionConflictError } from '../api';
import type { GetProjectResponse, JsonProjectData, ServerValidationError } from '../api';

type EditorHostProps = Readonly<{
  path: string | null;
  // Latest server-announced version observed via the WebSocket. App
  // updates this whenever a `ProjectChanged` arrives for `path`. When
  // it advances past the version EditorHost currently holds in state,
  // EditorHost refetches and remounts the Editor with the new payload
  // (Phase 3 Task 11). The default of 0 is "no live version observed
  // yet"; the gate in componentDidUpdate compares strictly greater.
  liveVersion?: number;
  // Invoked when a `.mdl` save creates a sidecar so the parent can update
  // its selectedPath state and refresh the project list. Optional because
  // not every host needs to track the redirect (e.g. tests that only
  // verify the wire format).
  //
  // Server-side counterpart: handlers.rs redirect_to_sidecar call, which
  // moves the registry entry from the .mdl key to the new sidecar key.
  onPathRedirect?: (newPath: string) => void;
  // Invoked when a save returns 409 Conflict and EditorHost has refetched
  // the latest server state. Lets the parent reset any external editor
  // state to the new authoritative payload. When omitted, EditorHost
  // re-renders the Editor itself with the refetched payload (the Editor's
  // `key` prop forces a remount so its internal state matches the new
  // initial JSON).
  onConflict?: (latestJson: string, latestVersion: number) => void;
}>;

type EditorHostState = {
  loadedPath: string | null;
  payload: GetProjectResponse | null;
  error: string | null;
  pending: boolean;
  // Bumped each time we replace the payload from the server (initial GET
  // or post-409 refetch). Used as part of the Editor's `key` so the
  // component remounts with fresh initial state when the server
  // authoritative copy diverges from what the Editor was tracking.
  // Successful saves do NOT bump this — the Editor tracks its own
  // version via `state.projectVersion`.
  loadGeneration: number;
  // Latest server version this host is aware of for the loaded path.
  // Distinct from `payload.version` because successful saves bump it
  // without bumping `payload` (the Editor keeps editing in-place
  // without remount). The WebSocket-driven refetch gate compares
  // `props.liveVersion` against this: if a `ProjectChanged` event
  // for a version we already know about (e.g., the echo of our own
  // save) arrives, we skip the refetch.
  serverVersion: number;
};

const INITIAL_STATE: EditorHostState = {
  loadedPath: null,
  payload: null,
  error: null,
  pending: false,
  loadGeneration: 0,
  serverVersion: 0,
};

// Format the server's per-error validation details into a single
// human-readable message for the Editor's toast surface. Each error
// becomes a bullet line of "<code>: <variable>: <message>" so the user
// can see all newly-introduced errors at once. Phase 2 does not try to
// highlight the offending variables in-canvas; that's a later polish.
function formatValidationErrors(errors: ReadonlyArray<ServerValidationError>): string {
  if (errors.length === 0) {
    return 'Save failed: validation rejected the edit but the server returned no details.';
  }
  const lines = errors.map((e) => {
    const variable = e.variableName ?? '(unknown)';
    return ` - ${e.code}: ${variable}: ${e.message}`;
  });
  return `Save failed:\n${lines.join('\n')}`;
}

export class EditorHost extends React.Component<EditorHostProps, EditorHostState> {
  // Track the in-flight request so that switching paths quickly doesn't paint
  // a stale model after the slow fetch finally resolves.
  private currentLoadKey: number = 0;

  state: EditorHostState = INITIAL_STATE;

  componentDidMount(): void {
    if (this.props.path) {
      void this.loadProject(this.props.path);
    }
  }

  componentDidUpdate(prev: EditorHostProps): void {
    if (prev.path !== this.props.path) {
      if (!this.props.path) {
        this.currentLoadKey += 1;
        this.setState(INITIAL_STATE);
        return;
      }
      void this.loadProject(this.props.path);
      return;
    }
    // Path unchanged: check whether the WS-driven liveVersion advanced
    // past our last-known server version. The strict `>` is what
    // prevents refetch loops on our own save's echo: a successful save
    // bumps `state.serverVersion` to the new server value, then the WS
    // delivers `liveVersion` equal to that same value, which fails the
    // gate. The Editor keeps editing in-place; no remount is needed.
    //
    // We bump `serverVersion` synchronously when scheduling the refetch
    // so the intermediate `setState({pending: true})` inside
    // `loadProject` doesn't re-enter this branch and re-issue the
    // request before the GET resolves.
    const path = this.props.path;
    const liveVersion = this.props.liveVersion ?? 0;
    if (path && liveVersion > this.state.serverVersion) {
      this.setState({ serverVersion: liveVersion });
      void this.loadProject(path);
    }
  }

  private async loadProject(path: string): Promise<void> {
    this.currentLoadKey += 1;
    const loadKey = this.currentLoadKey;

    this.setState({ pending: true, error: null });

    try {
      const payload = await fetchProject(path);
      if (loadKey !== this.currentLoadKey) {
        return;
      }
      this.setState((prev) => ({
        loadedPath: path,
        payload,
        error: null,
        pending: false,
        loadGeneration: prev.loadGeneration + 1,
        serverVersion: payload.version,
      }));
    } catch (err) {
      if (loadKey !== this.currentLoadKey) {
        return;
      }
      const message = err instanceof Error ? err.message : 'failed to load project';
      this.setState({ payload: null, error: message, pending: false });
    }
  }

  private handleSave = async (project: JsonProjectData, currVersion: number): Promise<number | undefined> => {
    // Defensive: the Editor only feeds us the protobuf format when
    // `inputFormat="protobuf"`, but the union allows it. We always use
    // JSON in serve so a mismatch indicates a bug we'd rather skip
    // silently than POST garbage.
    if (project.format !== 'json') {
      return undefined;
    }
    const path = this.props.path;
    if (!path) {
      return undefined;
    }
    try {
      const result = await saveProject(path, project.data, currVersion);
      if (result.path !== path) {
        this.props.onPathRedirect?.(result.path);
      }
      // Track the post-save server version so the WS echo of our own
      // save (which arrives with the same version) does not trigger a
      // refetch. `setState` here only matters for the WS gate; the
      // Editor itself owns its `projectVersion` via `result.version`.
      this.setState({ serverVersion: result.version });
      return result.version;
    } catch (err) {
      if (err instanceof VersionConflictError) {
        await this.handleVersionConflict(path);
        // Surface a friendly toast via the Editor's onSave error path.
        // Phase 3 will replace this round-trip with proper Loro merging.
        throw new Error(
          'Your edit conflicted with another save. The latest version has been loaded — please re-apply your changes.',
        );
      }
      if (err instanceof ValidationError) {
        throw new Error(formatValidationErrors(err.errors));
      }
      throw err;
    }
  };

  // Refetch the project after a 409 and surface the new authoritative
  // state. If the parent supplied `onConflict`, hand the latest payload
  // off to it; otherwise update local state and let the Editor's `key`
  // remount with the new initial JSON + version. Refetch failures
  // propagate to the caller so the user sees the underlying error
  // instead of a stale conflict toast.
  private async handleVersionConflict(path: string): Promise<void> {
    const latest = await fetchProject(path);
    if (this.props.onConflict) {
      this.props.onConflict(latest.json, latest.version);
      return;
    }
    this.setState((prev) => ({
      loadedPath: path,
      payload: latest,
      error: null,
      pending: false,
      loadGeneration: prev.loadGeneration + 1,
      serverVersion: latest.version,
    }));
  }

  render(): React.ReactNode {
    const { path } = this.props;
    const { payload, error, loadedPath, loadGeneration } = this.state;

    if (!path) {
      return null;
    }

    if (error) {
      return (
        <div className="serve-editor-host" role="alert">
          <p>{`failed to load ${path}: ${error}`}</p>
        </div>
      );
    }

    if (!payload || loadedPath !== path) {
      return <div className="serve-editor-host serve-editor-host--loading">Loading {path}…</div>;
    }

    const showMdlBanner = payload.source_format === 'mdl';

    return (
      <div className="serve-editor-host">
        {showMdlBanner ? (
          <div className="serve-mdl-banner" role="note">
            Vensim MDL — saves will be written to a <code>.sd.json</code> sidecar.
          </div>
        ) : null}
        <Editor
          key={`${path}#${loadGeneration}`}
          inputFormat="json"
          initialProjectJson={payload.json}
          initialProjectVersion={payload.version}
          name={path}
          onSave={this.handleSave}
        />
      </div>
    );
  }
}
