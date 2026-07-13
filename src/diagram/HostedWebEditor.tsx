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
import {
  HostedWebEditorError,
  ProjectEndpoint,
  SaveErrorReason,
  loadProject,
  saveProject,
} from './hosted-web-editor-core';
// Imported as a namespace so the delete-flow navigation and DELETE go through
// `core.*`, which a test can intercept with a spy (jsdom's
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

// A save failure the loaded editor must keep visible until a later save
// succeeds (issue #928): serviceErrors only render in the pre-load placeholder,
// so routing save failures there made them invisible exactly when the user had
// work to lose.
interface SaveFailure {
  reason: SaveErrorReason;
  message: string;
}

export function HostedWebEditor(props: HostedWebEditorProps): React.ReactElement {
  const { username, projectName, embedded, readOnlyMode, moduleCreationEnabled } = props;

  const [serviceErrors, setServiceErrors] = React.useState<readonly Error[]>([]);
  const [projectBinary, setProjectBinary] = React.useState<Readonly<Uint8Array> | undefined>(undefined);
  const [projectVersion, setProjectVersion] = React.useState<number>(-1);
  const [saveFailure, setSaveFailure] = React.useState<SaveFailure | undefined>(undefined);
  // Set when a save 409s: the server has a newer version than `currVersion`.
  // The editor only learns a new version from a successful save, so in
  // practice every autosave after a conflict carries this same stale version
  // and would just 409 again -- handleSave suppresses those instead of
  // hammering the server. Not quite an invariant: the controller's fractional
  // +0.01 cache-key bumps can drift toInt(projectVersion) across an integer
  // boundary after ~100 edits (issue #958), which is why suppression matches
  // this exact version instead of latching -- a drifted version POSTs once,
  // 409s again, and re-arms suppression at the new value. Cleared on a
  // successful save and irrelevant after the reload recovery.
  const staleVersion = React.useRef<number | undefined>(undefined);

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

    if (staleVersion.current === currVersion) {
      // A known-stale version: re-POSTing would just 409 again (see the
      // staleVersion comment above). The conflict banner is already up; keep
      // the editor's in-memory work intact and skip the request.
      return undefined;
    }

    const result = await saveProject(makeEndpoint(), project, currVersion);
    if (result.kind === 'error') {
      if (result.reason === 'conflict') {
        staleVersion.current = currVersion;
      }
      setSaveFailure({ reason: result.reason, message: result.message });
      return undefined;
    }
    staleVersion.current = undefined;
    setSaveFailure(undefined);
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

  // The persistent save-failure banner (issue #928). It floats over the top of
  // the canvas without blocking editing, and it is deliberately NOT dismissible:
  // while saves are failing the honest state is "your work is not being saved",
  // and only a subsequent successful save (or the reload recovery) clears it.
  // The Editor's own toast list is transient and Editor-internal, so the shell
  // owns this surface.
  const getSaveFailureBanner = (): React.ReactNode => {
    if (!saveFailure) {
      return undefined;
    }

    let title: string;
    let body: string;
    let action: React.ReactNode;
    switch (saveFailure.reason) {
      case 'conflict':
        title = 'This project changed somewhere else';
        body =
          'A newer version of this project exists, usually from an edit in another tab or window. ' +
          "You can keep editing here, but new changes won't be saved. " +
          'Reloading picks up the latest version and discards your unsaved changes in this window.';
        // Reload discards local work, so it is styled as a destructive choice,
        // not the primary happy path; "keep editing" is simply not clicking.
        action = (
          <Button variant="outlined" color="error" size="small" onClick={() => core.reloadPage()}>
            Reload and discard my changes
          </Button>
        );
        break;
      case 'unauthorized':
        title = 'Your session expired';
        body =
          "Your work is still here, but it can't be saved until you sign in again. " +
          'Sign in from a new tab so this window keeps your changes, then make any edit to save.';
        action = (
          <Button
            variant="contained"
            color="primary"
            size="small"
            onClick={() => core.openSignInPage(`${getBaseURL()}/`)}
          >
            Sign in in a new tab
          </Button>
        );
        break;
      default:
        title = 'Changes not saved';
        body = "We couldn't save your latest changes. We'll try again as you keep editing.";
        action = undefined;
    }

    return (
      <div className={styles.saveBanner} role="alert">
        <p className={styles.saveBannerTitle}>{title}</p>
        <p className={styles.saveBannerBody}>{body}</p>
        {saveFailure.reason === 'other' ? <p className={styles.saveBannerDetail}>{saveFailure.message}</p> : undefined}
        {action ? <div className={styles.saveBannerActions}>{action}</div> : undefined}
      </div>
    );
  };

  // Embedded hosts get a positioned (but otherwise layout-neutral) wrapper so
  // the absolutely-positioned save banner anchors to the embed element instead
  // of escaping to the page, mirroring the .centerEmbedded rationale.
  const classNames = embedded ? styles.embedHost : styles.bg;

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
      {getSaveFailureBanner()}
    </div>
  );
}
