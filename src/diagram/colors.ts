// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// ColorBrewer Dark2 palette - 8 qualitative colors
// Source: https://colorbrewer2.org/ (Cynthia Brewer, Mark Harrower, Penn State)
// License: Apache 2.0 (compatible with this project)
export const Dark2 = [
  '#1b9e77', // teal
  '#d95f02', // orange
  '#7570b3', // purple
  '#e7298a', // pink
  '#66a61e', // green
  '#e6ab02', // yellow
  '#a6761d', // brown
  '#666666', // gray
] as const;

export type Dark2Color = (typeof Dark2)[number];
