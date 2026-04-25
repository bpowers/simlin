// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Editor } from '@simlin/diagram';

import { fetchProject } from '../api';
import type { GetProjectResponse, JsonProjectData } from '../api';

type EditorHostProps = Readonly<{
  path: string | null;
}>;

type EditorHostState = {
  loadedPath: string | null;
  payload: GetProjectResponse | null;
  error: string | null;
  pending: boolean;
};

const INITIAL_STATE: EditorHostState = {
  loadedPath: null,
  payload: null,
  error: null,
  pending: false,
};

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
    if (prev.path === this.props.path) {
      return;
    }
    if (!this.props.path) {
      this.currentLoadKey += 1;
      this.setState(INITIAL_STATE);
      return;
    }
    void this.loadProject(this.props.path);
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
      this.setState({ loadedPath: path, payload, error: null, pending: false });
    } catch (err) {
      if (loadKey !== this.currentLoadKey) {
        return;
      }
      const message = err instanceof Error ? err.message : 'failed to load project';
      this.setState({ payload: null, error: message, pending: false });
    }
  }

  // Phase 1 is read-only. The Editor's `onSave` prop is required by the
  // TypeScript types, so we satisfy it with a no-op that returns `undefined`
  // (signaling "no new version assigned"). Phase 2 wires the real save.
  private handleSave = async (_project: JsonProjectData, _currVersion: number): Promise<number | undefined> => {
    return undefined;
  };

  render(): React.ReactNode {
    const { path } = this.props;
    const { payload, error, loadedPath } = this.state;

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
          inputFormat="json"
          initialProjectJson={payload.json}
          initialProjectVersion={payload.version}
          name={path}
          embedded={true}
          readOnlyMode={true}
          onSave={this.handleSave}
        />
      </div>
    );
  }
}
