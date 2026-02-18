// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'wouter';
import clsx from 'clsx';
import {
  AppBar,
  Button,
  ImageList,
  ImageListItem,
  IconButton,
  Menu,
  MenuItem,
  Paper,
  Toolbar,
  Avatar,
  AccountCircleIcon,
  MenuIcon,
} from '@simlin/diagram';

import { NewProject } from './NewProject';
import { Project } from './Project';
import { User } from './User';

import styles from './Home.module.css';
import typography from './typography.module.css';

interface HomeState {
  anchorEl?: HTMLElement;
  projects: readonly Project[];
}

interface HomeProps {
  user: User;
  isNewProject: boolean;
  onNewProjectDone?: () => void;
}

const AnchorOrigin = {
  vertical: 'bottom' as const,
  horizontal: 'right' as const,
};

class Home extends React.Component<HomeProps, HomeState> {
  state: HomeState;

  constructor(props: HomeProps) {
    super(props);

    this.state = {
      anchorEl: undefined,
      projects: [],
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
      projects,
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
        <div className={styles.centeredFlex}>
          <div>
            <NewProject user={this.props.user} onProjectCreated={this.handleProjectCreated} />
          </div>
        </div>
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
                  <h3 className={typography.heading5}>{project.displayName}</h3>
                  <p>{project.description}&nbsp;</p>
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
      <AccountCircleIcon />
    );

    const content = this.props.isNewProject ? this.newProjectForm() : this.projects();

    return (
      <div className={clsx(styles.root)}>
        <AppBar position="fixed">
          <Toolbar variant="dense">
            <IconButton className={styles.menuButton} color="inherit" aria-label="Menu" edge="start" size="small">
              <MenuIcon />
            </IconButton>
            <h6 className={clsx(typography.heading6, typography.colorInherit, styles.flex)}>
              <Link to="/" className={styles.modelLink}>
                Simlin
              </Link>
            </h6>
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
                size="small"
              >
                {account}
              </IconButton>
              <Menu
                id="menu-appbar"
                anchorEl={anchorEl ?? null}
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
        <div className={styles.toolbarSpacer} />
        {content}
      </div>
    );
  }
}

export default Home;
