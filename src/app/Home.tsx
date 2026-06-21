// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'wouter';
import clsx from 'clsx';
import {
  AppBar,
  Button,
  CircularProgress,
  ImageList,
  ImageListItem,
  IconButton,
  Menu,
  MenuItem,
  Paper,
  Toolbar,
  Avatar,
  AccountCircleIcon,
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
  // Distinguishes "still loading", "load failed", and "loaded (maybe empty)" so
  // the three no longer collapse into the same blank page. Without this a fetch
  // failure, a slow network, and a brand-new account all rendered an identical
  // empty grid with no feedback.
  const [loadState, setLoadState] = React.useState<'loading' | 'loaded' | 'error'>('loading');

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
        if (!refs.current.unmounted) {
          setLoadState('error');
        }
        return;
      }
      projects = (await response.json()) as Project[];
    } catch (err) {
      console.error("couldn't fetch projects:", err);
      if (!refs.current.unmounted) {
        setLoadState('error');
      }
      return;
    }
    if (refs.current.unmounted) {
      return;
    }
    setProjects(projects);
    setLoadState('loaded');
  };

  // Re-run the fetch from the error state's Retry button.
  const handleRetry = (): void => {
    setLoadState('loading');
    void getProjects();
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
    if (loadState === 'loading') {
      return (
        <div className={styles.statusArea}>
          <CircularProgress label="Loading your models" />
        </div>
      );
    }

    if (loadState === 'error') {
      return (
        <div className={styles.statusArea}>
          <h2 className={clsx(typography.heading5, styles.statusTitle)}>We couldn&apos;t load your models</h2>
          <p className={clsx(typography.body2, styles.statusBody)}>
            Something went wrong reaching the server. Check your connection and try again.
          </p>
          <Button variant="contained" color="primary" onClick={handleRetry}>
            Retry
          </Button>
        </div>
      );
    }

    if (projects.length === 0) {
      return (
        <div className={styles.statusArea}>
          <h2 className={clsx(typography.heading5, styles.statusTitle)}>No models yet</h2>
          <p className={clsx(typography.body2, styles.statusBody)}>
            Create your first model to start debugging your intuition.
          </p>
          <Link to="/new" className={styles.modelLink}>
            <Button variant="contained" color="primary">
              New Project
            </Button>
          </Link>
        </div>
      );
    }

    return (
      <div className={styles.projectGrid}>
        {/* auto-fill + minmax reflows the grid from 1 column on a phone up to as
            many ~280px columns as fit, replacing the old hardcoded 2-up that
            squeezed two tiny cards side-by-side on mobile. The grid owns the
            gutter (gap), so the cards carry no margin -- inner and outer gutters
            match. min(280px, 100%) keeps a single card from overflowing very
            narrow viewports. */}
        <ImageList gap={24} style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(min(280px, 100%), 1fr))' }}>
          {projects.map((project) => (
            <ImageListItem key={project.id} style={{ height: 'auto' }}>
              <Link to={`/${project.id}`} className={clsx(styles.modelLink, styles.cardLink)}>
                <Paper className={styles.paper} elevation={1}>
                  <div className={styles.preview}>
                    <img src={`/api/preview/${project.id}`} alt="model preview" className={styles.previewImg} />
                  </div>
                  <h3 className={clsx(typography.heading6, styles.cardTitle)}>{project.displayName}</h3>
                  {project.description && (
                    <p className={clsx(typography.body2, styles.cardDescription)}>{project.description}</p>
                  )}
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

  // Render the fallback through Avatar too, so the no-photo state keeps the same
  // 32px circle as the photo state instead of shrinking to a bare 24px icon and
  // shifting the toolbar's trailing edge.
  const account = photoUrl ? (
    <Avatar alt={props.user.displayName} src={photoUrl} className={styles.avatar} />
  ) : (
    <Avatar alt={props.user.displayName} className={styles.avatar}>
      <AccountCircleIcon />
    </Avatar>
  );

  const content = props.isNewProject ? newProjectForm() : projectsView();

  return (
    <div className={clsx(styles.root)}>
      <AppBar position="fixed">
        <Toolbar variant="dense">
          <h6 className={clsx(typography.heading6, typography.colorInherit, styles.appBarTitle)}>
            <Link to="/" className={styles.modelLink}>
              Simlin
            </Link>
          </h6>
          <div className={styles.toolbarActions}>
            <Link to="/new" className={styles.modelLink}>
              <Button variant="outlined" color="inherit">
                New Project
              </Button>
            </Link>

            <IconButton
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
