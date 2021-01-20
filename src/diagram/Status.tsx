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
  status: 'ok' | 'error' | 'disabled';
  onClick: () => void;
}

export type StatusProps = Pick<StatusPropsFull, 'status'>;

export const Status = withStyles(styles)(
  class Status extends React.PureComponent<StatusPropsFull> {
    handleClick = () => {
      this.props.onClick();
    };

    render() {
      const { classes, status } = this.props;
      const fill = status === 'ok' ? '#81c784' : status === 'error' ? 'rgb(255, 152, 0)' : '#DCDCDC';
      return (
        <svg className={classes.status}>
          <circle cx={12} cy={12} r={12} fill={fill} onClick={this.handleClick} />
        </svg>
      );
    }
  },
);
