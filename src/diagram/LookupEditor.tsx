// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import Button from './components/Button';
import TextField from './components/TextField';

import { defined } from '@simlin/core/common';
import { at } from '@simlin/core/collections';
import { Variable, GraphicalFunction, GraphicalFunctionKind, variableGf } from '@simlin/core/datamodel';

import { LineChart } from './LineChart';

import styles from './LookupEditor.module.css';

// GFTable is a consistent format for the data from a GF that can be
// used for efficient lookup operations
export interface GFTable {
  type: GraphicalFunctionKind;
  size: number;
  x: Float64Array;
  y: Float64Array;
}

// The x value for point i of a `size`-point table linearly spanning
// [xMin, xMax]. A single-point table maps to xMin -- the naive
// `i / (size - 1)` is 0/0 === NaN for size 1, and a NaN x poisons the
// lookup()-based resampling (every interpolation read returns NaN) and can
// then be saved into the table.
export function xAtTableIndex(i: number, size: number, xMin: number, xMax: number): number {
  if (size <= 1) {
    return xMin;
  }
  return (i / (size - 1)) * (xMax - xMin) + xMin;
}

function tableFrom(gf: GraphicalFunction): GFTable | undefined {
  const xpts = gf.xPoints;
  const xscale = gf.xScale;
  const xmin = xscale ? xscale.min : 0;
  const xmax = xscale ? xscale.max : 0;

  if (!gf.yPoints) {
    return undefined;
  }
  const ypts: readonly number[] = gf.yPoints;

  const size = gf.yPoints.length;
  const xList = new Float64Array(size);
  const yList = new Float64Array(size);

  for (let i = 0; i < ypts.length; i++) {
    // either the x points have been explicitly specified, or
    // it is a linear mapping of points between xmin and xmax,
    // inclusive
    const xVal = xpts ? at(xpts, i) : xAtTableIndex(i, size, xmin, xmax);
    xList[i] = xVal;
    yList[i] = at(ypts, i);
  }

  return {
    size,
    type: gf.kind,
    x: xList,
    y: yList,
  };
}

interface LookupEditorProps {
  variable: Variable;
  onLookupChange: (ident: string, newTable: GraphicalFunction | null) => void;
}

const CHART_HEIGHT = 300;

function lookup(table: GFTable, index: number): number {
  const size = table.size;
  if (size <= 0) {
    return NaN;
  }

  const x = table.x;
  const y = table.y;

  if (index <= x[0]) {
    return y[0];
  } else if (index >= x[size - 1]) {
    return y[size - 1];
  }

  // binary search seems to be the most appropriate choice here.
  let low = 0;
  let high = size;
  let mid: number;
  while (low < high) {
    mid = Math.floor(low + (high - low) / 2);
    if (x[mid] < index) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }

  const i = low;
  if (x[i] === index) {
    return y[i];
  } else {
    // slope = deltaY/deltaX
    const slope = (y[i] - y[i - 1]) / (x[i] - x[i - 1]);
    // y = m*x + b
    return (index - x[i - 1]) * slope + y[i - 1];
  }
}

// Derive the editor's GraphicalFunction from a variable, ensuring yScale always
// exists (deriving it from the y-point extent when absent). Pure in `variable`.
function getVariableGF(variable: Variable): GraphicalFunction {
  let gf = defined(variableGf(variable));

  // ensure yScale always exists
  if (!gf.yScale) {
    let min = 0;
    let max = 0;
    for (let i = 0; i < gf.yPoints.length; i++) {
      const y = at(gf.yPoints, i);
      if (y < min) {
        min = y;
      }
      if (y > max) {
        max = y;
      }
    }
    min = Math.floor(min);
    max = Math.ceil(max);

    gf = { ...gf, yScale: { min, max } };
  }

  return gf;
}

