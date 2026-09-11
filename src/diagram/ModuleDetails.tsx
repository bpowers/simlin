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
  qualifyDst,
  removeReference,
  unqualifyDst,
  updateReferenceDst,
  updateReferenceSrc,
} from './module-wiring';
import { plainDeserialize, plainSerialize } from './drawing/common';
import {
  basesAfterFailedSubmission,
  draftText,
  fallBackOnFailedPending,
  pendingTexts,
  type DraftFields,
  type PendingSubmission,
} from './VariableDetails';
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
  // May resolve whether the submission landed (see draftText in VariableDetails).
  onUnitsDocsChange: (
    ident: string,
    newUnits: string | undefined,
    newDocs: string | undefined,
  ) => Promise<boolean> | void;
  onDrillIntoModule: (moduleIdent: string, targetModelName: string) => void;
  onCreateModel: (moduleName: string) => void;
  onDuplicateModel: (moduleIdent: string, sourceModelName: string) => void;
  onReferencesChange: (ident: string, newReferences: ReadonlyArray<ModuleReference>) => void;
  // Inspection-only mode (issue #935): the model reference, wiring, output
  // ports, and units/docs stay visible (and Open Model still drills in), but
  // the reference selector is disabled, the wiring rows render as static text
  // without add/remove, the units/docs fields are non-editable, and the
  // module-delete affordance is hidden.
  readOnly?: boolean;
  // Registers the units/docs draft commit with the host; see the same prop on
  // VariableDetails.
  registerDraftFlush?: (flush: () => boolean) => () => void;
  // Reports whether units or docs hold a draft; see the same prop on VariableDetails.
  onDraftStateChange?: (hasDraft: boolean) => void;
  // The host's latest pending submission for this element; see the same prop on
  // VariableDetails.
  pendingSubmission?: PendingSubmission;
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
  const readOnly = !!props.readOnly;

  // Seed the Slate editors and their contents from props exactly once per mount
  // (lazy useState initializers), mirroring the old constructor. The Editor keys
  // this panel on the module's committed editable content, so a landed edit to
  // it remounts the panel and re-seeds it -- there is deliberately NO prop-sync
  // effect here, which would fight that keyed-remount invariant (see
  // diagram/CLAUDE.md "Details panels are keyed by the selected variable's
  // committed content").
  const [unitsEditor] = React.useState<CustomEditor>(
    () => withHistory(withReact(createEditor())) as unknown as CustomEditor,
  );
  const pendingAtMount = React.useRef(props.pendingSubmission);
  const [unitsContents, setUnitsContents] = React.useState<Descendant[]>(() =>
    plainDeserialize('equation', pendingAtMount.current?.units?.text ?? variable.units),
  );
  const [notesEditor] = React.useState<CustomEditor>(
    () => withHistory(withReact(createEditor())) as unknown as CustomEditor,
  );
  const [notesContents, setNotesContents] = React.useState<Descendant[]>(() =>
    plainDeserialize('equation', pendingAtMount.current?.docs?.text ?? variable.documentation),
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

  // The texts the fields were seeded with, and what this panel last submitted
  // for each; a field's draft is as VariableDetails defines it (draftText).
  const [seeded] = React.useState(() => ({ units: variable.units, docs: variable.documentation }));
  const [submitted, setSubmitted] = React.useState<Partial<Record<keyof DraftFields<string>, string>>>(() =>
    pendingTexts(pendingAtMount.current),
  );
  const committedRef = React.useRef<DraftFields<string>>({ equation: '', units: '', docs: '' });
  committedRef.current = { equation: '', units: variable.units, docs: variable.documentation };
  const alive = React.useRef(true);
  React.useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  React.useEffect(() => fallBackOnFailedPending(pendingAtMount.current, alive, committedRef, setSubmitted), []);
  const unitsDraft = draftText(plainSerialize(unitsContents), submitted.units ?? seeded.units);
  const docsDraft = draftText(plainSerialize(notesContents), submitted.docs ?? seeded.docs);
  // Independent of readOnly (see VariableDetails).
  const hasDraft = unitsDraft !== undefined || docsDraft !== undefined;
  const onDraftStateChange = props.onDraftStateChange;
  React.useEffect(() => {
    onDraftStateChange?.(hasDraft);
  }, [hasDraft, onDraftStateChange]);
  React.useEffect(() => () => onDraftStateChange?.(false), [onDraftStateChange]);

  // True when a draft was submitted.
  const handleUnitDocsSave = (): boolean => {
    if (readOnly || !hasDraft) {
      return false;
    }
    const submission = { units: unitsDraft, docs: docsDraft };
    setSubmitted((prev) => ({
      ...prev,
      ...(submission.units !== undefined ? { units: submission.units } : {}),
      ...(submission.docs !== undefined ? { docs: submission.docs } : {}),
    }));
    const landed = onUnitsDocsChange(variable.ident, submission.units, submission.docs);
    void landed?.then((ok) => {
      if (!ok && alive.current) {
        setSubmitted((prev) => basesAfterFailedSubmission(prev, submission, committedRef.current));
      }
    });
    return true;
  };

  // The flush the host calls before a canvas press (see VariableDetails).
  const saveRef = React.useRef(handleUnitDocsSave);
  saveRef.current = handleUnitDocsSave;
  const registerDraftFlush = props.registerDraftFlush;
  React.useEffect(() => registerDraftFlush?.(() => saveRef.current()), [registerDraftFlush]);

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
          disabled={readOnly}
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
    // The dropdown yields a bare child port; persist the canonical
    // module-qualified `{moduleIdent}·{port}` form the engine wires against.
    const updated = updateReferenceDst(variable.references, index, qualifyDst(variable.ident, newDst));
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
                  {readOnly ? (
                    // Static text instead of dropdowns: the Autocomplete has no
                    // disabled state, and an interactive-looking combobox whose
                    // change silently no-ops is exactly the "editable but
                    // unsavable" trap read-only mode must not present.
                    <>
                      <td className={styles.wiringDropdown}>{ref.src}</td>
                      <td className={styles.wiringDropdown}>{unqualifyDst(ref.dst)}</td>
                      <td></td>
                    </>
                  ) : (
                    <>
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
                          value={unqualifyDst(ref.dst) || null}
                          options={dstOptions}
                          onChange={(_: React.SyntheticEvent | null, newValue: string | null) => {
                            if (newValue) {
                              handleDstChange(i, newValue);
                            }
                          }}
                          renderInput={(params) => (
                            <TextField {...params} variant="standard" placeholder="Select input" />
                          )}
                        />
                      </td>
                      <td>
                        <IconButton size="small" aria-label="Remove reference" onClick={() => handleRemoveReference(i)}>
                          <RemoveIcon />
                        </IconButton>
                      </td>
                    </>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {!readOnly && (
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
        )}
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
            readOnly={readOnly}
            onBlur={readOnly ? undefined : handleUnitDocsSave}
          />
        </Slate>

        <Slate editor={notesEditor} initialValue={notesContents} onChange={handleNotesChange}>
          <Editable
            className={styles.notesEditor}
            placeholder="Documentation"
            spellCheck={false}
            readOnly={readOnly}
            onBlur={readOnly ? undefined : handleUnitDocsSave}
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

        {!readOnly && (
          <div className={styles.cardActions}>
            <Button size="small" color="error" onClick={handleDelete} className={styles.deleteButton}>
              Delete Module
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
