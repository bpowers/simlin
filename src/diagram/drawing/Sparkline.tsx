// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { defined, Series } from '@simlin/core/common';
import { Dark2 } from '../colors';

import styles from './Sparkline.module.css';

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

export class Sparkline extends React.PureComponent<SparklineProps> {
  // these should all be 'private', but Typescript can't enforce that with the `styled` above
  pAxis = '';
  sparklines: Array<React.ReactNode> = [];
  cachedSeries: Readonly<Array<Series>> | unknown;

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

    const colors = Dark2;
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
      sparklines.push(
        <path key={dataset.name} d={p} className={`${styles.sparkline} simlin-sparkline-line`} style={style} />,
      );
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

    return (
      <g>
        <>
          <path key="$axis" d={this.pAxis} className={`${styles.axis} simlin-sparkline-axis`} />
          {this.sparklines}
        </>
      </g>
    );
  }
}
