// Copyright 2024 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { useEffect, useState } from 'react';
import { toUint8Array } from 'js-base64';
import { Set } from 'immutable';

import { createTheme, ThemeProvider } from '@mui/material/styles';
import CssBaseline from '@mui/material/CssBaseline';

import { Project, UID, ViewElement } from '@system-dynamics/core/datamodel';
import { defined } from '@system-dynamics/core/common';
import { fromXmile } from '@system-dynamics/importer';
import { Canvas } from '@system-dynamics/diagram/drawing/Canvas';
import { Point } from '@system-dynamics/diagram/drawing/common';

const theme = createTheme({
  palette: {
    mode: 'light',
  },
});

export const VisualTestPage: React.FC = () => {
  const [project, setProject] = useState<Project | undefined>();
  const [error, setError] = useState<string | undefined>();

  useEffect(() => {
    // Expose API for Playwright tests
    (window as any).loadXmileModel = async (xmileContent: string) => {
      try {
        console.log('Loading XMILE model, length:', xmileContent.length);
        const projectBinary = await fromXmile(xmileContent);
        console.log('Got project binary, length:', projectBinary.length);
        const importedProject = Project.deserializeBinary(projectBinary);
        console.log('Deserialized project, models:', importedProject.models.size);
        setProject(importedProject);
        setError(undefined);
        return true;
      } catch (err) {
        setError(String(err));
        console.error('Failed to load model:', err);
        return false;
      }
    };

    (window as any).loadProjectBinary = (base64: string) => {
      try {
        const binary = toUint8Array(base64);
        const project = Project.deserializeBinary(binary);
        setProject(project);
        setError(undefined);
        return true;
      } catch (err) {
        setError(String(err));
        console.error('Failed to load project:', err);
        return false;
      }
    };

    // Signal that the test page is ready
    (window as any).visualTestReady = true;
  }, []);

  if (error) {
    return <div style={{ padding: 20, color: 'red' }}>Error: {error}</div>;
  }

  if (!project) {
    return (
      <div style={{ padding: 20 }}>
        <div id="status">Waiting for model...</div>
        <div style={{ marginTop: 10, fontSize: '12px', color: '#666' }}>
          Visual test page ready: {(window as any).visualTestReady ? 'Yes' : 'No'}
        </div>
      </div>
    );
  }

  // Get the main model or the first available model
  const model = project.models.get('main') || project.models.first();

  if (!model) {
    return <div style={{ padding: 20, color: 'red' }}>Error: No model found in project</div>;
  }

  const view = defined(model.views.get(0));

  // Stub callbacks for static rendering
  const noop = () => {};
  const noopMove = (_element: ViewElement, _target: number, _position: Point) => {};
  const noopMoveSelection = (_position: Point) => {};
  const noopSetSelection = (_selected: Set<UID>) => {};
  const noopMoveLabel = (_uid: UID, _side: 'top' | 'left' | 'bottom' | 'right') => {};

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <div style={{ width: '100vw', height: '100vh', overflow: 'hidden' }}>
        <Canvas
          embedded={true}
          project={project}
          model={model}
          view={view}
          version={1}
          selectedTool={undefined}
          selection={Set()}
          onRenameVariable={noop}
          onSetSelection={noopSetSelection}
          onMoveSelection={noopMoveSelection}
          onMoveFlow={noopMove}
          onMoveLabel={noopMoveLabel}
          onAttachLink={noop}
          onCreateVariable={noop}
          onClearSelectedTool={noop}
          onDeleteSelection={noop}
          onShowVariableDetails={noop}
          onViewBoxChange={noop}
        />
      </div>
    </ThemeProvider>
  );
};
