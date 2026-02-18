// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { fromUint8Array } from 'js-base64';

import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Button,
  Checkbox,
  FormControlLabel,
  InputAdornment,
  TextField,
  ExpandMoreIcon,
} from '@simlin/diagram';

import { Project } from './Project';
import { User } from './User';
import { projectFromJson } from '@simlin/core/datamodel';
import { Project as EngineProject } from '@simlin/engine';
import type { JsonProject } from '@simlin/engine';

import styles from './NewProject.module.css';
import typography from './typography.module.css';

interface NewProjectProps {
  user: User;
  onProjectCreated: (project: Project) => void;
}

interface NewProjectState {
  projectNameField: string;
  descriptionField: string;
  errorMsg?: string;
  expanded: boolean;
  projectPB?: Uint8Array;
  isPublic?: boolean;
}

const readFile = (file: Blob): Promise<string> => {
  const reader = new FileReader();

  return new Promise((resolve, reject) => {
    reader.onerror = (err) => {
      reader.abort();
      reject(new DOMException(`Problem parsing input file: ${err.type}`));
    };

    reader.onload = () => {
      resolve(reader.result as string);
    };
    reader.readAsText(file);
  });
};

export class NewProject extends React.Component<NewProjectProps, NewProjectState> {
  state: NewProjectState;

  constructor(props: NewProjectProps) {
    super(props);
    this.state = {
      projectNameField: '',
      descriptionField: '',
      expanded: false,
    };
  }

  handleProjectNameChanged = (event: React.ChangeEvent<HTMLInputElement>): void => {
    this.setState({
      projectNameField: event.target.value,
    });
  };

  handleDescriptionChanged = (event: React.ChangeEvent<HTMLInputElement>): void => {
    this.setState({
      descriptionField: event.target.value,
    });
  };

  setProjectName = async (): Promise<void> => {
    const bodyContents: any = {
      projectName: this.state.projectNameField,
      description: this.state.descriptionField,
    };
    if (this.state.projectPB) {
      bodyContents.projectPB = fromUint8Array(this.state.projectPB);
    }
    if (this.state.isPublic) {
      bodyContents.isPublic = true;
    }
    const response = await fetch('/api/projects', {
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
      this.setState({
        errorMsg,
      });
      return;
    }

    const project = (await response.json()) as Project;
    this.props.onProjectCreated(project);
  };

  handleKeyPress = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      this.handleClose();
    }
  };

  handleClose = (): void => {
    if (this.state.projectNameField === '') {
      this.setState({
        errorMsg: 'Please give your project a non-empty name',
      });
    } else {
      setTimeout(this.setProjectName);
    }
  };

  handlePublicChecked = (checked: boolean): void => {
    this.setState({ isPublic: checked });
  };

  uploadModel = async (event: React.ChangeEvent<HTMLInputElement>) => {
    if (!event.target || !event.target.files || event.target.files.length <= 0) {
      console.log('expected non-empty list of files?');
      return;
    }
    const file = event.target.files[0];
    const contents = await readFile(file);
    try {
      let engineProject: EngineProject;

      if (file.name.endsWith('.mdl')) {
        engineProject = await EngineProject.openVensim(contents);
      } else {
        engineProject = await EngineProject.open(contents);
      }

      const projectPB = await engineProject.serializeProtobuf();
      const json = JSON.parse(await engineProject.serializeJson()) as JsonProject;
      const activeProject = projectFromJson(json);
      const views = activeProject.models.get('main')?.views;
      if (!views || views.length === 0) {
        this.setState({
          errorMsg: `can't import model with no view at this time.`,
        });
        return;
      }

      this.setState({
        projectPB,
        errorMsg: undefined,
      });
    } catch (e) {
      this.setState({
        errorMsg: `${e}`,
      });
      return;
    }
  };

  render() {
    const warningText = this.state.errorMsg || '';
    return (
      <div>
        <h2 className={typography.heading2}>Create a project</h2>
        <div className={styles.subtitle}>
          <p className={typography.subtitle1}>A project holds models and data, along with simulation results.</p>
        </div>
        <br />
        <TextField
          onChange={this.handleProjectNameChanged}
          autoFocus
          id="projectName"
          label="Project Name"
          type="text"
          error={this.state.errorMsg !== undefined}
          onKeyPress={this.handleKeyPress}
          fullWidth
          InputProps={{
            startAdornment: <InputAdornment position="start">{this.props.user.id}/</InputAdornment>,
          }}
        />
        <br />
        <br />
        <TextField
          onChange={this.handleDescriptionChanged}
          id="description"
          label="Project Description"
          type="text"
          onKeyPress={this.handleKeyPress}
          fullWidth
        />

        <br />
        <br />

        <Accordion>
          <AccordionSummary expandIcon={<ExpandMoreIcon />}>
            <div>
              <span>Advanced</span>
            </div>
          </AccordionSummary>
          <AccordionDetails>
            <form>
              <div className={styles.advancedGrid}>
                <div className={styles.gridCol8}>
                  <span>Use existing model</span>
                </div>
                <div className={styles.gridCol4}>
                  <Button variant="contained" className="NewProject-upload-button" color="secondary" component="label">
                    Select
                    <input
                      style={{ display: 'none' }}
                      accept=".stmx,.itmx,.xmile,.mdl"
                      id="xmile-model-file"
                      type="file"
                      onChange={this.uploadModel}
                    />
                  </Button>
                </div>
                <div className={styles.gridCol12}>
                  <FormControlLabel
                    control={<Checkbox checked={this.state.isPublic} onChange={this.handlePublicChecked} />}
                    label="Publicly accessible"
                  />
                </div>
              </div>
            </form>
          </AccordionDetails>
        </Accordion>

        <br />
        <br />
        <br />
        <p className={typography.subtitle2} style={{ whiteSpace: 'pre-wrap' }}>
          <b>{warningText || '\xa0'}</b>
        </p>
        <p className={typography.textRight}>
          <Button onClick={this.handleClose} color="primary">
            Create
          </Button>
        </p>
      </div>
    );
  }
}
