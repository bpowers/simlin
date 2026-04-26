// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { fetchProjects } from './api';
import type { ProjectMeta } from './api';
import { EditorHost } from './components/EditorHost';
import { EmptyState } from './components/EmptyState';
import { ProjectList } from './components/ProjectList';
import { readLaunchToken } from './launch-token';
import { UpdatesSocket } from './ws';
import type { WsMessage } from './ws';

const GIT_HINT_DISMISSED_KEY = 'simlin-serve-git-hint-dismissed';

// Reads the dismissed flag from sessionStorage without throwing: some browsers
// (notably in private/incognito mode) throw on any sessionStorage access rather
// than returning null. The pattern mirrors readLaunchToken in launch-token.ts.
function readDismissedFlag(): boolean {
  if (typeof sessionStorage === 'undefined') {
    return false;
  }
  try {
    return sessionStorage.getItem(GIT_HINT_DISMISSED_KEY) === '1';
  } catch {
    return false;
  }
}

type AppState = {
  projects: ReadonlyArray<ProjectMeta> | null;
  gitAvailable: boolean;
  selectedPath: string | null;
  gitHintDismissed: boolean;
  loadError: string | null;
  // Latest server-announced version per project path. Updated whenever
  // the WebSocket reports a `ProjectChanged`. EditorHost compares this
  // against the version of the JSON it currently holds and refetches
  // when the live version advances. Saves originated by this tab also
  // increment server-side and echo back via the WS, so the
  // version-equality check (`liveVersion <= state.version`) prevents
  // a refetch loop when the echoed version equals what we already have.
  liveVersions: Readonly<Record<string, number>>;
};

export class App extends React.Component<Record<string, never>, AppState> {
  state: AppState = {
    projects: null,
    gitAvailable: true,
    selectedPath: null,
    // Persist across reloads of the same browser tab so the AC2.5 hint stays
    // dismissed for the duration of the session, but reappears in fresh tabs
    // (matches the design's "one-time hint" wording).
    gitHintDismissed: readDismissedFlag(),
    loadError: null,
    liveVersions: {},
  };

  // Held so componentWillUnmount can dispose the connection. Not in
  // state because the socket is a side-effecting handle, not data the
  // render function consumes.
  private socket: UpdatesSocket | null = null;

  componentDidMount(): void {
    void this.loadProjects();
    this.openLiveUpdates();
  }

  componentWillUnmount(): void {
    if (this.socket) {
      this.socket.close();
      this.socket = null;
    }
  }

  private openLiveUpdates(): void {
    const token = readLaunchToken();
    if (!token) {
      // The legacy/manual flow without a launch token cannot upgrade
      // (the server enforces the token on /api/updates). Skip rather
      // than spin a reconnect loop that will keep getting 401s.
      return;
    }
    this.socket = new UpdatesSocket(token, this.handleLiveMessage);
  }

  private handleLiveMessage = (msg: WsMessage): void => {
    if (msg.type === 'projectChanged') {
      this.setState((prev) => {
        const previous = prev.liveVersions[msg.path] ?? 0;
        // Versions are monotonically increasing per path; if a stale
        // event arrives (e.g. due to broadcast ordering races), keep the
        // higher value so the EditorHost refetch gate doesn't oscillate.
        if (msg.version <= previous) {
          return null;
        }
        return {
          liveVersions: { ...prev.liveVersions, [msg.path]: msg.version },
        };
      });
      return;
    }
    if (msg.type === 'projectRemoved') {
      this.setState((prev) => {
        const projects = prev.projects;
        if (projects === null) {
          return null;
        }
        const filtered = projects.filter((p) => p.path !== msg.path);
        // Drop the path from liveVersions so a future re-creation under
        // the same path starts from a clean slate (fresh registry entries
        // begin at version 0).
        const nextLiveVersions = { ...prev.liveVersions };
        delete nextLiveVersions[msg.path];
        // Phase 4 lays the wiring for delete; Phase 8 polishes the
        // "this model was deleted on disk" message. Falling back to
        // selectedPath = null is the sane default — render either the
        // empty state (when no projects remain) or no editor selection.
        const nextSelected = prev.selectedPath === msg.path ? null : prev.selectedPath;
        return {
          projects: filtered,
          selectedPath: nextSelected,
          liveVersions: nextLiveVersions,
        };
      });
    }
  };

  private async loadProjects(): Promise<void> {
    try {
      const response = await fetchProjects();
      this.setState({
        projects: response.projects,
        gitAvailable: response.git_available,
        loadError: null,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'failed to load projects';
      this.setState({
        projects: [],
        loadError: message,
      });
    }
  }

  private handleSelect = (path: string): void => {
    this.setState({ selectedPath: path });
  };

  // After a .mdl save creates a sibling .sd.json sidecar the registry
  // swaps the .mdl entry for the sidecar entry. Update the active
  // selection to the new path and re-fetch the project list so the
  // sidebar reflects the rename without a full reload.
  private handlePathRedirect = (newPath: string): void => {
    this.setState({ selectedPath: newPath });
    void this.loadProjects();
  };

  private handleDismissGitHint = (): void => {
    if (typeof sessionStorage !== 'undefined') {
      sessionStorage.setItem(GIT_HINT_DISMISSED_KEY, '1');
    }
    this.setState({ gitHintDismissed: true });
  };

  render(): React.ReactNode {
    const { projects, gitAvailable, selectedPath, gitHintDismissed, loadError, liveVersions } = this.state;

    const showHint = !gitAvailable && !gitHintDismissed;
    const ready = projects !== null;
    const empty = ready && projects.length === 0;
    // The EditorHost only ever displays one project at a time, so we
    // hand it the live version for the currently-selected path. A
    // missing entry maps to 0 (no live update yet), which the host's
    // refetch gate treats as "no advance".
    const liveVersion = selectedPath !== null ? (liveVersions[selectedPath] ?? 0) : 0;

    return (
      <div className="serve-app">
        {showHint ? (
          <div role="banner" className="serve-git-hint">
            <span>git not on PATH — version-control state will not be shown.</span>
            <button type="button" onClick={this.handleDismissGitHint} aria-label="Dismiss">
              Dismiss
            </button>
          </div>
        ) : null}
        {loadError ? (
          <div role="alert" className="serve-load-error">
            {loadError}
          </div>
        ) : null}
        {!ready ? (
          <div className="serve-loading">Loading projects…</div>
        ) : empty ? (
          <EmptyState />
        ) : (
          <div className="serve-layout">
            <ProjectList projects={projects} selectedPath={selectedPath} onSelect={this.handleSelect} />
            <EditorHost
              path={selectedPath}
              liveVersion={liveVersion}
              onPathRedirect={this.handlePathRedirect}
            />
          </div>
        )}
      </div>
    );
  }
}
