// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { Link } from 'wouter';
import Button from './components/Button';
import IconButton from './components/IconButton';
import Drawer from './components/Drawer';
import TextField from './components/TextField';
import { ArrowBackIcon, ClearIcon, CloudDownloadIcon } from './components/icons';

import { DeleteProjectButton } from './DeleteProjectButton';
import { ModelIcon } from './ModelIcon';
import { formatSimSpecValue, resolveSimSpecDraft, type SimSpecField } from './sim-spec-draft';

import styles from './ModelPropertiesDrawer.module.css';

interface ModelPropertiesDrawerProps {
  modelName: string;
  open: boolean;
  onDrawerToggle: (isOpen: boolean) => void;
  startTime: number;
  stopTime: number;
  dt: number;
  timeUnits: string;
  // Fired once when a sim-specs field settles (blur/Enter) with a value that
  // actually changed and passed validation -- NOT per keystroke. Each call is
  // one engine patch / one undo entry. Replaces the old per-`onChange`
  // handlers, which recorded an undo entry and scheduled a save on every
  // character typed (issue #55).
  onSimSpecCommit: (field: SimSpecField, value: number | string) => void;
  onDownloadXmile: () => void;
  // When provided, a destructive "Delete project" action is shown. Hosts that
  // can't (or shouldn't) delete -- read-only viewers, embeds, the local
  // file-backed viewer -- simply leave this undefined.
  onDelete?: () => Promise<void>;
  // Read-only viewers (issue #935) still see the sim specs and can download
  // the model, but the fields are disabled: sim specs are project content and
  // a draft that silently never commits would misrepresent editability.
  readOnly?: boolean;
  // The "Exit" link navigates to "/" -- the project list in the app and in
  // simlin-serve. Defaults to shown; a host that embeds the Editor in a page it
  // owns (a notebook cell) has no such route and hides it, which also means no
  // router is needed to mount the drawer.
  showHomeLink?: boolean;
}

interface SimSpecDraftFieldProps {
  field: SimSpecField;
  label: string;
  value: number | string;
  type?: string;
  onCommit: (field: SimSpecField, value: number | string) => void;
  disabled?: boolean;
}

// A single sim-specs field: controlled from `value` (the model) when idle, but
// holding a local draft string while focused so typing does not touch the
// model. It commits ONCE on settle (blur/Enter, via the pure
// `resolveSimSpecDraft`), reverts on Escape, and never clobbers an in-progress
// draft when new props arrive mid-edit. Keeping the draft state per-field (one
// component instance each) makes those semantics fall out structurally: an
// engine refresh only releases the drafts of fields that are not focused.
function SimSpecDraftField(props: SimSpecDraftFieldProps): React.ReactElement {
  const { field, label, value, type, onCommit, disabled } = props;

  // `undefined` means "not editing -- show the model value". A string means an
  // edit is in flight (or a just-committed value awaiting the model catching up).
  const [draft, setDraft] = React.useState<string | undefined>(undefined);
  const focusedRef = React.useRef(false);
  // Set synchronously by Escape so the blur it triggers skips the commit. A ref
  // (not state) because the blur handler runs in the same tick, before a state
  // update would be visible.
  const skipNextCommitRef = React.useRef(false);
  // The exact draft text of the most recent commit, held until the model prop
  // catches up. It makes commit idempotent under prop lag: a second blur of the
  // same retained draft (e.g. focus moving to a sibling field re-blurs this
  // one) must not fire a duplicate patch/undo entry, because `value` is still
  // the stale pre-commit model value the decision compares against. Cleared on
  // any fresh keystroke and when the draft releases.
  const committedDraftRef = React.useRef<string | undefined>(undefined);

  // While unfocused, the model value is the source of truth: a new prop (engine
  // refresh, undo, external edit) releases any lingering draft so the field
  // shows the model. Guarded on focus so a refresh mid-edit never clobbers what
  // the user is typing. This also clears the "committed, awaiting model" draft
  // once the model reflects the commit -- so a valid commit shows the typed
  // value continuously, with no flash of the pre-edit value.
  React.useEffect(() => {
    if (!focusedRef.current) {
      setDraft(undefined);
      committedDraftRef.current = undefined;
    }
  }, [value]);

  const display = draft ?? formatSimSpecValue(value);

  const commit = (): void => {
    // A disabled field never delivers change events through React, so `draft`
    // stays undefined and the check below already suffices; the explicit gate
    // is defense in depth against a synthesized event slipping a draft in.
    if (disabled) {
      return;
    }
    // `draft` is read from the current render's closure. Every keystroke
    // re-renders (onChange -> setDraft), so by the time a blur/Enter fires this
    // closure has the latest text.
    if (draft === undefined || committedDraftRef.current === draft) {
      return;
    }
    const decision = resolveSimSpecDraft(field, draft, value);
    if (decision.shouldCommit && decision.value !== undefined) {
      committedDraftRef.current = draft;
      onCommit(field, decision.value);
      // Keep the committed text visible; the [value] effect releases the draft
      // when the model prop catches up.
    } else {
      // Unchanged / invalid / empty: discard the draft and show the model.
      setDraft(undefined);
    }
  };

  return (
    <TextField
      label={label}
      value={display}
      type={type}
      margin="normal"
      fullWidth
      onChange={(e) => {
        // A fresh keystroke supersedes any retained committed text.
        committedDraftRef.current = undefined;
        setDraft(e.target.value);
      }}
      inputProps={{
        disabled,
        onFocus: () => {
          focusedRef.current = true;
          // Seed from the current display so refocusing before the model
          // catches up preserves a just-committed value.
          setDraft((d) => d ?? formatSimSpecValue(value));
        },
        onBlur: () => {
          focusedRef.current = false;
          if (skipNextCommitRef.current) {
            skipNextCommitRef.current = false;
            setDraft(undefined);
            return;
          }
          commit();
        },
        onKeyDown: (e) => {
          if (e.key === 'Enter') {
            // Blur drives the single commit (avoids a double commit from
            // committing here and again on the ensuing blur).
            e.preventDefault();
            e.currentTarget.blur();
          } else if (e.key === 'Escape') {
            e.preventDefault();
            // Escape here means "revert this field", not "dismiss the drawer":
            // stop propagation so the Drawer's own document-level Escape-close
            // listener doesn't also fire and yank the whole panel away.
            e.stopPropagation();
            // Discard the draft (revert to the model) and skip the commit the
            // ensuing blur would otherwise run against the pre-Escape draft.
            skipNextCommitRef.current = true;
            setDraft(undefined);
            e.currentTarget.blur();
          }
        },
      }}
    />
  );
}

