// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'react-router-dom';

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
import withWidth, { isWidthUp, WithWidthProps } from '@material-ui/core/withWidth';

import Avatar from '@material-ui/core/Avatar';

import { List } from 'immutable';

import { PopoverOrigin } from '@material-ui/core/Popover';

import AccountCircle from '@material-ui/icons/AccountCircle';
import MenuIcon from '@material-ui/icons/Menu';

import { createStyles } from '@material-ui/core/styles';
import withStyles, { WithStyles } from '@material-ui/core/styles/withStyles';

import { NewProject } from './NewProject';
import { Project } from './Project';
import { User } from './User';

const styles = createStyles({
  root: {
    flexGrow: 1,
  },
  flex: {
    flex: 1,
  },
  profileIcon: {
    padding: 8,
  },
  sdTitle: {
    fontWeight: 300,
  },
  avatar: {
    width: 32,
    height: 32,
  },
  menuButton: {
    marginLeft: -12,
    marginRight: 20,
  },
  newProjectButton: {
    color: 'white',
    border: '1px solid rgba(255, 255, 255, 0.76)',
    textDecoration: 'none',
    marginRight: 16,
  },
  paper: {
    margin: 24,
    padding: 12,
  },
  preview: {
    textAlign: 'center',
    height: 200,
  },
  previewImg: {
    width: '100%',
    maxHeight: 200,
    objectFit: 'scale-down',
  },
  modelLink: {
    color: 'white',
    textDecoration: 'none',
  },
  newForm: {
    margin: 32,
    padding: 12,
  },
  projectGrid: {
    boxSizing: 'border-box',
    marginLeft: 'auto',
    marginRight: 'auto',
    maxWidth: 1024,
  },
});

interface HomeState {
  anchorEl?: HTMLElement;
  projects: List<Project>;
}

interface HomeProps extends WithStyles<typeof styles> {
  user: User;
  isNewProject: boolean;
  onNewProjectDone?: () => void;
}

const AnchorOrigin: PopoverOrigin = {
  vertical: 'bottom',
  horizontal: 'right',
};

const Home = withWidth()(
  withStyles(styles)(
    class HomeInner extends React.Component<HomeProps & WithWidthProps, HomeState> {
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
        // should never happen, but required for type checking
        if (this.props.width === undefined) {
          return 2;
        }

        if (isWidthUp('sm', this.props.width)) {
          return 2;
        }

        return 1;
      };

      newProjectForm() {
        const { classes } = this.props;
        return (
          <div className={classes.newForm}>
            <Grid container direction="row" justifyContent="center" alignItems="center">
              <Grid item>
                <NewProject user={this.props.user} onProjectCreated={this.handleProjectCreated} />
              </Grid>
            </Grid>
          </div>
        );
      }

      projects() {
        const { classes } = this.props;
        const { projects } = this.state;
        return (
          <div className={classes.projectGrid}>
            <ImageList cols={this.getGridListCols()} gap={0}>
              {projects.map((project) => (
                <ImageListItem key={project.id} style={{ height: 'auto' }}>
                  <Link to={`/${project.id}`} className={classes.modelLink}>
                    <Paper className={classes.paper} elevation={4}>
                      <div className={classes.preview}>
                        <img src={`/api/preview/${project.id}`} alt="model preview" className={classes.previewImg} />
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
        const classes = this.props.classes;
        const { anchorEl } = this.state;
        const { photoUrl } = this.props.user;
        const open = Boolean(anchorEl);

        const account = photoUrl ? (
          <Avatar alt={this.props.user.displayName} src={photoUrl} className={classes.avatar} />
        ) : (
          <AccountCircle />
        );

        const content = this.props.isNewProject ? this.newProjectForm() : this.projects();

        return (
          <div className={classes.root}>
            <AppBar position="fixed">
              <Toolbar variant="dense">
                <IconButton className={classes.menuButton} color="inherit" aria-label="Menu">
                  <MenuIcon />
                </IconButton>
                <Typography variant="h6" color="inherit" className={classes.flex}>
                  <Link to="/" className={classes.modelLink}>
                    Simlin
                  </Link>
                  {/*&nbsp;*/}
                  {/*<span className={classes.sdTitle}>*/}
                  {/*  System Dynamics*/}
                  {/*</span>*/}
                </Typography>
                <div>
                  <Link to="/new" className={classes.modelLink}>
                    <Button variant="outlined" className={classes.newProjectButton}>
                      New Project
                    </Button>
                  </Link>

                  <IconButton
                    className={classes.profileIcon}
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
  ),
);

export default Home;
