// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Minimal stub for the 'wouter' package. wouter ships as ESM-only which
// CommonJS ts-jest cannot process. Tests that exercise non-routing code
// (e.g. Editor.save()) import React components that transitively load
// wouter, but never actually render the routing components. This stub
// provides just enough of the wouter API surface to satisfy those imports.

import * as React from 'react';

export const Link: React.FC<React.AnchorHTMLAttributes<HTMLAnchorElement> & { to?: string }> = ({
  children,
  to: _to,
  ...rest
}) => React.createElement('a', rest, children);

export const Route: React.FC<{ path?: string; component?: React.ComponentType }> = () => null;

export const Switch: React.FC<{ children?: React.ReactNode }> = ({ children }) =>
  React.createElement(React.Fragment, null, children);

export const Redirect: React.FC<{ to: string }> = () => null;

export function useLocation(): [string, (to: string) => void] {
  return ['/', () => {}];
}

export function useParams(): Record<string, string> {
  return {};
}

export function useRoute(_pattern: string): [boolean, Record<string, string>] {
  return [false, {}];
}
