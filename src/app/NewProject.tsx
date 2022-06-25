// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { fromUint8Array } from 'js-base64';
import { styled } from '@mui/material/styles';
import Accordion from '@mui/material/Accordion';
import AccordionDetails from '@mui/material/AccordionDetails';
import AccordionSummary from '@mui/material/AccordionSummary';
import Button from '@mui/material/Button';
import Checkbox from '@mui/material/Checkbox';
import Grid from '@mui/material/Grid';
import InputAdornment from '@mui/material/InputAdornment';
import TextField from '@mui/material/TextField';
import Typography from '@mui/material/Typography';
import ExpandMoreIcon from '@mui/icons-material/ExpandMore';

import { Project } from './Project';
import { User } from './User';
import { Project as ProjectDM } from '@system-dynamics/core/datamodel';
import { convertMdlToXmile } from '@system-dynamics/xmutil';
import { fromXmile } from '@system-dynamics/importer';

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
      reject(new DOMException(`Problem parsing input file: ${err}`));
    };

    reader.onload = () => {
      resolve(reader.result as string);
    };
    reader.readAsText(file);
  });
};

export const NewProject = styled(
  class NewProject extends React.Component<NewProjectProps & { className?: string }, NewProjectState> {
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
        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
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
        // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
        const body = await response.json();
        // eslint-disable-next-line @typescript-eslint/no-unsafe-call
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
        // eslint-disable-next-line @typescript-eslint/no-misused-promises
        setTimeout(this.setProjectName);
      }
    };

    handlePublicChecked = (): void => {
      this.setState((state) => ({ isPublic: !state.isPublic }));
    };

    uploadModel = async (event: React.ChangeEvent<HTMLInputElement>) => {
      if (!event.target || !event.target.files || event.target.files.length <= 0) {
        console.log('expected non-empty list of files?');
        return;
      }
      const file = event.target.files[0];
      let contents = await readFile(file);
      let logs: string | undefined;

      try {
        // convert vensim files to xmile
        if (file.name.endsWith('.mdl')) {
          [contents, logs] = await convertMdlToXmile(contents, true);
          if (contents.length === 0) {
              throw new Error('Vensim converter: ' + (logs || 'unknown error'));
          }
        }

        const projectPB: Uint8Array = await fromXmile(contents);
        const activeProject = ProjectDM.deserializeBinary(projectPB);
        const views = activeProject.models.get('main')?.views;
        if (!views || views.isEmpty()) {
          let errorMsg = `can't import model with no view at this time.`;
          if (logs && logs.length !== 0) {
              errorMsg = logs;
          }
          this.setState({
            errorMsg,
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
      const { className } = this.props;
      const warningText = this.state.errorMsg || '';
      return (
        <div className={className}>
          <Typography variant="h2">Create a project</Typography>
          <div className="simlin-newproject-subtitle">
            <Typography variant="subtitle1">A project holds models and data, along with simulation results.</Typography>
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
                <Typography>Advanced</Typography>
              </div>
            </AccordionSummary>
            <AccordionDetails>
              <form>
                <Grid container spacing={10} justifyContent="center" alignItems="center">
                  <Grid item xs={8}>
                    <Typography>Use existing model</Typography>
                  </Grid>
                  <Grid item xs={4}>
                    <Button
                      variant="contained"
                      className="NewProject-upload-button"
                      color="secondary"
                      component="label"
                    >
                      Select
                      <input
                        style={{ display: 'none' }}
                        accept=".stmx,.itmx,.xmile,.mdl"
                        id="xmile-model-file"
                        type="file"
                        /* eslint-disable-next-line @typescript-eslint/no-misused-promises */
                        onChange={this.uploadModel}
                      />
                    </Button>
                  </Grid>
                  <Grid item xs={12}>
                    <Typography>
                      <Checkbox checked={this.state.isPublic} onChange={this.handlePublicChecked} />
                      Publicly accessible
                    </Typography>
                  </Grid>
                </Grid>
              </form>
            </AccordionDetails>
          </Accordion>

          <br />
          <br />
          <br />
          <Typography variant="subtitle2" style={{whiteSpace: "pre-wrap"}}>
            <b>{warningText || '\xa0'}</b>
          </Typography>
          <Typography align="right">
            <Button onClick={this.handleClose} color="primary">
              Create
            </Button>
          </Typography>
        </div>
      );
    }
  },
)(() => ({
  '.simlin-newproject-subtitle': {
    marginTop: 6,
  },
}));
