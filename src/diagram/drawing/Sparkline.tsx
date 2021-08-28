// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';

import { styled } from '@material-ui/core/styles';

import { brewer } from 'chroma-js';

import { defined, Series } from '@system-dynamics/core/common';

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

export interface SparklineProps {
  series: Readonly<Array<Series>>;
  width: number;
  height: number;
}

export const Sparkline = styled(
  class Sparkline extends React.PureComponent<SparklineProps & { className?: string }> {
    // these should all be 'private', but Typescript can't enforce that with the `styled` above
    pAxis = '';
    sparklines: Array<React.SVGProps<SVGPathElement>> = [];
    cachedSeries: List<Series> | unknown;

    recache() {
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
        sparklines.push(<path key={dataset.name} d={p} className="simlin-sparkline" style={style} />);
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

      const { className } = this.props;
      return (
        <g className={className}>
          <path key="$axis" d={this.pAxis} className="simlin-sparkline-axis" />
          {this.sparklines}
        </g>
      );
    }
  },
)(`
    & .simlin-sparkline {
      stroke-width: 0.5px;
      stroke-linecap: round;
      stroke: #2299dd;
      fill: none;
    }
    & .simlin-sparkline-axis {
      stroke-width: 0.75px;
      stroke-linecap: round;
      stroke: #999;
      fill: none;
    }
`);
