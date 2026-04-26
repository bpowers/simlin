// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Editor } from '@simlin/diagram';

import { fetchProject, saveProject, ValidationError, VersionConflictError } from '../api';
import type { GetProjectResponse, JsonProjectData, ServerValidationError } from '../api';
import type { ChangeSource, UpdatesSocket } from '../ws';

type EditorHostProps = Readonly<{
  path: string | null;
  // Latest server-announced version observed via the WebSocket. App
  // updates this whenever a `ProjectChanged` arrives for `path`. When
  // it advances past the version EditorHost currently holds in state,
  // EditorHost refetches and remounts the Editor with the new payload
  // (Phase 3 Task 11). The default of 0 is "no live version observed
  // yet"; the gate in componentDidUpdate compares strictly greater.
  liveVersion?: number;
  // Provenance of the most recent live-version advance. When `disk`,
  // EditorHost surfaces a transient toast so the user understands the
  // remount was triggered by an external editor (e.g. a save in vim).
  // Other sources (`user`/`agent`) are silent — the user already knows
  // their own save happened.
  liveSource?: ChangeSource;
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
  // Live-update channel used to push browser intent (project focus,
  // selection) back to the server so MCP clients can be notified. The
  // prop is optional because (a) the App skips the socket entirely
  // when no launch token is present, and (b) component tests that only
  // exercise the fetch/save paths don't need a socket. Phase 7 wires
  // `projectFocused` and `selectionChanged` through this channel.
  socket?: UpdatesSocket;
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
  // True while a transient "this model was updated on disk" notice is
  // visible. Set when a disk-source live advance is observed; cleared
  // either by the auto-dismiss timer or when a different path/source
  // takes over.
  diskNoticeVisible: boolean;
};

const INITIAL_STATE: EditorHostState = {
  loadedPath: null,
  payload: null,
  error: null,
  pending: false,
  loadGeneration: 0,
  serverVersion: 0,
  diskNoticeVisible: false,
};

// How long the disk-update toast lingers before auto-dismissing. Long
// enough to read the message, short enough that it doesn't pile up if
// the user is editing a file with frequent external saves (e.g. a vim
// session that writes on every change).
const DISK_NOTICE_TIMEOUT_MS = 5000;

// Coalesce rapid selection changes into a single WS frame. The Editor
// fires onSelectionChanged on every click/drag during a multi-element
// box-select; bundling them avoids flooding the MCP fan-out with
// transient intermediate states. 150ms keeps point-and-click feel
// responsive while smoothing drags.
const SELECTION_DEBOUNCE_MS = 150;

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
  // Pending auto-dismiss timer for the disk-update toast. Held so we
  // can clear a previous timer when a second disk advance arrives
  // before the first one expires (otherwise the second toast would be
  // dismissed early by the first timer's callback).
  private diskNoticeTimer: ReturnType<typeof setTimeout> | null = null;
  // Pending debounce timer for the next selection-changed frame. We
  // hold the handle so a rapid sequence of selection changes can
  // collapse into one outbound frame: each new event clears the
  // previous timer and schedules a fresh one. Cleared on unmount.
  private selectionDebounceTimer: ReturnType<typeof setTimeout> | null = null;

  state: EditorHostState = INITIAL_STATE;

  componentDidMount(): void {
    if (this.props.path) {
      void this.loadProject(this.props.path);
      this.emitProjectFocused(this.props.path);
    }
  }

  componentWillUnmount(): void {
    if (this.diskNoticeTimer !== null) {
      clearTimeout(this.diskNoticeTimer);
      this.diskNoticeTimer = null;
    }
    if (this.selectionDebounceTimer !== null) {
      clearTimeout(this.selectionDebounceTimer);
      this.selectionDebounceTimer = null;
    }
  }

  componentDidUpdate(prev: EditorHostProps): void {
    if (prev.path !== this.props.path) {
      if (!this.props.path) {
        this.currentLoadKey += 1;
        this.clearDiskNoticeTimer();
        this.setState(INITIAL_STATE);
        return;
      }
      // Switching to a different path drops any in-flight toast — it
      // belonged to the old path and would be confusing in context of
      // the new one.
      this.clearDiskNoticeTimer();
      this.setState({ diskNoticeVisible: false });
      void this.loadProject(this.props.path);
      this.emitProjectFocused(this.props.path);
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
      if (this.props.liveSource === 'disk') {
        this.showDiskNotice();
      }
      void this.loadProject(path);
    }
  }

  // Tell the server which project the browser is currently looking at
  // so the MCP notification fan-out can correlate AI sessions with the
  // user's focus. The `Editor` component itself doesn't know this — the
  // host owns the path-to-project mapping.
  private emitProjectFocused(path: string): void {
    this.props.socket?.send({ type: 'projectFocused', path });
  }

  // Coalesce a burst of selection changes into one WS frame. Each new
  // call replaces any pending frame so only the latest selection set
  // is sent. The path is read from props at flush time to ensure a
  // selection event that happens to arrive across a path swap targets
  // the project the user actually has selected.
  private handleSelectionChanged = (idents: ReadonlyArray<string>): void => {
    if (this.selectionDebounceTimer !== null) {
      clearTimeout(this.selectionDebounceTimer);
    }
    this.selectionDebounceTimer = setTimeout(() => {
      this.selectionDebounceTimer = null;
      const path = this.props.path;
      if (!path) {
        return;
      }
      this.props.socket?.send({
        type: 'selectionChanged',
        path,
        variableIdents: idents,
      });
    }, SELECTION_DEBOUNCE_MS);
  };

  private showDiskNotice(): void {
    this.clearDiskNoticeTimer();
    this.setState({ diskNoticeVisible: true });
    this.diskNoticeTimer = setTimeout(() => {
      this.diskNoticeTimer = null;
      this.setState({ diskNoticeVisible: false });
    }, DISK_NOTICE_TIMEOUT_MS);
  }

  private clearDiskNoticeTimer(): void {
    if (this.diskNoticeTimer !== null) {
      clearTimeout(this.diskNoticeTimer);
      this.diskNoticeTimer = null;
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
    const { payload, error, loadedPath, loadGeneration, diskNoticeVisible } = this.state;

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
        {diskNoticeVisible ? (
          <div className="serve-disk-notice" role="status" aria-live="polite">
            This model was updated on disk.
          </div>
        ) : null}
        <Editor
          key={`${path}#${loadGeneration}`}
          inputFormat="json"
          initialProjectJson={payload.json}
          initialProjectVersion={payload.version}
          name={path}
          onSave={this.handleSave}
          onSelectionChanged={this.handleSelectionChanged}
        />
      </div>
    );
  }
}
