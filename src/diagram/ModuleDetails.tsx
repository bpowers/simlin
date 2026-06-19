// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createEditor, Descendant } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, Slate, withReact } from 'slate-react';

import Autocomplete from './components/Autocomplete';
import Button from './components/Button';
import IconButton from './components/IconButton';
import TextField from './components/TextField';
import { AddIcon, RemoveIcon } from './components/icons';
import { getAvailableModels, getInputPorts, getPublicVariables } from './module-details-utils';
import { STDLIB_PREFIX } from './module-navigation';
import {
  addReference,
  getAvailableSrcVariables,
  removeReference,
  updateReferenceDst,
  updateReferenceSrc,
} from './module-wiring';
import { plainDeserialize, plainSerialize } from './drawing/common';
import type { CustomEditor } from './drawing/SlateEditor';

import type { Module, ModuleReference, Project, Variable, ViewElement } from '@simlin/core/datamodel';

import styles from './ModuleDetails.module.css';

interface ModuleDetailsProps {
  variable: Module;
  viewElement: ViewElement;
  project: Project;
  currentModelName: string;
  onDelete: (ident: string) => void;
  onModelReferenceChange: (ident: string, newModelName: string) => void;
  onUnitsDocsChange: (ident: string, newUnits: string | undefined, newDocs: string | undefined) => void;
  onDrillIntoModule: (moduleIdent: string, targetModelName: string) => void;
  onCreateModel: (moduleName: string) => void;
  onDuplicateModel: (moduleIdent: string, sourceModelName: string) => void;
  onReferencesChange: (ident: string, newReferences: ReadonlyArray<ModuleReference>) => void;
}

