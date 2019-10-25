// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

const styles = createStyles({
  status: {
    position: 'absolute',
    top: 0,
    right: 0,
    height: 24,
    width: 24,
    margin: 12,
    marginRight: 16,
  },
});

interface StatusPropsFull extends WithStyles<typeof styles> {
  status: 'ok' | 'error';
}

export type StatusProps = Pick<StatusPropsFull, 'status'>;

export const Status = withStyles(styles)(
  class extends React.PureComponent<StatusPropsFull> {
    constructor(props: StatusPropsFull) {
      super(props);
    }

    render() {
      const { classes } = this.props;
      return (
        <svg className={classes.status}>
          <circle cx={12} cy={12} r={12} fill="#81c784" />
        </svg>
      );
    }
  },
);
