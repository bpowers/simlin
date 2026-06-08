// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Mock the heavy upstream modules before importing NewProject so the import
// graph doesn't pull in the WASM engine, datamodel, or the diagram component
// library at test time. We exercise just the upload/leak behavior of
// NewProject.uploadModel.

const dispose = jest.fn().mockResolvedValue(undefined);
const serializeProtobuf = jest.fn();
const serializeJson = jest.fn();
const open = jest.fn();
const openVensim = jest.fn();

jest.mock(
  '@simlin/engine',
  () => ({
    Project: {
      open: (...args: unknown[]) => open(...args),
      openVensim: (...args: unknown[]) => openVensim(...args),
    },
  }),
  { virtual: true },
);

// projectFromJson is called by uploadModel after serializeJson resolves; we
// stub it so we can assert dispose runs even on the happy path without
// pulling in the real datamodel parsing.
const projectFromJson = jest.fn();
jest.mock(
  '@simlin/core/datamodel',
  () => ({
    projectFromJson: (...args: unknown[]) => projectFromJson(...args),
  }),
  { virtual: true },
);

// The diagram package re-exports a large component library plus CSS modules
// neither of which we exercise here.
jest.mock(
  '@simlin/diagram',
  () => {
    const React = require('react');
    // eslint-disable-next-line react/display-name
    const passthrough = (name: string) => (props: { children?: React.ReactNode }) =>
      React.createElement('div', { 'data-component': name }, props.children);
    return {
      Accordion: passthrough('Accordion'),
      AccordionDetails: passthrough('AccordionDetails'),
      AccordionSummary: passthrough('AccordionSummary'),
      Button: passthrough('Button'),
      Checkbox: passthrough('Checkbox'),
      FormControlLabel: passthrough('FormControlLabel'),
      InputAdornment: passthrough('InputAdornment'),
      TextField: passthrough('TextField'),
      ExpandMoreIcon: passthrough('ExpandMoreIcon'),
    };
  },
  { virtual: true },
);

import * as React from 'react';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react';

import { NewProject } from '../NewProject';
import type { User } from '../User';

function makeFakeFile(name: string, contents: string): File {
  // jsdom's File implementation works fine for our purposes.
  return new File([contents], name, { type: 'text/plain' });
}

afterEach(() => {
  cleanup();
});

describe('NewProject.uploadModel', () => {
  beforeEach(() => {
    dispose.mockClear();
    serializeProtobuf.mockReset().mockResolvedValue(new Uint8Array([1, 2, 3]));
    serializeJson.mockReset().mockResolvedValue('{}');
    open.mockReset().mockResolvedValue({
      serializeProtobuf,
      serializeJson,
      dispose,
    });
    openVensim.mockReset().mockResolvedValue({
      serializeProtobuf,
      serializeJson,
      dispose,
    });
    projectFromJson.mockReset().mockReturnValue({
      models: new Map([['main', { views: [{ id: 'view1' }] }]]),
    });
  });

  // Render a real NewProject and drive its hidden file <input> with the given
  // file, awaiting the async uploadModel handler to completion. NewProject is
  // a function component, so we exercise uploadModel through its observable
  // surface (the file input's onChange) rather than reaching into an instance.
  async function uploadFile(file: File): Promise<void> {
    const fakeUser = { id: 'tester', email: 't@example.com', displayName: 'tester' } as unknown as User;
    const onProjectCreated = jest.fn();
    const { container } = render(<NewProject user={fakeUser} onProjectCreated={onProjectCreated} />);

    const input = container.querySelector('#xmile-model-file') as HTMLInputElement;
    expect(input).not.toBeNull();
    // Drive React's onChange via fireEvent (it wraps the dispatch in act and
    // routes through React's synthetic event system) with a target.files list
    // uploadModel reads. The async uploadModel chain (readFile -> engine open
    // -> serialize -> dispose) settles via the dispose assertion's waitFor.
    fireEvent.change(input, { target: { files: [file] } });
    await waitFor(() => {
      expect(dispose).toHaveBeenCalled();
    });
  }

  test('disposes the engine project after a successful XMILE upload', async () => {
    await uploadFile(makeFakeFile('model.xmile', '<xmile/>'));

    expect(open).toHaveBeenCalledTimes(1);
    expect(dispose).toHaveBeenCalledTimes(1);
  });

  test('disposes the engine project after a successful Vensim upload', async () => {
    await uploadFile(makeFakeFile('model.mdl', 'vensim contents'));

    expect(openVensim).toHaveBeenCalledTimes(1);
    expect(dispose).toHaveBeenCalledTimes(1);
  });

  test('disposes the engine project even when serializeProtobuf rejects', async () => {
    serializeProtobuf.mockRejectedValueOnce(new Error('boom'));

    await uploadFile(makeFakeFile('model.xmile', '<xmile/>'));

    // Dispose must run regardless of the inner failure to avoid leaking the
    // WASM handle. The error is surfaced via setErrorMsg; the dispose
    // discipline is the load-bearing behavior under test here.
    expect(open).toHaveBeenCalledTimes(1);
    expect(dispose).toHaveBeenCalledTimes(1);
  });

  test('disposes the engine project when projectFromJson throws', async () => {
    projectFromJson.mockImplementationOnce(() => {
      throw new Error('bad json');
    });

    await uploadFile(makeFakeFile('model.xmile', '<xmile/>'));

    expect(dispose).toHaveBeenCalledTimes(1);
  });
});
