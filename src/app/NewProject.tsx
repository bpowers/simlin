// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import clsx from 'clsx';

import { fromUint8Array } from '@simlin/core/base64';

import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Button,
  Checkbox,
  FormControlLabel,
  InputAdornment,
  TextField,
  ExpandMoreIcon,
} from '@simlin/diagram';

import { Project } from './Project';
import { User } from './User';
import { projectFromJson } from '@simlin/core/datamodel';
import { Project as EngineProject } from '@simlin/engine';
import type { JsonProject } from '@simlin/engine';

import styles from './NewProject.module.css';
import typography from './typography.module.css';

interface NewProjectProps {
  user: User;
  onProjectCreated: (project: Project) => void;
}

const readFile = (file: Blob): Promise<string> => {
  const reader = new FileReader();

  return new Promise((resolve, reject) => {
    reader.onerror = (err) => {
      reader.abort();
      reject(new DOMException(`Problem parsing input file: ${err.type}`));
    };

    reader.onload = () => {
      resolve(reader.result as string);
    };
    reader.readAsText(file);
  });
};

export function NewProject(props: NewProjectProps): React.JSX.Element {
  const [projectNameField, setProjectNameField] = React.useState('');
  const [descriptionField, setDescriptionField] = React.useState('');
  const [errorMsg, setErrorMsg] = React.useState<string | undefined>(undefined);
  const [projectPB, setProjectPB] = React.useState<Uint8Array | undefined>(undefined);
  // Controlled from the start (false, not undefined) so the Radix checkbox never
  // flips from uncontrolled to controlled on first toggle.
  const [isPublic, setIsPublic] = React.useState<boolean>(false);

  // The deferred setProjectName() (scheduled via setTimeout from handleClose)
  // and the async uploadModel continuation read the freshest form values and
  // the onProjectCreated callback through this ref, so they observe current
  // values rather than those captured when they were kicked off.
  const latest = React.useRef<{
    projectNameField: string;
    descriptionField: string;
    projectPB: Uint8Array | undefined;
    isPublic: boolean;
    onProjectCreated: (project: Project) => void;
  }>(
    undefined as unknown as {
      projectNameField: string;
      descriptionField: string;
      projectPB: Uint8Array | undefined;
      isPublic: boolean;
      onProjectCreated: (project: Project) => void;
    },
  );
  latest.current = {
    projectNameField,
    descriptionField,
    projectPB,
    isPublic,
    onProjectCreated: props.onProjectCreated,
  };

  const handleProjectNameChanged = (event: React.ChangeEvent<HTMLInputElement>): void => {
    setProjectNameField(event.target.value);
  };

  const handleDescriptionChanged = (event: React.ChangeEvent<HTMLInputElement>): void => {
    setDescriptionField(event.target.value);
  };

  const setProjectName = async (): Promise<void> => {
    const bodyContents: {
      projectName: string;
      description: string;
      projectPB?: string;
      isPublic?: boolean;
    } = {
      projectName: latest.current.projectNameField,
      description: latest.current.descriptionField,
    };
    if (latest.current.projectPB) {
      bodyContents.projectPB = fromUint8Array(latest.current.projectPB);
    }
    if (latest.current.isPublic) {
      bodyContents.isPublic = true;
    }
    const response = await fetch('/api/projects', {
      credentials: 'same-origin',
      method: 'POST',
      cache: 'no-cache',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(bodyContents),
    });

    const status = response.status;
    if (!(status >= 200 && status < 400)) {
      const body = await response.json();
      const errorMsg =
        body && body.error
          ? (body.error as string)
          : `We couldn't create your project (HTTP ${status}). Please try again.`;
      setErrorMsg(errorMsg);
      return;
    }

    const project = (await response.json()) as Project;
    latest.current.onProjectCreated(project);
  };

  const handleClose = (): void => {
    if (latest.current.projectNameField === '') {
      setErrorMsg('Please give your project a non-empty name');
    } else {
      setTimeout(setProjectName);
    }
  };

  const handleKeyPress = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      handleClose();
    }
  };

  const handlePublicChecked = (checked: boolean): void => {
    setIsPublic(checked);
  };

  const uploadModel = async (event: React.ChangeEvent<HTMLInputElement>) => {
    if (!event.target || !event.target.files || event.target.files.length <= 0) {
      console.log('expected non-empty list of files?');
      return;
    }
    const file = event.target.files[0];
    const contents = await readFile(file);
    let engineProject: EngineProject;
    try {
      if (file.name.endsWith('.mdl')) {
        engineProject = await EngineProject.openVensim(contents);
      } else {
        engineProject = await EngineProject.open(contents);
      }
    } catch (e) {
      // The engine never produced a handle, so there's nothing to dispose.
      setErrorMsg(`That model couldn't be imported: ${e}`);
      return;
    }

    // Wrap every code path that uses `engineProject` in try/finally so the
    // underlying WASM project handle is released even if serialization or
    // JSON conversion throws. Mirrors the discipline in src/server/render.ts
    // and src/server/project-creation.ts.
    try {
      const projectPB = await engineProject.serializeProtobuf();
      const json = JSON.parse(await engineProject.serializeJson()) as JsonProject;
      const activeProject = projectFromJson(json);
      const views = activeProject.models.get('main')?.views;
      if (!views || views.length === 0) {
        setErrorMsg(
          `That model has no diagram to import yet. Open it in its original tool, add a view, and try again.`,
        );
        return;
      }

      setProjectPB(projectPB);
      setErrorMsg(undefined);
    } catch (e) {
      setErrorMsg(`That model couldn't be imported: ${e}`);
    } finally {
      await engineProject.dispose();
    }
  };

  const warningText = errorMsg || '';
  return (
    <div>
      <h2 className={typography.heading2}>Create a project</h2>
      <p className={clsx(typography.subtitle1, styles.subtitle)}>
        A project holds models and data, along with simulation results.
      </p>
      <div className={styles.formStack}>
        <TextField
          onChange={handleProjectNameChanged}
          autoFocus
          id="projectName"
          label="Project Name"
          type="text"
          error={errorMsg !== undefined}
          onKeyPress={handleKeyPress}
          fullWidth
          InputProps={{
            startAdornment: <InputAdornment position="start">{props.user.id}/</InputAdornment>,
          }}
        />
        <TextField
          onChange={handleDescriptionChanged}
          id="description"
          label="Project Description"
          type="text"
          onKeyPress={handleKeyPress}
          fullWidth
        />

        <Accordion>
          <AccordionSummary expandIcon={<ExpandMoreIcon />}>
            <div>
              <span>Advanced</span>
            </div>
          </AccordionSummary>
          <AccordionDetails>
            <form>
              <div className={styles.advancedGrid}>
                <div className={styles.gridCol8}>
                  <span>Use existing model</span>
                </div>
                <div className={styles.gridCol4}>
                  <Button variant="contained" className="NewProject-upload-button" color="secondary" component="label">
                    Select
                    <input
                      style={{ display: 'none' }}
                      accept=".stmx,.itmx,.xmile,.mdl"
                      id="xmile-model-file"
                      type="file"
                      onChange={uploadModel}
                    />
                  </Button>
                </div>
                <div className={styles.gridCol12}>
                  <FormControlLabel
                    control={<Checkbox checked={isPublic} onChange={handlePublicChecked} />}
                    label="Publicly accessible"
                  />
                </div>
              </div>
            </form>
          </AccordionDetails>
        </Accordion>

        <p className={clsx(typography.subtitle2, styles.warning)}>
          <b>{warningText || '\xa0'}</b>
        </p>
        <div className={styles.actions}>
          <Button onClick={handleClose} color="primary" variant="contained">
            Create
          </Button>
        </div>
      </div>
    </div>
  );
}
