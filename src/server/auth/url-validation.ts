// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

const DANGEROUS_PROTOCOLS = ['javascript:', 'data:', 'vbscript:', 'file:'];

export function validateReturnUrl(returnUrl: string | undefined, baseUrl: string): string {
  if (!returnUrl || returnUrl.trim() === '') {
    return '/';
  }

  const trimmedUrl = returnUrl.trim();

  if (DANGEROUS_PROTOCOLS.some((proto) => trimmedUrl.toLowerCase().startsWith(proto))) {
    return '/';
  }

  if (trimmedUrl.startsWith('//')) {
    return '/';
  }

  if (trimmedUrl.includes('\\')) {
    return '/';
  }

  if (trimmedUrl.startsWith('/') && !trimmedUrl.startsWith('//')) {
    if (!trimmedUrl.includes('\\') && !trimmedUrl.includes('\n') && !trimmedUrl.includes('\r')) {
      return trimmedUrl;
    }
    return '/';
  }

  try {
    const returnUrlObj = new URL(returnUrl);
    const baseUrlObj = new URL(baseUrl);

    if (returnUrlObj.protocol !== baseUrlObj.protocol) {
      return '/';
    }

    if (returnUrlObj.host !== baseUrlObj.host) {
      return '/';
    }

    return returnUrl;
  } catch {
    return '/';
  }
}
