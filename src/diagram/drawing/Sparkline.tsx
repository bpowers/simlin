// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { defined, Series } from '@simlin/core/common';
import { Dark2 } from '../colors';
import { jsFormatNumber as f } from '../render-common';

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

export const Sparkline = React.memo(function Sparkline(props: SparklineProps): React.ReactElement {
  const { series, width, height } = props;

  // Path construction walks every data point of every series, so memoize it;
  // this replaces the hand-rolled `recache()`/`cachedSeries` instance-field
  // cache the class component used (which only keyed off series identity --
  // keying off width/height too is strictly more correct).
  const { pAxis, sparklines } = React.useMemo(() => {
    const time = defined(series[0]).time;
    const x = 0;
    const y = 0;
    const w = width;
    const h = height;

    const xMin = time[0];
    const xMax = last(time);
    const xSpan = xMax - xMin;

    let yMin = 0;
    let yMax = -Infinity;
    // first build up the min + max across all datasets
    for (const dataset of series) {
      const values = dataset.values;
      yMin = Math.min(yMin, min(values)); // 0 or below 0
      yMax = Math.max(yMax, max(values));
    }
    const ySpan = yMax - yMin || 1;

    const colors = Dark2;
    const lines = [];
    let i = 0;
    for (const dataset of series) {
      const values = dataset.values;
      let p = '';
      for (let j = 0; j < values.length; j++) {
        if (isNaN(values[j])) {
          // console.log(`NaN at ${time[j]}`);
          continue;
        }
        const prefix = j === 0 ? 'M' : 'L';
        // Quantize sparkline path coordinates for cross-toolchain SVG parity;
        // see `jsFormatNumber` in `render-common.tsx`.
        p += `${prefix}${f(x + (w * (time[j] - xMin)) / xSpan)},${f(y + h - (h * (values[j] - yMin)) / ySpan)}`;
      }
      const style = series.length === 1 ? undefined : { stroke: colors[i % colors.length] };
      lines.push(
        <path key={dataset.name} d={p} className={`${styles.sparkline} simlin-sparkline-line`} style={style} />,
      );
      i++;
    }

    const axis = `M${f(x)},${f(y + h - (h * (0 - yMin)) / ySpan)}L${f(x + w)},${f(y + h - (h * (0 - yMin)) / ySpan)}`;
    return { pAxis: axis, sparklines: lines };
  }, [series, width, height]);

  return (
    <g>
      <path key="$axis" d={pAxis} className={`${styles.axis} simlin-sparkline-axis`} />
      {sparklines}
    </g>
  );
});
