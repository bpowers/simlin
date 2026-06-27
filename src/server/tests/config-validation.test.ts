// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { validateRuntimeConfig } from '../config-validation';

describe('validateRuntimeConfig', () => {
  it('allows the default development session-key placeholder', () => {
    expect(() =>
      validateRuntimeConfig('development', {
        seshcookie: {
          key: '',
        },
      }),
    ).not.toThrow();
  });

  it.each(['', '   ', 'IN ENV'])('rejects production session key %p', (key) => {
    expect(() =>
      validateRuntimeConfig('production', {
        seshcookie: {
          key,
        },
      }),
    ).toThrow('production authentication.seshcookie.key');
  });

  it('allows a production session key supplied from the environment', () => {
    expect(() =>
      validateRuntimeConfig('production', {
        seshcookie: {
          key: 'a real deployment secret',
        },
      }),
    ).not.toThrow();
  });
});
