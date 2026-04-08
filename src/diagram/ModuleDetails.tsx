// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Mixed (React class component with side effects + pure rendering logic)

import * as React from 'react';

import { createEditor, Descendant } from 'slate';
import { withHistory } from 'slate-history';
import { Editable, Slate, withReact } from 'slate-react';

import Button from './components/Button';
import { getAvailableModels, getPublicVariables } from './module-details-utils';
import { plainDeserialize, plainSerialize } from './drawing/common';
import type { CustomEditor } from './drawing/SlateEditor';

import type { Module, Project, Variable, ViewElement } from '@simlin/core/datamodel';

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
    const hasModelRef = variable.modelName !== '';

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

  renderInputWiring(): React.ReactNode {
    const { variable } = this.props;

    if (!variable.modelName) {
      return null;
    }

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
              </tr>
            </thead>
            <tbody>
              {variable.references.map((ref, i) => (
                <tr key={i}>
                  <td>{ref.src}</td>
                  <td>{ref.dst}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
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
