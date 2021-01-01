// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { fromUint8Array } from 'js-base64';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import Accordion from '@material-ui/core/Accordion';
import AccordionDetails from '@material-ui/core/AccordionDetails';
import AccordionSummary from '@material-ui/core/AccordionSummary';
import Button from '@material-ui/core/Button';
import Checkbox from '@material-ui/core/Checkbox';
import Grid from '@material-ui/core/Grid';
import InputAdornment from '@material-ui/core/InputAdornment';
import TextField from '@material-ui/core/TextField';
import Typography from '@material-ui/core/Typography';
import ExpandMoreIcon from '@material-ui/icons/ExpandMore';

import { Project } from './Project';
import { User } from './User';
import { Project as ProjectDM } from './datamodel';
import { convertMdlToXmile } from '../xmutil-js';
import { fromXmile } from '../importer';

const styles = createStyles({
  newSubtitle: {
    marginTop: 6,
  },
});

interface NewProjectProps extends WithStyles<typeof styles> {
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

const readFile = (file: any): Promise<string> => {
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

export const NewProject = withStyles(styles)(
  class NewProject extends React.Component<NewProjectProps, NewProjectState> {
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

      // convert vensim files to xmile
      if (file.name.endsWith('.mdl')) {
        contents = await convertMdlToXmile(contents, false);
      }

      try {
        const projectPB: Uint8Array = await fromXmile(contents);
        const activeProject = ProjectDM.deserializeBinary(projectPB);
        const views = activeProject.models.get('main')?.views;
        if (!views || views.isEmpty()) {
          this.setState({
            errorMsg: `can't import model with no view at this time.`,
          });
          return;
        }

        this.setState({ projectPB });
      } catch (e) {
        this.setState({
          errorMsg: `importer: ${e}`,
        });
        return;
      }
    };

    render() {
      const { classes } = this.props;
      const warningText = this.state.errorMsg || '';
      return (
        <div>
          <Typography variant="h2">Create a project</Typography>
          <div className={classes.newSubtitle}>
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
                        onChange={this.uploadModel}
                      />
                    </Button>
                  </Grid>
                  <Grid item xs={12}>
                    <Typography>
                      <Checkbox checked={this.state.isPublic} onChange={this.handlePublicChecked} />
                      Publically accessible
                    </Typography>
                  </Grid>
                </Grid>
              </form>
            </AccordionDetails>
          </Accordion>

          <br />
          <br />
          <br />
          <Typography variant="subtitle2">
            <b>&nbsp;{warningText}</b>
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
);
