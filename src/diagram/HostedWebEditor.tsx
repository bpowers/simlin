// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { fromUint8Array, toUint8Array } from 'js-base64';
import { History } from 'history';

import { baseURL, defined } from '@system-dynamics/core/common';

import { Editor } from './Editor';

class HostedWebEditorError implements Error {
  name = 'HostedWebEditorError';
  message: string;
  constructor(msg: string) {
    this.message = msg;
  }
}

interface HostedWebEditorProps {
  username: string;
  projectName: string;
  embedded?: boolean;
  baseURL?: string;
  history?: History;
}

// eslint-disable-next-line @typescript-eslint/no-empty-interface
interface HostedWebEditorState {
  serviceErrors: List<Error>;
  projectBinary: Readonly<Uint8Array> | undefined;
  projectVersion: number;
}

export class HostedWebEditor extends React.PureComponent<HostedWebEditorProps, HostedWebEditorState> {
  constructor(props: HostedWebEditorProps) {
    super(props);

    this.state = {
      serviceErrors: List<Error>(),
      projectBinary: undefined,
      projectVersion: -1,
    };

    // eslint-disable-next-line @typescript-eslint/no-misused-promises
    setTimeout(async () => {
      await this.loadProject();
    });
  }

  private appendModelError(msg: string) {
    this.setState({
      serviceErrors: this.state.serviceErrors.push(new HostedWebEditorError(msg)),
    });
  }

  private getBaseURL(): string {
    return this.props.baseURL !== undefined ? this.props.baseURL : baseURL;
  }

  private handleSave = async (project: Readonly<Uint8Array>, currVersion: number): Promise<number | undefined> => {
    const bodyContents = {
      currVersion,
      projectPB: fromUint8Array(project as Uint8Array),
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
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const body = await response.json();
      const errorMsg =
        body && body.error ? (body.error as string) : `HTTP ${status}; maybe try a different username ¯\\_(ツ)_/¯`;
      this.appendModelError(errorMsg);
      return undefined;
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const projectResponse = await response.json();
    const projectVersion = defined(projectResponse.version) as number;

    this.setState({ projectVersion });

    return projectVersion;
  };

  private async loadProject(): Promise<void> {
    const base = this.getBaseURL();
    const apiPath = `${base}/api/projects/${this.props.username}/${this.props.projectName}`;
    const response = await fetch(apiPath);
    if (response.status >= 400) {
      this.appendModelError(`unable to load ${apiPath}`);
      return;
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const projectResponse = await response.json();

    const projectBinary = toUint8Array(projectResponse.pb);

    this.setState({
      projectBinary,
      projectVersion: defined(projectResponse.version) as number,
    });
  }

  render(): React.ReactNode {
    if (!this.state.projectBinary || !this.state.projectVersion) {
      if (!this.state.serviceErrors.isEmpty()) {
        // TODO: render this more nicely
        return <div>{defined(this.state.serviceErrors.first()).message}</div>;
      } else {
        return <div />;
      }
    }

    return (
      <Editor
        initialProjectBinary={this.state.projectBinary}
        initialProjectVersion={this.state.projectVersion}
        embedded={this.props.embedded}
        onSave={this.handleSave}
      />
    );
  }
}
