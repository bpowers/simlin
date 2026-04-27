// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

// Pure helper for the project list: when two or more entries share a
// basename, render their full relative paths so the user can tell them
// apart; otherwise render just the basename. Operates on any item shape
// that exposes `path` so the same helper works for `ProjectMeta` and
// for test fixtures with extra fields.

export type Disambiguated<T> = Readonly<{
  item: T;
  label: string;
}>;

export function disambiguatedLabels<T extends { readonly path: string }>(
  items: ReadonlyArray<T>,
): ReadonlyArray<Disambiguated<T>> {
  const counts = new Map<string, number>();
  for (const it of items) {
    const base = basename(it.path);
    counts.set(base, (counts.get(base) ?? 0) + 1);
  }
  return items.map((it) => {
    const base = basename(it.path);
    const ambiguous = (counts.get(base) ?? 0) > 1;
    return { item: it, label: ambiguous ? it.path : base };
  });
}

// Path components on the wire are always forward-slash separated (the
// server's `path_to_forward_slash` normalizes Windows separators), so a
// simple `lastIndexOf('/')` is sufficient — no need to deal with `\`.
function basename(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? path : path.slice(idx + 1);
}
