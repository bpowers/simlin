// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { fetchProjects } from './api';
import type { ProjectMeta } from './api';
import { EditorHost } from './components/EditorHost';
import { EmptyState } from './components/EmptyState';
import { ProjectList } from './components/ProjectList';
import { UpdatesSocket } from './ws';
import type { ChangeSource, WsMessage } from './ws';

const GIT_HINT_DISMISSED_KEY = 'simlin-serve-git-hint-dismissed';

// Reads the dismissed flag from sessionStorage without throwing: some browsers
// (notably in private/incognito mode) throw on any sessionStorage access rather
// than returning null.
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
  // Provenance of the most recent live-version advance per path. Tracked
  // alongside liveVersions so the EditorHost can surface a "this model
  // was updated on disk" toast for `disk` source events without showing
  // anything for the user's own saves echoed back over the WS.
  liveSources: Readonly<Record<string, ChangeSource>>;
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
    liveSources: {},
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
    this.socket = new UpdatesSocket(this.handleLiveMessage);
  }

  private handleLiveMessage = (msg: WsMessage): void => {
    if (msg.type === 'projectChanged') {
      this.setState((prev) => {
        const previous = prev.liveVersions[msg.path];
        // Drop only when we have already observed a version greater than
        // or equal to this one. A first-time event for an unseen path
        // always lands so the create/discover flow (which emits
        // version 0 for fresh registry entries) updates live state in
        // every receiving tab. Without this guard the gate compared
        // `msg.version <= 0` against an `undefined ?? 0` default and
        // dropped every first-time event at version 0.
        if (previous !== undefined && msg.version <= previous) {
          return null;
        }
        return {
          liveVersions: { ...prev.liveVersions, [msg.path]: msg.version },
          liveSources: { ...prev.liveSources, [msg.path]: msg.source },
        };
      });
      // When a projectChanged carries a path the sidebar does not yet
      // know about (cross-tab CreateModel, MCP create, or a new file
      // the watcher just discovered), refresh the projects list so the
      // entry appears without a manual reload. Reading `this.state`
      // gives the most recent committed projects; setState calls earlier
      // in this handler are queued, but for an unseen path the projects
      // entry would not be there in either pre- or post-commit state.
      const known = this.state.projects?.some((p) => p.path === msg.path) ?? false;
      if (!known) {
        void this.loadProjects();
      }
      return;
    }
    if (msg.type === 'projectRemoved') {
      this.setState((prev) => {
        const projects = prev.projects;
        if (projects === null) {
          return null;
        }
        const filtered = projects.filter((p) => p.path !== msg.path);
        // Drop the path from liveVersions/liveSources so a future
        // re-creation under the same path starts from a clean slate
        // (fresh registry entries begin at version 0 with no recorded
        // source).
        const nextLiveVersions = { ...prev.liveVersions };
        delete nextLiveVersions[msg.path];
        const nextLiveSources = { ...prev.liveSources };
        delete nextLiveSources[msg.path];
        // Phase 4 lays the wiring for delete; Phase 8 polishes the
        // "this model was deleted on disk" message. Falling back to
        // selectedPath = null is the sane default — render either the
        // empty state (when no projects remain) or no editor selection.
        const nextSelected = prev.selectedPath === msg.path ? null : prev.selectedPath;
        return {
          projects: filtered,
          selectedPath: nextSelected,
          liveVersions: nextLiveVersions,
          liveSources: nextLiveSources,
        };
      });
      return;
    }
    if (msg.type === 'projectRenamed') {
      this.setState((prev) => {
        const projects = prev.projects;
        if (projects === null) {
          return null;
        }
        // Replace the entry whose path matches `from` with one keyed on
        // `to`. The doc / version / hash are unchanged server-side, so
        // we carry the cached liveVersion forward under the new key —
        // EditorHost's refetch gate then sees `liveVersion === serverVersion`
        // and stays mounted on the same payload. Clearing `liveVersions[from]`
        // avoids leaking stale entries if the path is later re-used.
        const swapped = projects.map((p) => (p.path === msg.from ? { ...p, path: msg.to } : p));
        const carriedVersion = prev.liveVersions[msg.from];
        const carriedSource = prev.liveSources[msg.from];
        const nextLiveVersions = { ...prev.liveVersions };
        delete nextLiveVersions[msg.from];
        if (carriedVersion !== undefined) {
          nextLiveVersions[msg.to] = carriedVersion;
        }
        const nextLiveSources = { ...prev.liveSources };
        delete nextLiveSources[msg.from];
        if (carriedSource !== undefined) {
          nextLiveSources[msg.to] = carriedSource;
        }
        const nextSelected = prev.selectedPath === msg.from ? msg.to : prev.selectedPath;
        return {
          projects: swapped,
          selectedPath: nextSelected,
          liveVersions: nextLiveVersions,
          liveSources: nextLiveSources,
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

  // The NewProjectButton calls this with the freshly-created file's
  // relative path. Selecting the new path opens the editor on it; the
  // server-side `ProjectChanged` broadcast already refreshed the list
  // for any other open tabs, but the originating tab also benefits
  // from a refetch here so the sidebar shows the new entry immediately
  // even before the WS event arrives.
  private handleProjectCreated = (newPath: string): void => {
    this.setState({ selectedPath: newPath });
    void this.loadProjects();
  };

  private handleDismissGitHint = (): void => {
    if (typeof sessionStorage !== 'undefined') {
      try {
        sessionStorage.setItem(GIT_HINT_DISMISSED_KEY, '1');
      } catch {
        // Some browsers (notably Safari in private mode) throw on any
        // sessionStorage access. Swallow the error so the in-memory
        // dismissal still lands; the hint reappears in fresh tabs but
        // that matches the documented "session" semantics. Mirrors the
        // try/catch pattern in readDismissedFlag.
      }
    }
    this.setState({ gitHintDismissed: true });
  };

  render(): React.ReactNode {
    const { projects, gitAvailable, selectedPath, gitHintDismissed, loadError, liveVersions, liveSources } = this.state;

    const showHint = !gitAvailable && !gitHintDismissed;
    const ready = projects !== null;
    const empty = ready && projects.length === 0;
    // The EditorHost only ever displays one project at a time, so we
    // hand it the live version for the currently-selected path. A
    // missing entry maps to 0 (no live update yet), which the host's
    // refetch gate treats as "no advance".
    const liveVersion = selectedPath !== null ? (liveVersions[selectedPath] ?? 0) : 0;
    const liveSource = selectedPath !== null ? liveSources[selectedPath] : undefined;

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
          <EmptyState onCreated={this.handleProjectCreated} />
        ) : (
          <div className="serve-layout">
            <ProjectList
              projects={projects}
              selectedPath={selectedPath}
              onSelect={this.handleSelect}
              onCreated={this.handleProjectCreated}
            />
            <EditorHost
              path={selectedPath}
              liveVersion={liveVersion}
              liveSource={liveSource}
              onPathRedirect={this.handlePathRedirect}
              socket={this.socket ?? undefined}
            />
          </div>
        )}
      </div>
    );
  }
}
