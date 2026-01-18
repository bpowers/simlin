// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'wouter';
import clsx from 'clsx';
import AppBar from '@mui/material/AppBar';
import Button from '@mui/material/Button';
import Grid from '@mui/material/Grid';
import ImageList from '@mui/material/ImageList';
import ImageListItem from '@mui/material/ImageListItem';
import IconButton from '@mui/material/IconButton';
import Menu from '@mui/material/Menu';
import MenuItem from '@mui/material/MenuItem';
import Paper from '@mui/material/Paper';
import Toolbar from '@mui/material/Toolbar';
import Typography from '@mui/material/Typography';
import Avatar from '@mui/material/Avatar';
import { List } from 'immutable';
import { PopoverOrigin } from '@mui/material/Popover';
import AccountCircle from '@mui/icons-material/AccountCircle';
import MenuIcon from '@mui/icons-material/Menu';

import { NewProject } from './NewProject';
import { Project } from './Project';
import { User } from './User';

import styles from './Home.module.css';

interface HomeState {
  anchorEl?: HTMLElement;
  projects: List<Project>;
}

interface HomeProps {
  user: User;
  isNewProject: boolean;
  onNewProjectDone?: () => void;
}

const AnchorOrigin: PopoverOrigin = {
  vertical: 'bottom',
  horizontal: 'right',
};

class Home extends React.Component<HomeProps, HomeState> {
  state: HomeState;

  constructor(props: HomeProps) {
    super(props);

    this.state = {
      anchorEl: undefined,
      projects: List<Project>(),
    };

    setTimeout(this.getProjects);
  }

  getProjects = async (): Promise<void> => {
    const response = await fetch('/api/projects', { credentials: 'same-origin' });
    const status = response.status;
    if (!(status >= 200 && status < 400)) {
      console.log("couldn't fetch projects.");
      return;
    }
    const projects = (await response.json()) as Project[];
    this.setState({
      projects: List(projects),
    });
  };

  handleClose = () => {
    this.setState({
      anchorEl: undefined,
    });
  };

  handleMenu = (event: React.MouseEvent<HTMLElement>) => {
    this.setState({
      anchorEl: event.currentTarget,
    });
  };

  handleProjectCreated = (project: Project) => {
    window.location.pathname = '/' + project.id;
  };

  getGridListCols = () => {
    // TODO: this should be 1 on small screens, but useMediaQuery doesn't
    //       work in class components, only function components.
    return 2;
  };

  newProjectForm() {
    return (
      <div className={styles.newProjectForm}>
        <Grid container direction="row" justifyContent="center" alignItems="center">
          <Grid item>
            <NewProject user={this.props.user} onProjectCreated={this.handleProjectCreated} />
          </Grid>
        </Grid>
      </div>
    );
  }

  projects() {
    const { projects } = this.state;
    return (
      <div className={styles.projectGrid}>
        <ImageList cols={this.getGridListCols()} gap={0}>
          {projects.map((project) => (
            <ImageListItem key={project.id} style={{ height: 'auto' }}>
              <Link to={`/${project.id}`} className={styles.modelLink}>
                <Paper className={styles.paper} elevation={4}>
                  <div className={styles.preview}>
                    <img src={`/api/preview/${project.id}`} alt="model preview" className={styles.previewImg} />
                  </div>
                  <Typography variant="h5" component="h3">
                    {project.displayName}
                  </Typography>
                  <Typography component="p">{project.description}&nbsp;</Typography>
                </Paper>
              </Link>
            </ImageListItem>
          ))}
        </ImageList>
      </div>
    );
  }

  render() {
    const { anchorEl } = this.state;
    const { photoUrl } = this.props.user;
    const open = Boolean(anchorEl);

    const account = photoUrl ? (
      <Avatar alt={this.props.user.displayName} src={photoUrl} className={styles.avatar} />
    ) : (
      <AccountCircle />
    );

    const content = this.props.isNewProject ? this.newProjectForm() : this.projects();

    return (
      <div className={clsx(styles.root)}>
        <AppBar position="fixed">
          <Toolbar variant="dense">
            <IconButton className={styles.menuButton} color="inherit" aria-label="Menu">
              <MenuIcon />
            </IconButton>
            <Typography variant="h6" color="inherit" className={styles.flex}>
              <Link to="/" className={styles.modelLink}>
                Simlin
              </Link>
              {/*&nbsp;*/}
              {/*<span className={classes.sdTitle}>*/}
              {/*  System Dynamics*/}
              {/*</span>*/}
            </Typography>
            <div>
              <Link to="/new" className={styles.modelLink}>
                <Button variant="outlined" className={styles.newProjectButton}>
                  New Project
                </Button>
              </Link>

              <IconButton
                className={styles.profileIcon}
                aria-owns={open ? 'menu-appbar' : undefined}
                aria-haspopup="true"
                onClick={this.handleMenu}
                color="inherit"
              >
                {account}
              </IconButton>
              <Menu
                id="menu-appbar"
                anchorEl={anchorEl}
                anchorOrigin={AnchorOrigin}
                transformOrigin={AnchorOrigin}
                open={open}
                onClose={this.handleClose}
              >
                <MenuItem onClick={this.handleClose}>Logout</MenuItem>
              </Menu>
            </div>
          </Toolbar>
        </AppBar>
        <br />
        <br />
        <br />
        {content}
      </div>
    );
  }
}

export default Home;
