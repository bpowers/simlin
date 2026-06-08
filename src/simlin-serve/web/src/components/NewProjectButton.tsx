// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

import * as React from 'react';

import { createProject } from '../api';
import type { CreateProjectFormat } from '../api';

type NewProjectButtonProps = Readonly<{
  // Invoked with the server-returned relative path on a successful
  // create. The caller updates its selectedPath state to navigate the
  // editor; the WS `projectChanged` event refreshes the project list
  // naturally.
  onCreated: (path: string) => void;
  // Optional forward-slash relative parent directory passed through to
  // the server. When omitted, the file lands at the scan root.
  parentDir?: string;
}>;

type NewProjectButtonState = {
  open: boolean;
  name: string;
  format: CreateProjectFormat;
  pending: boolean;
  // Server-side or client-side validation error to surface inline.
  // null when the form is valid and no request is in flight.
  error: string | null;
};

const INITIAL_STATE: NewProjectButtonState = {
  open: false,
  name: '',
  format: 'stmx',
  pending: false,
  error: null,
};

// Length cap mirrors the server's MAX_NEW_PROJECT_NAME_LEN. Capping
// client-side gives quicker feedback and avoids a round-trip for
// obviously-too-long names.
const MAX_NAME_LEN = 64;
const VALID_NAME_PATTERN = /^[A-Za-z0-9_-]+$/;

// Pure helper extracted so future additions (e.g. localised messages)
// have one place to plug into. Returns null when the name passes our
// client-side checks; the server is still authoritative.
function clientNameError(raw: string): string | null {
  if (raw.length === 0) {
    return null;
  }
  if (raw.length > MAX_NAME_LEN) {
    return `name must be ${MAX_NAME_LEN} characters or fewer`;
  }
  if (raw.startsWith('.')) {
    return 'name may not start with a dot';
  }
  if (!VALID_NAME_PATTERN.test(raw)) {
    return 'name may contain only letters, digits, `_`, and `-`';
  }
  return null;
}

// The escaped async continuation (handleCreate) reads CURRENT state/props
// through this ref, exactly as the class read this.state / this.props at call
// time rather than as captured by a stale render closure.
interface NewProjectButtonLatest {
  state: NewProjectButtonState;
  props: NewProjectButtonProps;
}

export function NewProjectButton(props: NewProjectButtonProps): React.ReactElement {
  const [state, setStateRaw] = React.useState<NewProjectButtonState>(INITIAL_STATE);

  // Class-parity setState: merges a partial patch onto the previous state,
  // exactly like React.Component's setState.
  const setState = React.useCallback((patch: Partial<NewProjectButtonState>): void => {
    setStateRaw((prev) => ({ ...prev, ...patch }));
  }, []);

  // Refreshed synchronously every render so the escaped async create handler
  // reads current state/props (the class read this.state / this.props, which
  // were always current).
  const latest = React.useRef<NewProjectButtonLatest>(undefined as unknown as NewProjectButtonLatest);
  latest.current = { state, props };

  const handleOpen = React.useCallback((): void => {
    setState({ open: true });
  }, [setState]);

  const handleCancel = React.useCallback((): void => {
    // Reset to the full initial state (not a merge) — Cancel collapses the
    // form and clears the name/format/error all at once, matching the
    // class's setState(INITIAL_STATE).
    setStateRaw(INITIAL_STATE);
  }, []);

  const handleNameChange = React.useCallback(
    (event: React.ChangeEvent<HTMLInputElement>): void => {
      const name = event.target.value;
      // Clear server-side errors on edit so the user sees their own
      // edits taking effect; client-side errors recompute from `name`
      // in render so we don't need to clear them explicitly.
      setState({ name, error: null });
    },
    [setState],
  );

  const handleFormatChange = React.useCallback(
    (event: React.ChangeEvent<HTMLSelectElement>): void => {
      const next = event.target.value as CreateProjectFormat;
      setState({ format: next });
    },
    [setState],
  );

  const handleCreate = React.useCallback(async (): Promise<void> => {
    const { name, format } = latest.current.state;
    const trimmed = name.trim();
    const clientErr = clientNameError(trimmed);
    if (clientErr || trimmed.length === 0) {
      setState({ error: clientErr ?? 'name is required' });
      return;
    }

    setState({ pending: true, error: null });
    try {
      const response = await createProject(trimmed, format, latest.current.props.parentDir);
      setStateRaw(INITIAL_STATE);
      latest.current.props.onCreated(response.path);
    } catch (err) {
      const message = err instanceof Error ? err.message : 'failed to create model';
      setState({ pending: false, error: message });
    }
    // Deps are just [setState] (stable): name/format/parentDir/onCreated are
    // read through `latest`, so they need not be deps. (The repo lint config
    // does not enable react-hooks/exhaustive-deps, so no disable directive is
    // needed.)
  }, [setState]);

  const handleKeyDown = React.useCallback(
    (event: React.KeyboardEvent<HTMLInputElement>): void => {
      // Enter submits, Escape cancels — common form ergonomics that don't
      // require introducing a real <form> element (which would otherwise
      // trigger a full-page submit if the launcher script-injected a host
      // form by mistake).
      if (event.key === 'Enter') {
        event.preventDefault();
        void handleCreate();
      } else if (event.key === 'Escape') {
        event.preventDefault();
        handleCancel();
      }
    },
    [handleCreate, handleCancel],
  );

  const { open, name, format, pending, error } = state;

  if (!open) {
    return (
      <div className="serve-new-project">
        <button type="button" className="serve-new-project-trigger" onClick={handleOpen} aria-label="Create new model">
          + New model
        </button>
      </div>
    );
  }

  const trimmed = name.trim();
  const clientErr = clientNameError(trimmed);
  const canCreate = !pending && trimmed.length > 0 && clientErr === null;
  const visibleError = error ?? clientErr;

  return (
    <div className="serve-new-project serve-new-project--open">
      <input
        type="text"
        className="serve-new-project-input"
        placeholder="filename"
        aria-label="filename"
        value={name}
        onChange={handleNameChange}
        onKeyDown={handleKeyDown}
        maxLength={MAX_NAME_LEN}
        disabled={pending}
        autoFocus
      />
      <label className="serve-new-project-format">
        <span className="serve-visually-hidden">format</span>
        <select aria-label="format" value={format} onChange={handleFormatChange} disabled={pending}>
          <option value="stmx">XMILE (.stmx)</option>
          <option value="sd_json">Simlin JSON (.sd.json)</option>
        </select>
      </label>
      <button
        type="button"
        className="serve-new-project-create"
        onClick={() => void handleCreate()}
        disabled={!canCreate}
      >
        Create
      </button>
      <button
        type="button"
        className="serve-new-project-cancel"
        onClick={handleCancel}
        aria-label="Cancel"
        disabled={pending}
      >
        Cancel
      </button>
      {visibleError ? (
        <div role="alert" className="serve-new-project-error">
          {visibleError}
        </div>
      ) : null}
    </div>
  );
}
