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
export const Status = React.memo(function Status({ status, onClick }: StatusProps): React.ReactElement {
  const fill = status === 'ok' ? '#81c784' : status === 'error' ? 'rgb(255, 152, 0)' : '#DCDCDC';
  return (
    <svg className={styles.status}>
      <circle cx={12} cy={12} r={12} fill={fill} onClick={onClick} />
    </svg>
  );
});
