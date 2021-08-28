// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';

import { createStyles, withStyles, WithStyles } from '@material-ui/styles';

import { brewer } from 'chroma-js';

import { defined, Series } from '@system-dynamics/core/common';

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

function last(arr: Readonly<Float64Array>): number {
  return arr[arr.length - 1];
}

function min(arr: Readonly<Float64Array>): number {
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

function max(arr: Readonly<Float64Array>): number {
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
  series: Readonly<Array<Series>>;
  width: number;
  height: number;
}

export const Sparkline = withStyles(styles)(
  class Sparkline extends React.PureComponent<SparklineProps> {
    private pAxis = '';
    private sparklines: Array<React.SVGProps<SVGPathElement>> = [];
    private cachedSeries: List<Series> | unknown;

    recache() {
      const { classes } = this.props;
      const time = defined(this.props.series[0]).time;
      const x = 0;
      const y = 0;
      const w = this.props.width;
      const h = this.props.height;

      const xMin = time[0];
      const xMax = last(time);
      const xSpan = xMax - xMin;

      let yMin = 0;
      let yMax = -Infinity;
      // first build up the min + max across all datasets
      for (const dataset of this.props.series) {
        const values = dataset.values;
        yMin = Math.min(yMin, min(values)); // 0 or below 0
        yMax = Math.max(yMax, max(values));
      }
      const ySpan = yMax - yMin || 1;

      const colors = brewer.Dark2;
      const sparklines = [];
      let i = 0;
      for (const dataset of this.props.series) {
        const values = dataset.values;
        let p = '';
        for (let i = 0; i < values.length; i++) {
          if (isNaN(values[i])) {
            // console.log(`NaN at ${time[i]}`);
            continue;
          }
          const prefix = i === 0 ? 'M' : 'L';
          p += `${prefix}${x + (w * (time[i] - xMin)) / xSpan},${y + h - (h * (values[i] - yMin)) / ySpan}`;
        }
        const style = this.props.series.length === 1 ? undefined : { stroke: colors[i % colors.length] };
        sparklines.push(<path key={dataset.name} d={p} className={classes.sparkline} style={style} />);
        i++;
      }

      this.pAxis = `M${x},${y + h - (h * (0 - yMin)) / ySpan}L${x + w},${y + h - (h * (0 - yMin)) / ySpan}`;
      this.sparklines = sparklines;
      this.cachedSeries = this.props.series;
    }

    render() {
      if (this.props.series !== this.cachedSeries) {
        this.recache();
      }

      const { classes } = this.props;
      return (
        <g>
          <path key="$axis" d={this.pAxis} className={classes.axis} />
          {this.sparklines}
        </g>
      );
    }
  },
);
