// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { fetchProjects } from './api';
import type { ProjectMeta } from './api';
import { EditorHost } from './components/EditorHost';
import { EmptyState } from './components/EmptyState';
import { ProjectList } from './components/ProjectList';

const GIT_HINT_DISMISSED_KEY = 'simlin-serve-git-hint-dismissed';

type AppState = {
  projects: ReadonlyArray<ProjectMeta> | null;
  gitAvailable: boolean;
  selectedPath: string | null;
  gitHintDismissed: boolean;
  loadError: string | null;
};

export class App extends React.Component<Record<string, never>, AppState> {
  state: AppState = {
    projects: null,
    gitAvailable: true,
    selectedPath: null,
    // Persist across reloads of the same browser tab so the AC2.5 hint stays
    // dismissed for the duration of the session, but reappears in fresh tabs
    // (matches the design's "one-time hint" wording).
    gitHintDismissed: typeof sessionStorage !== 'undefined' && sessionStorage.getItem(GIT_HINT_DISMISSED_KEY) === '1',
    loadError: null,
  };

  componentDidMount(): void {
    void this.loadProjects();
  }

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

  private handleDismissGitHint = (): void => {
    if (typeof sessionStorage !== 'undefined') {
      sessionStorage.setItem(GIT_HINT_DISMISSED_KEY, '1');
    }
    this.setState({ gitHintDismissed: true });
  };

  render(): React.ReactNode {
    const { projects, gitAvailable, selectedPath, gitHintDismissed, loadError } = this.state;

    const showHint = !gitAvailable && !gitHintDismissed;
    const ready = projects !== null;
    const empty = ready && projects.length === 0;

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
            <EditorHost path={selectedPath} />
          </div>
        )}
      </div>
    );
  }
}
