// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { validateCreateProjectBody, validateUserPatchBody } from '../api-validation';

// Regression coverage for issue #691: after the body-parser 1 -> 2 upgrade,
// `req.body` is `undefined` (not `{}`) for empty-body or wrong-Content-Type
// requests. These pure validators must treat that case as a 400-worthy bad
// request rather than letting a TypeError escape into a generic 500.

describe('validateCreateProjectBody', () => {
  it('rejects an undefined body (empty / wrong Content-Type request)', () => {
    expect(validateCreateProjectBody(undefined)).toBe('projectName is required');
  });

  it('rejects a null body', () => {
    expect(validateCreateProjectBody(null)).toBe('projectName is required');
  });

  it('rejects an empty object', () => {
    expect(validateCreateProjectBody({})).toBe('projectName is required');
  });

  it('rejects a body with a falsy projectName', () => {
    expect(validateCreateProjectBody({ projectName: '' })).toBe('projectName is required');
  });

  it('rejects a non-object body', () => {
    expect(validateCreateProjectBody('projectName=foo')).toBe('projectName is required');
  });

  it('accepts a body with a projectName', () => {
    expect(validateCreateProjectBody({ projectName: 'My Model' })).toBeUndefined();
  });

  it('accepts a body with projectName plus extra optional fields', () => {
    expect(
      validateCreateProjectBody({ projectName: 'My Model', description: 'd', isPublic: true, projectPB: 'AA==' }),
    ).toBeUndefined();
  });
});

describe('validateUserPatchBody', () => {
  it('rejects an undefined body (empty / wrong Content-Type request)', () => {
    expect(validateUserPatchBody(undefined)).toBe('only username can be patched');
  });

  it('rejects a null body', () => {
    expect(validateUserPatchBody(null)).toBe('only username can be patched');
  });

  it('rejects an empty object', () => {
    expect(validateUserPatchBody({})).toBe('only username can be patched');
  });

  it('rejects a body with the wrong number of keys', () => {
    expect(validateUserPatchBody({ username: 'alice' })).toBe('only username can be patched');
    expect(validateUserPatchBody({ username: 'alice', agreeToTermsAndPrivacyPolicy: true, extra: 1 })).toBe(
      'only username can be patched',
    );
  });

  it('rejects two keys when username is falsy', () => {
    expect(validateUserPatchBody({ username: '', agreeToTermsAndPrivacyPolicy: true })).toBe(
      'only username can be patched',
    );
  });

  it('accepts exactly { username, agreeToTermsAndPrivacyPolicy }', () => {
    expect(validateUserPatchBody({ username: 'alice', agreeToTermsAndPrivacyPolicy: true })).toBeUndefined();
  });
});
