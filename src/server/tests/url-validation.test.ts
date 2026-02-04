// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { validateReturnUrl } from '../auth/url-validation';

describe('validateReturnUrl', () => {
  const baseUrl = 'https://app.simlin.com';

  describe('valid URLs', () => {
    it('should accept relative paths starting with /', () => {
      expect(validateReturnUrl('/', baseUrl)).toBe('/');
      expect(validateReturnUrl('/home', baseUrl)).toBe('/home');
    });

    it('should accept /projects/user/name', () => {
      expect(validateReturnUrl('/projects/user/name', baseUrl)).toBe('/projects/user/name');
    });

    it('should accept same-origin absolute URLs', () => {
      expect(validateReturnUrl('https://app.simlin.com/projects', baseUrl)).toBe('https://app.simlin.com/projects');
    });

    it('should handle URLs with query strings', () => {
      expect(validateReturnUrl('/search?q=test', baseUrl)).toBe('/search?q=test');
      expect(validateReturnUrl('https://app.simlin.com/search?q=test', baseUrl)).toBe(
        'https://app.simlin.com/search?q=test',
      );
    });

    it('should handle URLs with fragments', () => {
      expect(validateReturnUrl('/page#section', baseUrl)).toBe('/page#section');
    });
  });

  describe('invalid URLs', () => {
    it('should reject external URLs', () => {
      expect(validateReturnUrl('https://evil.com/steal', baseUrl)).toBe('/');
      expect(validateReturnUrl('https://app.simlin.com.evil.com/steal', baseUrl)).toBe('/');
    });

    it('should reject javascript: URLs', () => {
      expect(validateReturnUrl('javascript:alert(1)', baseUrl)).toBe('/');
    });

    it('should reject data: URLs', () => {
      expect(validateReturnUrl('data:text/html,<script>alert(1)</script>', baseUrl)).toBe('/');
    });

    it('should reject vbscript: URLs', () => {
      expect(validateReturnUrl('vbscript:msgbox(1)', baseUrl)).toBe('/');
    });

    it('should reject protocol-relative URLs (//evil.com)', () => {
      expect(validateReturnUrl('//evil.com/steal', baseUrl)).toBe('/');
    });

    it('should reject URLs with different port', () => {
      expect(validateReturnUrl('https://app.simlin.com:8080/page', baseUrl)).toBe('/');
    });
  });

  describe('edge cases', () => {
    it('should return / for undefined', () => {
      expect(validateReturnUrl(undefined, baseUrl)).toBe('/');
    });

    it('should return / for empty string', () => {
      expect(validateReturnUrl('', baseUrl)).toBe('/');
    });

    it('should return / for invalid URL', () => {
      expect(validateReturnUrl('not a url at all', baseUrl)).toBe('/');
    });

    it('should handle URL encoding', () => {
      expect(validateReturnUrl('/projects/user%20name/model', baseUrl)).toBe('/projects/user%20name/model');
    });

    it('should handle backslash tricks', () => {
      expect(validateReturnUrl('/\\evil.com', baseUrl)).toBe('/');
      expect(validateReturnUrl('https://app.simlin.com\\@evil.com', baseUrl)).toBe('/');
    });
  });
});