export function ModelPropertiesDrawer(props: ModelPropertiesDrawerProps): React.ReactElement {
  const {
    modelName,
    open,
    onDrawerToggle,
    startTime,
    stopTime,
    dt,
    timeUnits,
    onSimSpecCommit,
    onDownloadXmile,
    onDelete,
    readOnly,
    showHomeLink = true,
  } = props;

  const handleOpen = (): void => {
    onDrawerToggle(true);
  };

  const handleClose = (): void => {
    onDrawerToggle(false);
  };

  return (
    <Drawer open={open} onOpen={handleOpen} onClose={handleClose}>
      <div className={styles.content}>
        <div>
          <div className={styles.modelApp}>
            <div className={styles.imageWrap}>
              <ModelIcon className={styles.modelIcon} />
            </div>
            <div className={styles.modelName}>Simlin</div>
          </div>
          {/* asChild: the Link injects href/onClick into the IconButton, whose
              href mode renders a single <a> styled as an icon button. The
              previous <a><button/></a> nesting was invalid interactive
              content (and double-announced to assistive tech). */}
          {showHomeLink ? (
            <Link to="/" asChild>
              <IconButton className={styles.menuButton} color="inherit" aria-label="Exit">
                <ArrowBackIcon />
              </IconButton>
            </Link>
          ) : null}
          <IconButton className={styles.closeButton} color="inherit" aria-label="Close" onClick={handleClose}>
            <ClearIcon />
          </IconButton>
        </div>

        <div className={styles.propsForm}>
          <h2>{modelName}</h2>
          <SimSpecDraftField
            field="startTime"
            label="Start Time"
            value={startTime}
            type="number"
            onCommit={onSimSpecCommit}
            disabled={readOnly}
          />
          <SimSpecDraftField
            field="stopTime"
            label="Stop Time"
            value={stopTime}
            type="number"
            onCommit={onSimSpecCommit}
            disabled={readOnly}
          />
          <SimSpecDraftField
            field="dt"
            label="dt"
            value={dt}
            type="number"
            onCommit={onSimSpecCommit}
            disabled={readOnly}
          />
          <SimSpecDraftField
            field="timeUnits"
            label="Time Units"
            value={timeUnits}
            onCommit={onSimSpecCommit}
            disabled={readOnly}
          />
          <br />
          <br />
          <Button
            className={styles.downloadButton}
            variant="contained"
            color="primary"
            size="large"
            startIcon={<CloudDownloadIcon />}
            onClick={onDownloadXmile}
          >
            Download model
          </Button>
          {onDelete ? <DeleteProjectButton projectName={modelName} onDelete={onDelete} /> : null}
        </div>
      </div>
    </Drawer>
  );
}
