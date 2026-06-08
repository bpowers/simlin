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

interface HomeProps {
  user: User;
  isNewProject: boolean;
  onNewProjectDone?: () => void;
  onLogout: () => void;
}

const AnchorOrigin = {
  vertical: 'bottom' as const,
  horizontal: 'right' as const,
};

function Home(props: HomeProps): React.JSX.Element {
  const [anchorEl, setAnchorEl] = React.useState<HTMLElement | undefined>(undefined);
  const [projects, setProjects] = React.useState<readonly Project[]>([]);

  // The mutable, non-render instance state that lived as class instance fields:
  // the pending setTimeout(0) handle for the deferred getProjects() and an
  // unmount flag. The fetch can resolve after a route change unmounts Home, and
  // the deferred timer must be cancellable from the cleanup. Collected into one
  // ref so escaped callbacks (the timer continuation and the async fetch) read
  // the latest values.
  const refs = React.useRef<{
    getProjectsTimer: ReturnType<typeof setTimeout> | null;
    unmounted: boolean;
  }>({ getProjectsTimer: null, unmounted: false });

  // getProjects reads the freshest onLogout callback indirectly through state
  // setters only, so no latest-props ref is needed here: it only ever calls
  // setProjects, which is stable.
  const getProjects = async (): Promise<void> => {
    let projects: Project[];
    try {
      const response = await fetch('/api/projects', { credentials: 'same-origin' });
      const status = response.status;
      if (!(status >= 200 && status < 400)) {
        console.error(`couldn't fetch projects: HTTP ${status}`);
        return;
      }
      projects = (await response.json()) as Project[];
    } catch (err) {
      console.error("couldn't fetch projects:", err);
      return;
    }
    if (refs.current.unmounted) {
      return;
    }
    setProjects(projects);
  };

  // Mount / unmount effect (formerly componentDidMount / componentWillUnmount).
  // The deferred getProjects() is scheduled here, not during render: a render
  // side effect also runs for StrictMode's discarded render-phase instance.
  // The cleanup cancels the pending timer and latches `unmounted` so a fetch
  // that resolves after a route change does not setProjects on an unmounted
  // component. A StrictMode mount/unmount/mount cycle therefore cancels the
  // first schedule and re-schedules on the second mount.
  React.useEffect(() => {
    const r = refs.current;
    r.unmounted = false;
    r.getProjectsTimer = setTimeout(() => {
      r.getProjectsTimer = null;
      void getProjects();
    });
    return () => {
      r.unmounted = true;
      if (r.getProjectsTimer !== null) {
        clearTimeout(r.getProjectsTimer);
        r.getProjectsTimer = null;
      }
    };
    // Empty deps: this effect mirrors componentDidMount/Unmount. getProjects
    // only closes over the persistent refs object and stable setters, so a
    // once-per-mount run carries no stale values. (The repo lint config does
    // not enable react-hooks/exhaustive-deps, so no disable directive is
    // needed.)
  }, []);

  const handleClose = () => {
    setAnchorEl(undefined);
  };

  const handleLogout = () => {
    handleClose();
    props.onLogout();
  };

  const handleMenu = (event: React.MouseEvent<HTMLElement>) => {
    setAnchorEl(event.currentTarget);
  };

  const handleProjectCreated = (project: Project) => {
    window.location.pathname = '/' + project.id;
  };

  const getGridListCols = () => {
    // TODO: this should be 1 on small screens, but useMediaQuery doesn't
    //       work in class components, only function components.
    return 2;
  };

  const newProjectForm = () => {
    return (
      <div className={styles.newProjectForm}>
        <div className={styles.centeredFlex}>
          <div>
            <NewProject user={props.user} onProjectCreated={handleProjectCreated} />
          </div>
        </div>
      </div>
    );
  };

  const projectsView = () => {
    return (
      <div className={styles.projectGrid}>
        <ImageList cols={getGridListCols()} gap={0}>
          {projects.map((project) => (
            <ImageListItem key={project.id} style={{ height: 'auto' }}>
              <Link to={`/${project.id}`} className={styles.modelLink}>
                <Paper className={styles.paper} elevation={4}>
                  <div className={styles.preview}>
                    <img src={`/api/preview/${project.id}`} alt="model preview" className={styles.previewImg} />
                  </div>
                  <h3 className={clsx(typography.heading5, styles.cardTitle)}>{project.displayName}</h3>
                  <p>{project.description}&nbsp;</p>
                </Paper>
              </Link>
            </ImageListItem>
          ))}
        </ImageList>
      </div>
    );
  };

  const { photoUrl } = props.user;
  const open = Boolean(anchorEl);

  const account = photoUrl ? (
    <Avatar alt={props.user.displayName} src={photoUrl} className={styles.avatar} />
  ) : (
    <AccountCircleIcon />
  );

  const content = props.isNewProject ? newProjectForm() : projectsView();

  return (
    <div className={clsx(styles.root)}>
      <AppBar position="fixed">
        <Toolbar variant="dense">
          <IconButton className={styles.menuButton} color="inherit" aria-label="Menu" edge="start" size="small">
            <MenuIcon />
          </IconButton>
          <h6 className={clsx(typography.heading6, typography.colorInherit, styles.appBarTitle)}>
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
              onClick={handleMenu}
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
              onClose={handleClose}
            >
              <MenuItem onClick={handleLogout}>Logout</MenuItem>
            </Menu>
          </div>
        </Toolbar>
      </AppBar>
      <div className={styles.toolbarSpacer} />
      {content}
    </div>
  );
}

export default Home;
