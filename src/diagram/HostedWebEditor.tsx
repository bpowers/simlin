// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { baseURL } from '@simlin/core/common';
import { first } from '@simlin/core/collections';

import { Editor, ProtobufProjectData } from './Editor';
import Button from './components/Button';
import CircularProgress from './components/CircularProgress';
import { ErrorBoundary } from './ErrorBoundary';
import { HostedWebEditorError, ProjectEndpoint, loadProject, saveProject } from './hosted-web-editor-core';
// Imported as a namespace so the delete-flow navigation and DELETE go through
// `core.*`, which a test can intercept with jest.spyOn (jsdom's
// window.location.assign is itself non-spyable).
import * as core from './hosted-web-editor-core';

import styles from './HostedWebEditor.module.css';

interface HostedWebEditorProps {
  username: string;
  projectName: string;
  embedded?: boolean;
  baseURL?: string;
  readOnlyMode?: boolean;
  // Forwarded to Editor: gates the module-creation tool. The app supplies this
  // from its build environment so production hides the still-maturing feature.
  moduleCreationEnabled?: boolean;
}

export function HostedWebEditor(props: HostedWebEditorProps): React.ReactElement {
  const { username, projectName, embedded, readOnlyMode, moduleCreationEnabled } = props;

  const [serviceErrors, setServiceErrors] = React.useState<readonly Error[]>([]);
  const [projectBinary, setProjectBinary] = React.useState<Readonly<Uint8Array> | undefined>(undefined);
  const [projectVersion, setProjectVersion] = React.useState<number>(-1);

  const getBaseURL = (): string => props.baseURL ?? baseURL;

  // The project endpoint is rebuilt per call (cheap) so escaped async callbacks
  // never close over a stale base/username/projectName.
  const makeEndpoint = (): ProjectEndpoint => ({
    base: getBaseURL(),
    username,
    projectName,
  });

  const appendServiceError = (msg: string): void => {
    setServiceErrors((prev) => [...prev, new HostedWebEditorError(msg)]);
  };

  // Mount guard for post-await setState, mirroring the class's `unmounted` flag.
  // Cleared in the effect cleanup so a load that already left the macrotask queue
  // (the timer drained before unmount) short-circuits instead of setState-ing on
  // an unmounted tree.
  const mounted = React.useRef(false);

  // Kick off the project load, deferred a macrotask exactly as the class's
  // componentDidMount setTimeout(0) was. The deferral is what makes the request
  // fire ONCE under React 18+ StrictMode: StrictMode drives the committed
  // component through mount -> unmount -> mount, so this becomes
  // schedule -> cancel (cleanup clearTimeout) -> schedule -- the throwaway first
  // mount's timer never fires loadProject(), and only the live mount's does.
  // (A plain `void loadProject()` in the effect body would fire the fetch on the
  // first mount before the cleanup can cancel it, issuing two network requests.)
  React.useEffect(() => {
    mounted.current = true;

    const timer = setTimeout(() => {
      void (async () => {
        const result = await loadProject(makeEndpoint());
        if (!mounted.current) {
          return;
        }
        if (result.kind === 'loaded') {
          setProjectBinary(result.projectBinary);
          setProjectVersion(result.projectVersion);
        } else {
          appendServiceError(result.message);
        }
      })();
    });

    return () => {
      mounted.current = false;
      clearTimeout(timer);
    };
    // Empty deps: the load runs once per committed mount. username/projectName/
    // baseURL are captured via makeEndpoint at call time; a host that swaps them
    // remounts via the App route, not an in-place rerender.
  }, []);

  const handleSave = async (project: ProtobufProjectData, currVersion: number): Promise<number | undefined> => {
    if (readOnlyMode) return undefined;

    const result = await saveProject(makeEndpoint(), project, currVersion);
    if (result.kind === 'error') {
      appendServiceError(result.message);
      return undefined;
    }
    setProjectVersion(result.version);
    return result.version;
  };

  const handleDelete = async (): Promise<void> => {
    if (readOnlyMode) return;

    // deleteProject throws on failure so the in-editor confirmation dialog (which
    // stays open for a retry) can surface the message; once a project loads,
    // serviceErrors are no longer rendered. On success it returns the home URL.
    const homeUrl = await core.deleteProject(makeEndpoint());
    // Full navigation back to the project list so it refetches without the
    // just-deleted project. Routed through the core namespace so it is mockable.
    core.redirectToHome(homeUrl);
  };

  if (!projectBinary || !projectVersion) {
    // A load failure used to render bare, unstyled error text; the in-flight
    // state used to be a blank <div/>. Both now render a styled, centered
    // surface. In embedded mode it fills the embed element (no fixed-viewport
    // overlay) so a slow or failed embedded model never covers the host page --
    // mirroring the success branch, which also drops the full-viewport `.bg`
    // when embedded.
    const placeholderClass = embedded ? styles.centerEmbedded : styles.center;
    if (serviceErrors.length > 0) {
      return (
        <div className={placeholderClass}>
          <div className={styles.errorBox} role="alert">
            <p className={styles.errorTitle}>We couldn&apos;t open this model</p>
            <p className={styles.errorMessage}>{first(serviceErrors).message}</p>
            <Button variant="contained" color="primary" onClick={() => window.location.reload()}>
              Reload
            </Button>
          </div>
        </div>
      );
    }
    return (
      <div className={placeholderClass}>
        <CircularProgress label="Loading model" />
      </div>
    );
  }

  const classNames = embedded ? undefined : styles.bg;

  return (
    <div className={classNames}>
      <ErrorBoundary resetKey={`${username}/${projectName}`} context={{ project: `${username}/${projectName}` }}>
        <Editor
          inputFormat="protobuf"
          initialProjectBinary={projectBinary}
          initialProjectVersion={projectVersion}
          name={projectName}
          embedded={embedded}
          onSave={handleSave}
          onDeleteProject={readOnlyMode ? undefined : handleDelete}
          readOnlyMode={readOnlyMode}
          moduleCreationEnabled={moduleCreationEnabled}
        />
      </ErrorBoundary>
    </div>
  );
}
