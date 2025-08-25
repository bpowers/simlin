// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link, useLocation } from 'wouter';
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
import { PopoverOrigin } from '@mui/material/Popover';
import AccountCircle from '@mui/icons-material/AccountCircle';
import MenuIcon from '@mui/icons-material/Menu';

import { NewProject } from './NewProject';
import { Project } from './Project';
import { User } from './User';
import { Suspense, useEffect, useRef, useState } from 'react';
import { useMediaQuery } from '@mui/system';
import { styled } from '@mui/material';

interface HomeProps {
  user: User;
  isNewProject: boolean;
  onNewProjectDone?: () => void;
}

const AnchorOrigin: PopoverOrigin = {
  vertical: 'bottom',
  horizontal: 'right',
};

async function fetchProjects() {
  const response = await fetch('/api/projects', { credentials: 'same-origin' });
  const status = response.status;
  if (!(status >= 200 && status < 400)) {
    console.log("Couldn't fetch projects.");
    return [];
  }
  return (await response.json()) as Project[];
}

function NewProjectForm({ user }: { user: User }) {
  const [, navigate] = useLocation();
  function handleProjectCreated(project: Project) {
    navigate('/' + project.id);
  }
  return (
    <div className="simlin-home-newprojectform">
      <Grid container direction="row" justifyContent="center" alignItems="center">
        <Grid item>
          <NewProject user={user} onProjectCreated={handleProjectCreated} />
        </Grid>
      </Grid>
    </div>
  );
}

function Projects() {
  const isLargeScreen = useMediaQuery('(min-width:600px)');
  const gridListCols = isLargeScreen ? 2 : 1;
  const [projects, setProjects] = useState<Project[]>([]);

  useEffect(() => {
    let ignore = false;
    fetchProjects().then((projects) => {
      if (!ignore) setProjects(projects);
    });
    return () => {
      ignore = true;
    };
  }, []);

  return (
    <div className="simlin-home-projectgrid">
      <ImageList cols={gridListCols} gap={0}>
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

function Home(props: HomeProps & { className?: string }) {
  const anchorEl = useRef<HTMLButtonElement>(null);
  const [isMenuOpen, setIsMenuOpen] = useState(false);

  const account = props.user.photoUrl ? (
    <Avatar alt={props.user.displayName} src={props.user.photoUrl} className="simlin-home-avatar" />
  ) : (
    <AccountCircle />
  );

  return (
    <div className={clsx(props.className, 'simlin-home-root')}>
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
              aria-owns={isMenuOpen ? 'menu-appbar' : undefined}
              aria-haspopup="true"
              onClick={() => setIsMenuOpen(true)}
              color="inherit"
              ref={anchorEl}
            >
              {account}
            </IconButton>
            <Menu
              id="menu-appbar"
              anchorEl={anchorEl.current}
              anchorOrigin={AnchorOrigin}
              transformOrigin={AnchorOrigin}
              open={isMenuOpen}
              onClose={() => setIsMenuOpen(false)}
            >
              <MenuItem onClick={() => setIsMenuOpen(false)}>Logout</MenuItem>
            </Menu>
          </div>
        </Toolbar>
      </AppBar>
      <br />
      <br />
      <br />
      {props.isNewProject ? (
        <NewProjectForm user={props.user} />
      ) : (
        <Suspense fallback={'Loading'}>
          <Projects />
        </Suspense>
      )}
    </div>
  );
}

export default styled(Home)({
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
});
