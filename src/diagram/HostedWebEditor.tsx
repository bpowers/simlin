// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { fromUint8Array, toUint8Array } from 'js-base64';

import { baseURL, defined } from '@system-dynamics/core/common';
import { first } from '@system-dynamics/core/collections';

import { Editor, ProtobufProjectData } from './Editor';

import styles from './HostedWebEditor.module.css';

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
  readOnlyMode?: boolean;
}

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

    setTimeout(async () => {
      await this.loadProject();
    });
  }

  appendModelError(msg: string) {
    this.setState({
      serviceErrors: this.state.serviceErrors.push(new HostedWebEditorError(msg)),
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

  async loadProject(): Promise<void> {
    const base = this.getBaseURL();
    const apiPath = `${base}/api/projects/${this.props.username}/${this.props.projectName}`;
    const response = await fetch(apiPath);
    if (response.status >= 400) {
      this.appendModelError(`unable to load ${apiPath}`);
      return;
    }

    const projectResponse = await response.json();

    const projectBinary = toUint8Array(projectResponse.pb);

    this.setState({
      projectBinary,
      projectVersion: defined(projectResponse.version) as number,
    });
  }

  render(): React.ReactNode {
    const { embedded } = this.props;

    if (!this.state.projectBinary || !this.state.projectVersion) {
      if (!this.state.serviceErrors.isEmpty()) {
        return <div>{first(this.state.serviceErrors).message}</div>;
      } else {
        return <div />;
      }
    }

    const classNames = embedded ? undefined : styles.bg;

    return (
      <div className={classNames}>
        <Editor
          inputFormat="protobuf"
          initialProjectBinary={this.state.projectBinary}
          initialProjectVersion={this.state.projectVersion}
          name={this.props.projectName}
          embedded={this.props.embedded}
          onSave={this.handleSave}
          readOnlyMode={this.props.readOnlyMode}
        />
      </div>
    );
  }
}
