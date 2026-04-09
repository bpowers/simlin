// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Mixed -- class component required by Slate editor lifecycle (same as VariableDetails)

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
import { isStdlibModel } from './module-navigation';
import { addReference, getAvailableSrcVariables, removeReference, updateReferenceDst, updateReferenceSrc } from './module-wiring';
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

interface ModuleDetailsState {
  unitsContents: Descendant[];
  unitsEditor: CustomEditor;
  notesContents: Descendant[];
  notesEditor: CustomEditor;
}

export class ModuleDetails extends React.PureComponent<ModuleDetailsProps, ModuleDetailsState> {
  constructor(props: ModuleDetailsProps) {
    super(props);

    this.state = {
      unitsEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
      unitsContents: plainDeserialize('equation', props.variable.units),
      notesEditor: withHistory(withReact(createEditor())) as unknown as CustomEditor,
      notesContents: plainDeserialize('equation', props.variable.documentation),
    };
  }

  handleDelete = (): void => {
    this.props.onDelete(this.props.variable.ident);
  };

  handleModelRefChange = (e: React.ChangeEvent<HTMLSelectElement>): void => {
    const value = e.target.value;
    if (value === '__create_new__') {
      this.props.onCreateModel(this.props.variable.ident);
    } else if (value === '__duplicate__') {
      this.props.onDuplicateModel(this.props.variable.ident, this.props.variable.modelName);
    } else if (value) {
      this.props.onModelReferenceChange(this.props.variable.ident, value);
    }
  };

  handleOpenModel = (): void => {
    this.props.onDrillIntoModule(this.props.variable.ident, this.props.variable.modelName);
  };

  handleUnitsChange = (value: Descendant[]): void => {
    this.setState({ unitsContents: value });
  };

  handleNotesChange = (value: Descendant[]): void => {
    this.setState({ notesContents: value });
  };

  handleUnitDocsSave = (): void => {
    const { variable } = this.props;
    const { unitsContents, notesContents } = this.state;

    const newUnits = plainSerialize(unitsContents);
    const newDocs = plainSerialize(notesContents);

    const unitsChanged = variable.units !== newUnits;
    const docsChanged = variable.documentation !== newDocs;

    if (unitsChanged || docsChanged) {
      this.props.onUnitsDocsChange(
        variable.ident,
        unitsChanged ? newUnits : undefined,
        docsChanged ? newDocs : undefined,
      );
    }
  };

  renderModelRefSelector(): React.ReactNode {
    const { variable, project, currentModelName } = this.props;
    const { projectModels, stdlibModels } = getAvailableModels(project, currentModelName);
    // Show duplicate for user-defined models, not for stdlib models (read-only)
    const hasModelRef =
      variable.modelName !== '' &&
      project.models.has(variable.modelName) &&
      !isStdlibModel(variable.modelName);

    return (
      <div className={styles.modelRefSection}>
        <div className={styles.modelRefLabel}>Model Reference</div>
        <select
          className={styles.modelRefSelect}
          value={variable.modelName || ''}
          onChange={this.handleModelRefChange}
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
  }

  handleAddReference = (): void => {
    const { variable } = this.props;
    const updated = addReference(variable.references, '', '');
    this.props.onReferencesChange(variable.ident, updated);
  };

  handleRemoveReference = (index: number): void => {
    const { variable } = this.props;
    const updated = removeReference(variable.references, index);
    this.props.onReferencesChange(variable.ident, updated);
  };

  handleSrcChange = (index: number, newSrc: string): void => {
    const { variable } = this.props;
    const updated = updateReferenceSrc(variable.references, index, newSrc);
    this.props.onReferencesChange(variable.ident, updated);
  };

  handleDstChange = (index: number, newDst: string): void => {
    const { variable } = this.props;
    const updated = updateReferenceDst(variable.references, index, newDst);
    this.props.onReferencesChange(variable.ident, updated);
  };

  renderInputWiring(): React.ReactNode {
    const { variable, project, currentModelName } = this.props;

    if (!variable.modelName) {
      return null;
    }

    const parentModel = project.models.get(currentModelName);
    const childModel = project.models.get(variable.modelName);

    const availableSrcVars: ReadonlyArray<string> = parentModel
      ? getAvailableSrcVariables(parentModel.variables)
      : [];
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
                          this.handleSrcChange(i, newValue);
                        }
                      }}
                      renderInput={(params) => (
                        <TextField
                          {...params}
                          variant="standard"
                          placeholder="Select variable"
                        />
                      )}
                    />
                  </td>
                  <td className={styles.wiringDropdown}>
                    <Autocomplete
                      value={ref.dst || null}
                      options={dstOptions}
                      onChange={(_: React.SyntheticEvent | null, newValue: string | null) => {
                        if (newValue) {
                          this.handleDstChange(i, newValue);
                        }
                      }}
                      renderInput={(params) => (
                        <TextField
                          {...params}
                          variant="standard"
                          placeholder="Select input"
                        />
                      )}
                    />
                  </td>
                  <td>
                    <IconButton
                      size="small"
                      aria-label="Remove reference"
                      onClick={() => this.handleRemoveReference(i)}
                    >
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
            onClick={this.handleAddReference}
            data-testid="add-input-button"
          >
            <AddIcon /> Add Input
          </Button>
        </div>
      </div>
    );
  }

  renderOutputPorts(): React.ReactNode {
    const { variable, project } = this.props;

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
  }

  renderUnitsDocsEditors(): React.ReactNode {
    return (
      <>
        <Slate
          editor={this.state.unitsEditor}
          initialValue={this.state.unitsContents}
          onChange={this.handleUnitsChange}
        >
          <Editable
            className={styles.unitsEditor}
            placeholder="Enter units..."
            spellCheck={false}
            onBlur={this.handleUnitDocsSave}
          />
        </Slate>

        <Slate
          editor={this.state.notesEditor}
          initialValue={this.state.notesContents}
          onChange={this.handleNotesChange}
        >
          <Editable
            className={styles.notesEditor}
            placeholder="Documentation"
            spellCheck={false}
            onBlur={this.handleUnitDocsSave}
          />
        </Slate>
      </>
    );
  }

  render(): React.ReactNode {
    const { variable } = this.props;
    const hasModelRef = variable.modelName !== '';

    return (
      <div className={styles.card}>
        <div className={styles.cardContent}>
          <div className={styles.header}>{variable.ident}</div>

          {this.renderModelRefSelector()}

          {hasModelRef && (
            <Button
              size="small"
              color="primary"
              variant="outlined"
              onClick={this.handleOpenModel}
              className={styles.openModelButton}
            >
              Open Model
            </Button>
          )}

          {this.renderInputWiring()}
          {this.renderOutputPorts()}

          {this.renderUnitsDocsEditors()}

          <div className={styles.cardActions}>
            <Button size="small" color="secondary" onClick={this.handleDelete} className={styles.deleteButton}>
              Delete Module
            </Button>
          </div>
        </div>
      </div>
    );
  }
}
