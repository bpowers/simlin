// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

export const canonicalize = (id: string): string => {
  let quoted = false;
  if (id.length > 1) {
    const f = id.slice(0, 1);
    const l = id.slice(id.length - 1);
    quoted = f === '"' && l === '"';
  }
  id = id.toLowerCase();
  id = id.replace(/\\n/g, '_');
  id = id.replace(/\\\\/g, '\\');
  id = id.replace(/\\"/g, '\\');
  id = id.replace(/[_\r\n\t \xa0]+/g, '_');
  if (quoted) {
    return id.slice(1, -1);
  }
  return id;
};
