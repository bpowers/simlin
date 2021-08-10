// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export interface Project {
  id: string;
  displayName: string;
  description: string;
  tags: string[];
  file: string;
}