// Recompute the x-points of a table to linearly span the gf's x scale. Pure;
// returns a fresh table (formerly the static LookupEditor.rescaleX).
export function rescaleX(gf: GraphicalFunction, table: GFTable): GFTable {
  const newTable = Object.assign({}, table);
  newTable.x = new Float64Array(table.x);

  if (table.size === 0) {
    return newTable;
  }

  const size = table.size;
  const xMin = defined(gf.xScale).min;
  const xMax = defined(gf.xScale).max;
  if (xMin >= xMax) {
    return newTable;
  }

  for (let i = 0; i < table.size; i++) {
    newTable.x[i] = xAtTableIndex(i, size, xMin, xMax);
  }

  return newTable;
}

interface LookupEditorSeed {
  hasChange: boolean;
  gf: GraphicalFunction;
  table: GFTable;
  yMin: number;
  yMax: number;
  datapointCount: number;
}

// Seed the editable state from a variable -- used both for the initial mount
// (lazy useState initializer) and for Cancel (revert to the saved values).
function seedFromVariable(variable: Variable): LookupEditorSeed {
  const gf = getVariableGF(variable);
  const table = defined(tableFrom(gf));
  return {
    hasChange: false,
    gf,
    table,
    yMin: defined(gf.yScale).min,
    yMax: defined(gf.yScale).max,
    datapointCount: table.size,
  };
}

