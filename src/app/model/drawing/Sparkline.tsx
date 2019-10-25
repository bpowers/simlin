// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { Series } from '../../common';

const styles = createStyles({
  sparkline: {
    strokeWidth: 0.5,
    strokeLinecap: 'round',
    stroke: '#2299dd',
    fill: 'none',
  },
  axis: {
    strokeWidth: 0.75,
    strokeLinecap: 'round',
    stroke: '#999',
    fill: 'none',
  },
});

function last(arr: Float64Array): number {
  return arr[arr.length - 1];
}

function min(arr: Float64Array): number {
  const len = arr.length;
  if (len < 1) {
    return -Infinity;
  }
  let n = arr[0];
  for (let i = 1; i < len; i++) {
    const m = arr[i];
    if (m < n) {
      n = m;
    }
  }
  return n;
}

function max(arr: Float64Array): number {
  const len = arr.length;
  if (len < 1) {
    return -Infinity;
  }
  let n = arr[0];
  for (let i = 1; i < len; i++) {
    const m = arr[i];
    if (m > n) {
      n = m;
    }
  }
  return n;
}

interface SparklineProps extends WithStyles<typeof styles> {
  series: Series;
  width: number;
  height: number;
}

export const Sparkline = withStyles(styles)(
  class extends React.PureComponent<SparklineProps> {
    constructor(props: SparklineProps) {
      super(props);
    }

    render() {
      const { classes } = this.props;
      const { time, values } = this.props.series;
      const x = 0;
      const y = 0;
      const w = this.props.width;
      const h = this.props.height;

      const xMin = time[0];
      const xMax = last(time);
      const xSpan = xMax - xMin;
      const yMin = Math.min(0, min(values)); // 0 or below 0
      const yMax = max(values);
      const ySpan = yMax - yMin || 1;
      let p = '';
      for (let i = 0; i < values.length; i++) {
        if (isNaN(values[i])) {
          console.log('NaN at ' + time[i]);
        }
        p +=
          (i === 0 ? 'M' : 'L') +
          (x + (w * (time[i] - xMin)) / xSpan) +
          ',' +
          (y + h - (h * (values[i] - yMin)) / ySpan);
      }
      const pAxis =
        'M' + x + ',' + (y + h - (h * (0 - yMin)) / ySpan) + 'L' + (x + w) + ',' + (y + h - (h * (0 - yMin)) / ySpan);

      return (
        <g>
          <path d={pAxis} className={classes.axis} />
          <path d={p} className={classes.sparkline} />
        </g>
      );
    }
  },
);
