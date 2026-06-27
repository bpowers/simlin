// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { act, waitFor } from '@testing-library/react';

jest.mock(
  '@simlin/diagram/HostedWebEditor',
  () => ({
    HostedWebEditor: () => React.createElement('div', { 'data-testid': 'hosted-editor' }, 'Editor'),
  }),
  { virtual: true },
);

import '../index-component';

describe('sd-model web component', () => {
  afterEach(() => {
    document.body.innerHTML = '';
    document.head.innerHTML = '';
    jest.restoreAllMocks();
  });

  it('loads its component stylesheet inside the shadow tree', async () => {
    const shadowRoot = document.createElement('div') as unknown as ShadowRoot;
    jest.spyOn(HTMLElement.prototype, 'attachShadow').mockReturnValue(shadowRoot);

    const element = document.createElement('sd-model');
    await act(async () => {
      document.body.appendChild(element);
    });

    await waitFor(() => {
      expect(
        shadowRoot.querySelector('link[href="https://app.simlin.com/static/css/sd-component.css"]'),
      ).not.toBeNull();
    });
  });
});
