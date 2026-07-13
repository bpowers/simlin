// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { describe, it, expect } from '@rstest/core';

import { validateCreateProjectBody, validateSaveProjectBody, validateUserPatchBody } from '../api-validation';

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

describe('validateSaveProjectBody', () => {
  it('rejects an undefined body (empty / wrong Content-Type request)', () => {
    expect(validateSaveProjectBody(undefined)).toBe('currVersion is required');
  });

  it('rejects a null body', () => {
    expect(validateSaveProjectBody(null)).toBe('currVersion is required');
  });

  it('rejects an empty object', () => {
    expect(validateSaveProjectBody({})).toBe('currVersion is required');
  });

  it('rejects a non-numeric currVersion', () => {
    expect(validateSaveProjectBody({ currVersion: '1', projectPB: 'AA==' })).toBe('currVersion must be an integer');
    expect(validateSaveProjectBody({ currVersion: null, projectPB: 'AA==' })).toBe('currVersion must be an integer');
  });

  it('rejects a non-integer currVersion (the token the server increments by 1)', () => {
    expect(validateSaveProjectBody({ currVersion: 1.5, projectPB: 'AA==' })).toBe('currVersion must be an integer');
    expect(validateSaveProjectBody({ currVersion: NaN, projectPB: 'AA==' })).toBe('currVersion must be an integer');
  });

  it('accepts currVersion 0: legacy rows predate version seeding, so 0 is legitimate', () => {
    expect(validateSaveProjectBody({ currVersion: 0, projectPB: 'AA==' })).toBeUndefined();
  });

  it('rejects a missing, empty, or non-string projectPB', () => {
    expect(validateSaveProjectBody({ currVersion: 1 })).toBe('projectPB is required');
    expect(validateSaveProjectBody({ currVersion: 1, projectPB: '' })).toBe('projectPB is required');
    expect(validateSaveProjectBody({ currVersion: 1, projectPB: 42 })).toBe('projectPB is required');
  });

  it('accepts an integer currVersion plus base64 projectPB', () => {
    expect(validateSaveProjectBody({ currVersion: 3, projectPB: 'AA==' })).toBeUndefined();
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
