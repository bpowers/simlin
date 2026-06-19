// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import React from 'react';
import { renderToString } from 'react-dom/server';
import { toUint8Array } from '@simlin/core/base64';

import './theme.css';

import { Series } from '@simlin/core/common';
import { at, getOrThrow } from '@simlin/core/collections';
import { UID, ViewElement, Project, projectFromJson, projectAttachData } from '@simlin/core/datamodel';
import { Project as EngineProject } from '@simlin/engine';
import type { JsonProject } from '@simlin/engine';
import { Point } from './drawing/common';
import { Canvas } from './drawing/Canvas';

interface DiagramProps {
  isDarkTheme?: boolean;
  projectPbBase64: string;
  project?: Project; // Pre-loaded project for SSR
  data?: ReadonlyMap<string, Series>;
}

export function StaticDiagram(props: DiagramProps): React.ReactElement | null {
  const { isDarkTheme, projectPbBase64, project: ssrProject, data } = props;

  // Seed from the pre-loaded SSR project (attaching data if provided) exactly
  // once, mirroring the old constructor's one-shot derivation. A lazy
  // useState initializer matches the constructor-once semantics: it does not
  // re-run when props change on subsequent renders.
  const [project, setProject] = React.useState<Project | undefined>(() => {
    let initial = ssrProject;
    if (initial && data !== undefined) {
      initial = projectAttachData(initial, data, 'main');
    }
    return initial;
  });

  // Async load when there is no pre-loaded project (i.e. not SSR), replacing
  // componentDidMount. A `cancelled` flag guards the post-await setState so a
  // StrictMode mount/unmount/mount cycle (or any unmount mid-load) does not
  // update state on an unmounted tree.
  React.useEffect(() => {
    if (project) {
      return undefined;
    }
    let cancelled = false;
    void (async () => {
      const serializedProject = toUint8Array(projectPbBase64);
      const engineProject = await EngineProject.openProtobuf(serializedProject);
      const json = JSON.parse(await engineProject.serializeJson()) as JsonProject;
      let loaded = projectFromJson(json);
      await engineProject.dispose();

      if (data !== undefined) {
        loaded = projectAttachData(loaded, data, 'main');
      }

      if (!cancelled) {
        setProject(loaded);
      }
    })();
    return () => {
      cancelled = true;
    };
    // Intentionally empty deps: this effect mirrors componentDidMount -- the
    // load is keyed to mount, not to prop identity. The SSR seed already
    // covers the has-project case, and the pb/data inputs are fixed for a
    // given mounted diagram. (The repo lint config does not enable
    // react-hooks/exhaustive-deps, so no disable directive is needed.)
  }, []);

  if (!project) {
    return null;
  }

  const canUseDOM = !!(typeof window !== 'undefined' && window.document && window.document.createElement);

  const model = getOrThrow(project.models, 'main');

  const renameVariable = (_oldName: string, _newName: string): void => {};
  const onSelection = (_selected: ReadonlySet<UID>): void => {};
  const moveSelection = (_position: Point): void => {};
  const moveFlow = (_element: ViewElement, _target: number, _position: Point): void => {};
  const moveLabel = (_uid: UID, _side: 'top' | 'left' | 'bottom' | 'right'): void => {};
  const attachLink = (_element: ViewElement, _to: string): void => {};
  const createCb = (_element: ViewElement): void => {};
  const nullCb = (): void => {};

  const canvasElement = (
    <Canvas
      embedded={true}
      project={project}
      model={model}
      view={at(model.views, 0)}
      version={1}
      selectedTool={undefined}
      selection={new Set()}
      onRenameVariable={renameVariable}
      onSetSelection={onSelection}
      onMoveSelection={moveSelection}
      onMoveFlow={moveFlow}
      onMoveLabel={moveLabel}
      onAttachLink={attachLink}
      onCreateVariable={createCb}
      onClearSelectedTool={nullCb}
      onDeleteSelection={nullCb}
      onShowVariableDetails={nullCb}
      onViewBoxChange={nullCb}
      onDrillIntoModule={nullCb}
    />
  );

  // This is the only place in the app that sets data-theme, so dark mode applies
  // ONLY to the canvas primitives rendered here (themed via theme.css). The
  // interactive editor chrome and app shell are intentionally light-only for now.
  const themedCanvas = <div data-theme={isDarkTheme ? 'dark' : undefined}>{canvasElement}</div>;

  if (canUseDOM) {
    return themedCanvas;
  } else {
    renderToString(themedCanvas);
    return <>{themedCanvas}</>;
  }
}
