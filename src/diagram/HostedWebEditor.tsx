// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { fromUint8Array, toUint8Array } from 'js-base64';

import { baseURL, defined } from '@simlin/core/common';
import { first } from '@simlin/core/collections';

import { Editor, ProtobufProjectData } from './Editor';
import { ErrorBoundary } from './ErrorBoundary';

import styles from './HostedWebEditor.module.css';

// Extends the built-in Error so instances carry a stack trace and satisfy
// `instanceof Error`. The explicit name assignment survives minification.
class HostedWebEditorError extends Error {
  constructor(msg: string) {
    super(msg);
    this.name = 'HostedWebEditorError';
  }
}

interface HostedWebEditorProps {
  username: string;
  projectName: string;
  embedded?: boolean;
  baseURL?: string;
  readOnlyMode?: boolean;
}

interface HostedWebEditorState {
  serviceErrors: readonly Error[];
  projectBinary: Readonly<Uint8Array> | undefined;
  projectVersion: number;
}

export class HostedWebEditor extends React.PureComponent<HostedWebEditorProps, HostedWebEditorState> {
  // Pending setTimeout(0) handle for the deferred loadProject(); held so
  // componentWillUnmount can cancel it. `unmounted` short-circuits a callback
  // that already drained off the macrotask queue (clearTimeout no longer
  // reaches it). Both reset in componentDidMount -- see the comment there.
  private loadProjectTimer: ReturnType<typeof setTimeout> | null = null;
  private unmounted = false;

  constructor(props: HostedWebEditorProps) {
    super(props);

    this.state = {
      serviceErrors: [],
      projectBinary: undefined,
      projectVersion: -1,
    };
    // The deferred loadProject() is kicked off in componentDidMount, not here
    // -- see the comment there. Keep this constructor side-effect free.
  }

  componentDidMount() {
    // React 18 StrictMode (dev) drives every committed component through
    // componentDidMount -> componentWillUnmount -> componentDidMount on the
    // *same* instance without re-running the constructor, and double-invokes
    // the render phase so a second instance is created and discarded. So:
    //  - `unmounted` is (re)set false here, not (only) in the constructor, so
    //    the second StrictMode mount doesn't leave it stuck true.
    //  - loadProject() is scheduled here, not in the constructor, so the
    //    StrictMode unmount/remount is schedule -> cancel -> schedule (one
    //    fetch, not two) and a discarded render-phase instance -- which never
    //    reaches componentDidMount -- never fires loadProject() onto a zombie
    //    `this` and setState()s on an instance React never committed.
    this.unmounted = false;
    this.loadProjectTimer = setTimeout(async () => {
      this.loadProjectTimer = null;
      if (this.unmounted) {
        return;
      }
      await this.loadProject();
    });
  }

  componentWillUnmount() {
    // Flag first, then cancel: a callback already drained off the macrotask
    // queue at unmount time must short-circuit on `unmounted` (clearTimeout
    // below is best-effort for the still-pending case).
    this.unmounted = true;
    if (this.loadProjectTimer !== null) {
      clearTimeout(this.loadProjectTimer);
      this.loadProjectTimer = null;
    }
  }

  appendModelError(msg: string) {
    this.setState({
      serviceErrors: [...this.state.serviceErrors, new HostedWebEditorError(msg)],
    });
  }

  getBaseURL(): string {
    return this.props.baseURL ?? baseURL;
  }

  handleSave = async (project: ProtobufProjectData, currVersion: number): Promise<number | undefined> => {
    if (this.props.readOnlyMode) return;

    const bodyContents = {
      currVersion,
      projectPB: fromUint8Array(project.data as Uint8Array),
    };

    const base = this.getBaseURL();
    const apiPath = `${base}/api/projects/${this.props.username}/${this.props.projectName}`;
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
      const body = await response.json();
      const errorMsg =
        body && body.error ? (body.error as string) : `HTTP ${status}; maybe try a different username ¯\\_(ツ)_/¯`;
      this.appendModelError(errorMsg);
      return undefined;
    }

    const projectResponse = await response.json();
    const projectVersion = defined(projectResponse.version) as number;

    this.setState({ projectVersion });

    return projectVersion;
  };

  handleDelete = async (): Promise<void> => {
    if (this.props.readOnlyMode) return;

    const base = this.getBaseURL();
    const apiPath = `${base}/api/projects/${this.props.username}/${this.props.projectName}`;
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
      // Surface this to the in-editor confirmation dialog (which stays open
      // for a retry) rather than appendModelError(): once a project loads,
      // serviceErrors are no longer rendered.
      throw new Error(errorMsg);
    }

    // Full navigation back to the project list so it refetches without the
    // just-deleted project.
    this.redirectToHome(`${base}/`);
  };

  // Extracted so tests can observe the post-delete navigation without
  // assigning to jsdom's non-writable window.location.
  redirectToHome(url: string): void {
    window.location.assign(url);
  }

  // Must never reject: the componentDidMount caller is fire-and-forget, so a
  // thrown error would become an unhandled rejection and leave the editor
  // permanently blank (render() shows nothing until projectBinary is set and
  // only serviceErrors produce a message).
  async loadProject(): Promise<void> {
    const base = this.getBaseURL();
    const apiPath = `${base}/api/projects/${this.props.username}/${this.props.projectName}`;
    try {
      const response = await fetch(apiPath);
      if (response.status >= 400) {
        this.appendModelError(`unable to load ${apiPath}`);
        return;
      }

      const projectResponse = (await response.json()) as { pb?: unknown; version?: unknown };
      if (typeof projectResponse?.pb !== 'string' || typeof projectResponse?.version !== 'number') {
        this.appendModelError(`malformed project response from ${apiPath}`);
        return;
      }

      const projectBinary = toUint8Array(projectResponse.pb);

      this.setState({
        projectBinary,
        projectVersion: projectResponse.version,
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      this.appendModelError(`unable to load ${apiPath}: ${msg}`);
    }
  }

  render(): React.ReactNode {
    const { embedded } = this.props;

    if (!this.state.projectBinary || !this.state.projectVersion) {
      if (this.state.serviceErrors.length > 0) {
        return <div>{first(this.state.serviceErrors).message}</div>;
      } else {
        return <div />;
      }
    }

    const classNames = embedded ? undefined : styles.bg;

    return (
      <div className={classNames}>
        <ErrorBoundary>
          <Editor
            inputFormat="protobuf"
            initialProjectBinary={this.state.projectBinary}
            initialProjectVersion={this.state.projectVersion}
            name={this.props.projectName}
            embedded={this.props.embedded}
            onSave={this.handleSave}
            onDeleteProject={this.props.readOnlyMode ? undefined : this.handleDelete}
            readOnlyMode={this.props.readOnlyMode}
          />
        </ErrorBoundary>
      </div>
    );
  }
}