export function ModuleDetails(props: ModuleDetailsProps): React.ReactElement {
  const {
    variable,
    project,
    currentModelName,
    onDelete,
    onModelReferenceChange,
    onUnitsDocsChange,
    onDrillIntoModule,
    onCreateModel,
    onDuplicateModel,
    onReferencesChange,
  } = props;

  // Seed the Slate editors and their contents from props exactly once per mount
  // (lazy useState initializers), mirroring the old constructor. The Editor keys
  // this panel on projectGeneration, so a content change remounts the panel and
  // re-seeds it -- there is deliberately NO prop-sync effect here, which would
  // fight that keyed-remount invariant (see diagram/CLAUDE.md "Details panels are
  // keyed by projectGeneration").
  const [unitsEditor] = React.useState<CustomEditor>(
    () => withHistory(withReact(createEditor())) as unknown as CustomEditor,
  );
  const [unitsContents, setUnitsContents] = React.useState<Descendant[]>(() =>
    plainDeserialize('equation', variable.units),
  );
  const [notesEditor] = React.useState<CustomEditor>(
    () => withHistory(withReact(createEditor())) as unknown as CustomEditor,
  );
  const [notesContents, setNotesContents] = React.useState<Descendant[]>(() =>
    plainDeserialize('equation', variable.documentation),
  );

  const handleDelete = (): void => {
    onDelete(variable.ident);
  };

  const handleModelRefChange = (e: React.ChangeEvent<HTMLSelectElement>): void => {
    const value = e.target.value;
    if (value === '__create_new__') {
      onCreateModel(variable.ident);
    } else if (value === '__duplicate__') {
      onDuplicateModel(variable.ident, variable.modelName);
    } else if (value) {
      onModelReferenceChange(variable.ident, value);
    }
  };

  const handleOpenModel = (): void => {
    onDrillIntoModule(variable.ident, variable.modelName);
  };

  const handleUnitsChange = (value: Descendant[]): void => {
    setUnitsContents(value);
  };

  const handleNotesChange = (value: Descendant[]): void => {
    setNotesContents(value);
  };

  const handleUnitDocsSave = (): void => {
    const newUnits = plainSerialize(unitsContents);
    const newDocs = plainSerialize(notesContents);

    const unitsChanged = variable.units !== newUnits;
    const docsChanged = variable.documentation !== newDocs;

    if (unitsChanged || docsChanged) {
      onUnitsDocsChange(variable.ident, unitsChanged ? newUnits : undefined, docsChanged ? newDocs : undefined);
    }
  };

  const renderModelRefSelector = (): React.ReactNode => {
    const { projectModels, stdlibModels } = getAvailableModels(project, currentModelName);
    // Show duplicate for user-defined models, not for stdlib models (read-only).
    // Use prefix check so user models with bare stdlib names (e.g. "delay1")
    // are still eligible for duplication.
    const hasModelRef =
      variable.modelName !== '' &&
      project.models.has(variable.modelName) &&
      !variable.modelName.startsWith(STDLIB_PREFIX);

    return (
      <div className={styles.modelRefSection}>
        <div className={styles.modelRefLabel}>Model Reference</div>
        <select
          className={styles.modelRefSelect}
          value={variable.modelName || ''}
          onChange={handleModelRefChange}
          data-testid="model-ref-select"
        >
          <option value="">Select a model to instantiate</option>

          {projectModels.length > 0 && (
            <optgroup label="Project Models">
              {projectModels.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </optgroup>
          )}

          {stdlibModels.length > 0 && (
            <optgroup label="Standard Library">
              {stdlibModels.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </optgroup>
          )}

          <optgroup label="Actions">
            <option value="__create_new__">Create new model</option>
            {hasModelRef && <option value="__duplicate__">Duplicate current model</option>}
          </optgroup>
        </select>
      </div>
    );
  };

  const handleAddReference = (): void => {
    const updated = addReference(variable.references, '', '');
    onReferencesChange(variable.ident, updated);
  };

  const handleRemoveReference = (index: number): void => {
    const updated = removeReference(variable.references, index);
    onReferencesChange(variable.ident, updated);
  };

  const handleSrcChange = (index: number, newSrc: string): void => {
    const updated = updateReferenceSrc(variable.references, index, newSrc);
    onReferencesChange(variable.ident, updated);
  };

  const handleDstChange = (index: number, newDst: string): void => {
    const updated = updateReferenceDst(variable.references, index, newDst);
    onReferencesChange(variable.ident, updated);
  };

  const renderInputWiring = (): React.ReactNode => {
    if (!variable.modelName) {
      return null;
    }

    const parentModel = project.models.get(currentModelName);
    const childModel = project.models.get(variable.modelName);

    const availableSrcVars: ReadonlyArray<string> = parentModel ? getAvailableSrcVariables(parentModel.variables) : [];
    const inputPorts: ReadonlyArray<Variable> = childModel ? getInputPorts(childModel) : [];
    const dstOptions: Array<string> = inputPorts.map((v) => v.ident).sort();

    return (
      <div className={styles.section}>
        <div className={styles.sectionTitle}>Input Wiring</div>
        {variable.references.length === 0 ? (
          <div className={styles.emptyMessage}>No inputs configured</div>
        ) : (
          <table className={styles.wiringTable}>
            <thead>
              <tr>
                <th>Source (parent)</th>
                <th>Destination (module)</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {variable.references.map((ref, i) => (
                <tr key={i} className={styles.wiringRow}>
                  <td className={styles.wiringDropdown}>
                    <Autocomplete
                      value={ref.src || null}
                      options={[...availableSrcVars]}
                      onChange={(_: React.SyntheticEvent | null, newValue: string | null) => {
                        if (newValue) {
                          handleSrcChange(i, newValue);
                        }
                      }}
                      renderInput={(params) => (
                        <TextField {...params} variant="standard" placeholder="Select variable" />
                      )}
                    />
                  </td>
                  <td className={styles.wiringDropdown}>
                    <Autocomplete
                      value={ref.dst || null}
                      options={dstOptions}
                      onChange={(_: React.SyntheticEvent | null, newValue: string | null) => {
                        if (newValue) {
                          handleDstChange(i, newValue);
                        }
                      }}
                      renderInput={(params) => <TextField {...params} variant="standard" placeholder="Select input" />}
                    />
                  </td>
                  <td>
                    <IconButton size="small" aria-label="Remove reference" onClick={() => handleRemoveReference(i)}>
                      <RemoveIcon />
                    </IconButton>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <div className={styles.addInputButton}>
          <Button
            size="small"
            variant="outlined"
            startIcon={<AddIcon />}
            onClick={handleAddReference}
            data-testid="add-input-button"
          >
            Add Input
          </Button>
        </div>
      </div>
    );
  };

  const renderOutputPorts = (): React.ReactNode => {
    if (!variable.modelName) {
      return null;
    }

    const referencedModel = project.models.get(variable.modelName);
    let publicVars: ReadonlyArray<Variable> = [];
    if (referencedModel) {
      publicVars = getPublicVariables(referencedModel);
    }

    return (
      <div className={styles.section}>
        <div className={styles.sectionTitle}>Output Ports</div>
        {publicVars.length === 0 ? (
          <div className={styles.emptyMessage}>No public outputs</div>
        ) : (
          <ul className={styles.portList}>
            {publicVars.map((v) => (
              <li key={v.ident}>{v.ident}</li>
            ))}
          </ul>
        )}
      </div>
    );
  };

  const renderUnitsDocsEditors = (): React.ReactNode => {
    return (
      <>
        <Slate editor={unitsEditor} initialValue={unitsContents} onChange={handleUnitsChange}>
          <Editable
            className={styles.unitsEditor}
            placeholder="Enter units..."
            spellCheck={false}
            onBlur={handleUnitDocsSave}
          />
        </Slate>

        <Slate editor={notesEditor} initialValue={notesContents} onChange={handleNotesChange}>
          <Editable
            className={styles.notesEditor}
            placeholder="Documentation"
            spellCheck={false}
            onBlur={handleUnitDocsSave}
          />
        </Slate>
      </>
    );
  };

  const hasModelRef = variable.modelName !== '';

  return (
    <div className={styles.card}>
      <div className={styles.cardContent}>
        <div className={styles.header}>{variable.ident}</div>

        {renderModelRefSelector()}

        {hasModelRef && (
          <Button
            size="small"
            color="primary"
            variant="outlined"
            onClick={handleOpenModel}
            className={styles.openModelButton}
          >
            Open Model
          </Button>
        )}

        {renderInputWiring()}
        {renderOutputPorts()}

        {renderUnitsDocsEditors()}

        <div className={styles.cardActions}>
          <Button size="small" color="error" onClick={handleDelete} className={styles.deleteButton}>
            Delete Module
          </Button>
        </div>
      </div>
    </div>
  );
}
