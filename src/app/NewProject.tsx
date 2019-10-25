// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import Button from '@material-ui/core/Button';
import Checkbox from '@material-ui/core/Checkbox';
import ExpansionPanel from '@material-ui/core/ExpansionPanel';
import ExpansionPanelDetails from '@material-ui/core/ExpansionPanelDetails';
import ExpansionPanelSummary from '@material-ui/core/ExpansionPanelSummary';
import Grid from '@material-ui/core/Grid';
import InputAdornment from '@material-ui/core/InputAdornment';
import TextField from '@material-ui/core/TextField';
import Typography from '@material-ui/core/Typography';
import ExpandMoreIcon from '@material-ui/icons/ExpandMore';

import { stdProject } from '../engine/project';

import { Project } from './Project';
import { User } from './User';

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
  projectJSON?: any;
  isPublic?: boolean;
}

const readFile = (file: any): Promise<string> => {
  const reader = new FileReader();

  return new Promise((resolve, reject) => {
    reader.onerror = err => {
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
  class extends React.Component<NewProjectProps, NewProjectState> {
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
      if (this.state.projectJSON) {
        bodyContents.projectJSON = this.state.projectJSON;
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
        const errorMsg = body && body.error ? body.error : `HTTP ${status}; maybe try a different username ¯\\_(ツ)_/¯`;
        this.setState({
          errorMsg,
        });
        return;
      }

      const project = await response.json();
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

    handleExpandClick = (): void => {
      this.setState(state => ({ expanded: !state.expanded }));
    };

    handlePublicChecked = (): void => {
      this.setState(state => ({ isPublic: !state.isPublic }));
    };

    uploadModel = async (event: React.ChangeEvent<HTMLInputElement>) => {
      if (!event.target || !event.target.files || event.target.files.length <= 0) {
        console.log('expected non-empty list of files?');
        return;
      }
      const contents = await readFile(event.target.files[0]);
      let doc: XMLDocument;
      try {
        doc = new DOMParser().parseFromString(contents, 'application/xml');
      } catch (e) {
        this.setState({
          errorMsg: `DOMParser: ${e}`,
        });
        return;
      }

      const [project, err] = stdProject.addXmileFile(doc, true);
      if (err) {
        console.log(err);
        this.setState({
          errorMsg: `error parsing model: ${err.message}`,
        });
        return;
      }
      if (!project) {
        this.setState({
          errorMsg: `unknown file creation error`,
        });
        return;
      }

      const file = project.toFile();
      // ensure we've converted to plain-old JavaScript objects
      const projectJSON = JSON.parse(JSON.stringify(file));

      this.setState({ projectJSON });
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
              startAdornment: <InputAdornment position="start">{this.props.user.username}/</InputAdornment>,
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

          <ExpansionPanel>
            <ExpansionPanelSummary expandIcon={<ExpandMoreIcon />}>
              <div>
                <Typography>Advanced</Typography>
              </div>
            </ExpansionPanelSummary>
            <ExpansionPanelDetails>
              <form>
                <Grid container spacing={10} justify="center" alignItems="center">
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
                        accept=".stmx,.itmx,.xmile"
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
            </ExpansionPanelDetails>
          </ExpansionPanel>

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