export function LookupEditor(props: LookupEditorProps): React.ReactElement {
  const { variable, onLookupChange } = props;

  // Seed the editable state once per mount from props, exactly as the class
  // constructor did. The panel is remounted (keyed) when the underlying
  // variable changes, so there is intentionally no prop-sync effect here.
  const [seed] = React.useState<LookupEditorSeed>(() => seedFromVariable(variable));
  const [hasChange, setHasChange] = React.useState(seed.hasChange);
  const [gf, setGf] = React.useState(seed.gf);
  const [table, setTable] = React.useState(seed.table);
  const [yMin, setYMin] = React.useState(seed.yMin);
  const [yMax, setYMax] = React.useState(seed.yMax);
  const [datapointCount, setDatapointCount] = React.useState(seed.datapointCount);
  // The inDrag flag mirrored the class state but was never read in render; it is
  // kept as a ref to preserve the onDragStart/onDragEnd bookkeeping without a
  // redundant re-render.
  const inDrag = React.useRef(false);

  const formatValue = (value: number): string => {
    return value.toFixed(3);
  };

  const handlePointDrag = (_seriesIndex: number, pointIndex: number, newY: number): void => {
    const newTable = { ...table, y: new Float64Array(table.y) };
    newTable.y[pointIndex] = newY;
    setHasChange(true);
    setTable(newTable);
  };

  const handleDragStart = (): void => {
    inDrag.current = true;
  };

  const handleDragEnd = (): void => {
    inDrag.current = false;
  };

  const handleYMinChange = (event: React.ChangeEvent<HTMLInputElement>): void => {
    const value = Number(event.target.value);
    setHasChange(true);
    setYMin(value);

    if (value < yMax) {
      setGf({ ...gf, yScale: { min: value, max: yMax } });
    }
  };

  const handleYMaxChange = (event: React.ChangeEvent<HTMLInputElement>): void => {
    const value = Number(event.target.value);
    setHasChange(true);
    setYMax(value);

    if (yMin < value) {
      setGf({ ...gf, yScale: { min: yMin, max: value } });
    }
  };

  const handleXMinChange = (event: React.ChangeEvent<HTMLInputElement>): void => {
    const value = Number(event.target.value);
    const xScale = defined(gf.xScale);
    const newGf: GraphicalFunction = { ...gf, xScale: { ...xScale, min: value } };

    const newTable = rescaleX(newGf, table);

    setHasChange(true);
    setGf(newGf);
    setTable(newTable);
  };

  const handleXMaxChange = (event: React.ChangeEvent<HTMLInputElement>): void => {
    const value = Number(event.target.value);
    const xScale = defined(gf.xScale);
    const newGf: GraphicalFunction = { ...gf, xScale: { ...xScale, max: value } };

    const newTable = rescaleX(newGf, table);

    setHasChange(true);
    setGf(newGf);
    setTable(newTable);
  };

  const handleDatapointCountChange = (event: React.ChangeEvent<HTMLInputElement>): void => {
    const newCount = Number(event.target.value);
    setHasChange(true);
    setDatapointCount(newCount);

    // don't rescale things when the current count is obviously bad
    if (newCount <= 0) {
      return;
    }

    const oldTable = table;
    const newTable = Object.assign({}, oldTable);
    newTable.x = new Float64Array(newCount);
    newTable.y = new Float64Array(newCount);

    const size = newCount;
    const xMin = defined(gf.xScale).min;
    const xMax = defined(gf.xScale).max;
    if (xMin >= xMax) {
      return;
    }

    for (let i = 0; i < size; i++) {
      const xVal = xAtTableIndex(i, size, xMin, xMax);
      newTable.x[i] = xVal;
      newTable.y[i] = lookup(oldTable, xVal);
    }
    newTable.size = newCount;

    setHasChange(true);
    setTable(newTable);
  };

  const handleLookupRemove = (): void => {
    onLookupChange(defined(variable.ident), null);
  };

  const handleLookupCancel = (): void => {
    const reverted = seedFromVariable(variable);
    setHasChange(false);
    setGf(reverted.gf);
    setTable(reverted.table);
    setYMin(reverted.yMin);
    setYMax(reverted.yMax);
    setDatapointCount(reverted.datapointCount);
  };

  const handleLookupSave = (): void => {
    const yPoints = Array.from(table.y);
    onLookupChange(defined(variable.ident), { ...gf, yPoints });
    setHasChange(false);
  };

  const yMinChart = defined(gf.yScale).min;
  const yMaxChart = defined(gf.yScale).max;

  const xMin = gf.xScale ? gf.xScale.min : 0;
  const xMax = gf.xScale ? gf.xScale.max : 0;

  const lookupActionsEnabled = hasChange;

  const series: { x: number; y: number }[] = [];
  for (let i = 0; i < table.size; i++) {
    series.push({ x: table.x[i], y: table.y[i] });
  }

  const xScaleError = xMin >= xMax;
  const yScaleError = yMin >= yMax;
  const datapointCountError = datapointCount <= 0;

  const isSaveDisabled = !lookupActionsEnabled || xScaleError || yScaleError || datapointCountError;

  return (
    <div>
      <div className={styles.cardContent}>
        <TextField
          className={styles.yAxisMax}
          error={yScaleError}
          label="Y axis max"
          value={yMax}
          onChange={handleYMaxChange}
          type="number"
          margin="normal"
        />
        <LineChart
          height={CHART_HEIGHT}
          series={[{ name: 'y', color: '#8884d8', points: series }]}
          yDomain={[yMinChart, yMaxChart]}
          tooltipFormatter={formatValue}
          onPointDrag={handlePointDrag}
          onDragStart={handleDragStart}
          onDragEnd={handleDragEnd}
        />
        <TextField
          className={styles.yAxisMin}
          error={yScaleError}
          label="Y axis min"
          value={yMin}
          onChange={handleYMinChange}
          type="number"
          margin="normal"
        />
        <br />
        <TextField
          className={styles.xScaleMin}
          error={xScaleError}
          label="X axis min"
          value={xMin}
          onChange={handleXMinChange}
          type="number"
          margin="normal"
        />
        <TextField
          className={styles.xScaleMax}
          error={xScaleError}
          label="X axis max"
          value={xMax}
          onChange={handleXMaxChange}
          type="number"
          margin="normal"
        />
        <TextField
          className={styles.datapoints}
          error={datapointCountError}
          label="Datapoint Count"
          value={datapointCount}
          onChange={handleDatapointCountChange}
          type="number"
          margin="normal"
        />
      </div>
      <div className={styles.cardActions}>
        <Button size="small" color="secondary" onClick={handleLookupRemove} className={styles.buttonLeft}>
          Remove
        </Button>
        <div className={styles.buttonRight}>
          <Button size="small" color="primary" disabled={!lookupActionsEnabled} onClick={handleLookupCancel}>
            Cancel
          </Button>
          <Button size="small" color="primary" disabled={isSaveDisabled} onClick={handleLookupSave}>
            Save
          </Button>
        </div>
      </div>
    </div>
  );
}
