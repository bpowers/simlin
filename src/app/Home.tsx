// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'react-router-dom';
import clsx from 'clsx';
import { styled } from '@material-ui/core/styles';
import AppBar from '@material-ui/core/AppBar';
import Button from '@material-ui/core/Button';
import Grid from '@material-ui/core/Grid';
import ImageList from '@material-ui/core/ImageList';
import ImageListItem from '@material-ui/core/ImageListItem';
import IconButton from '@material-ui/core/IconButton';
import Menu from '@material-ui/core/Menu';
import MenuItem from '@material-ui/core/MenuItem';
import Paper from '@material-ui/core/Paper';
import Toolbar from '@material-ui/core/Toolbar';
import Typography from '@material-ui/core/Typography';
import Avatar from '@material-ui/core/Avatar';
import { List } from 'immutable';
import { PopoverOrigin } from '@material-ui/core/Popover';
import AccountCircle from '@material-ui/icons/AccountCircle';
import MenuIcon from '@material-ui/icons/Menu';

import { NewProject } from './NewProject';
import { Project } from './Project';
import { User } from './User';

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

const Home = styled(
  class HomeInner extends React.Component<HomeProps & { className?: string }, HomeState> {
    state: HomeState;

    constructor(props: HomeProps) {
      super(props);

      this.state = {
        anchorEl: undefined,
        projects: List<Project>(),
      };

      // eslint-disable-next-line @typescript-eslint/no-misused-promises
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
        <div className="simlin-home-newprojectform">
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
        <div className="simlin-home-projectgrid">
          <ImageList cols={this.getGridListCols()} gap={0}>
            {projects.map((project) => (
              <ImageListItem key={project.id} style={{ height: 'auto' }}>
                <Link to={`/${project.id}`} className="simlin-home-modellink">
                  <Paper className="simlin-home-paper" elevation={4}>
                    <div className="simlin-home-preview">
                      <img src={`/api/preview/${project.id}`} alt="model preview" className="simlin-home-previewimg" />
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
      const { className } = this.props;
      const { anchorEl } = this.state;
      const { photoUrl } = this.props.user;
      const open = Boolean(anchorEl);

      const account = photoUrl ? (
        <Avatar alt={this.props.user.displayName} src={photoUrl} className="simlin-home-avatar" />
      ) : (
        <AccountCircle />
      );

      const content = this.props.isNewProject ? this.newProjectForm() : this.projects();

      return (
        <div className={clsx(className, 'simlin-home-root')}>
          <AppBar position="fixed">
            <Toolbar variant="dense">
              <IconButton className="simlin-home-menubutton" color="inherit" aria-label="Menu">
                <MenuIcon />
              </IconButton>
              <Typography variant="h6" color="inherit" className="simlin-home-flex">
                <Link to="/" className="simlin-home-modellink">
                  Simlin
                </Link>
                {/*&nbsp;*/}
                {/*<span className={classes.sdTitle}>*/}
                {/*  System Dynamics*/}
                {/*</span>*/}
              </Typography>
              <div>
                <Link to="/new" className="simlin-home-modellink">
                  <Button variant="outlined" className="simlin-home-newprojectbutton">
                    New Project
                  </Button>
                </Link>

                <IconButton
                  className="simlin-home-profileicon"
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
  },
)(({ _theme }) => ({
  '&.simlin-home-root': {
    flexGrow: 1,
  },
  '.simlin-home-flex': {
    flex: 1,
  },
  '.simlin-home-profileicon': {
    padding: 8,
  },
  '.simlin-home-sdtitle': {
    fontWeight: 300,
  },
  '.simlin-home-avatar': {
    width: 32,
    height: 32,
  },
  '.simlin-home-menubutton': {
    marginLeft: -12,
    marginRight: 20,
  },
  '.simlin-home-newprojectbutton': {
    color: 'white',
    border: '1px solid rgba(255, 255, 255, 0.76)',
    textDecoration: 'none',
    marginRight: 16,
  },
  '.simlin-home-paper': {
    margin: 24,
    padding: 12,
  },
  '.simlin-home-preview': {
    textAlign: 'center',
    height: 200,
  },
  '.simlin-home-previewimg': {
    width: '100%',
    maxHeight: 200,
    objectFit: 'scale-down',
  },
  '.simlin-home-modellink': {
    color: 'white',
    textDecoration: 'none',
  },
  '.simlin-home-newform': {
    margin: 32,
    padding: 12,
  },
  '.simlin-home-projectgrid': {
    boxSizing: 'border-box',
    marginLeft: 'auto',
    marginRight: 'auto',
    maxWidth: 1024,
  },
}));

export default Home;
