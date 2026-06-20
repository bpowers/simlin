// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import styles from './Status.module.css';

interface StatusProps {
  status: 'ok' | 'error' | 'disabled';
  onClick: () => void;
}

// Memoized: the Editor re-renders on every pan/momentum frame (optimistic view
// updates replace the controller snapshot wholesale), but Status's props are
// stable during a pan. React.memo restores the old PureComponent shallow-prop
// bailout so it does not re-render every frame. The Editor's `onClick` is a
// stable bound handler, so the memo holds.
// Mirror the theme status tokens (theme.css): success green for a healthy
// model, error RED -- not the warning orange the dot previously used, which
// read a hard model error as a soft advisory -- and a medium grey (rather than
// the near-white #DCDCDC, which barely showed on the white search bar) when the
// model is not simulatable. Inlined rather than via var() because this fills an
// SVG presentation attribute; the dot only ever renders on the light search bar.
export const Status = React.memo(function Status({ status, onClick }: StatusProps): React.ReactElement {
  const fill = status === 'ok' ? '#2e7d32' : status === 'error' ? '#c62828' : '#bdbdbd';
  return (
    <svg className={styles.status}>
      <circle cx={12} cy={12} r={12} fill={fill} onClick={onClick} />
    </svg>
  );
});
