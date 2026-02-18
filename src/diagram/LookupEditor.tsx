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
    const xVal = xpts ? at(xpts, i) : (i / (size - 1)) * (xmax - xmin) + xmin;
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

interface LookupEditorState {
  inDrag: boolean;
  hasChange: boolean;
  gf: GraphicalFunction;
  table: GFTable;
  yMin: number;
  yMax: number;
  datapointCount: number;
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

export class LookupEditor extends React.PureComponent<LookupEditorProps, LookupEditorState> {
  constructor(props: LookupEditorProps) {
    super(props);

    const gf = this.getVariableGF();
    const table = defined(tableFrom(gf));

    this.state = {
      inDrag: false,
      hasChange: false,
      gf,
      table,
      yMin: defined(gf.yScale).min,
      yMax: defined(gf.yScale).max,
      datapointCount: table.size,
    };
  }

  getVariableGF(): GraphicalFunction {
    const { variable } = this.props;
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

  formatValue = (value: number): string => {
    return value.toFixed(3);
  };

  handlePointDrag = (_seriesIndex: number, pointIndex: number, newY: number) => {
    const { table } = this.state;
    const newTable = { ...table, y: new Float64Array(table.y) };
    newTable.y[pointIndex] = newY;
    this.setState({ hasChange: true, table: newTable });
  };

  handleDragStart = () => {
    this.setState({ inDrag: true });
  };

  handleDragEnd = () => {
    this.setState({ inDrag: false });
  };

  handleYMinChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = Number(event.target.value);
    this.setState({
      hasChange: true,
      yMin: value,
    });

    const { yMax } = this.state;
    if (value < yMax) {
      this.setState({
        gf: { ...this.state.gf, yScale: { min: value, max: yMax } },
      });
    }
  };

  handleYMaxChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = Number(event.target.value);
    this.setState({
      hasChange: true,
      yMax: value,
    });

    const { yMin } = this.state;
    if (yMin < value) {
      this.setState({
        gf: { ...this.state.gf, yScale: { min: yMin, max: value } },
      });
    }
  };

  static rescaleX(gf: GraphicalFunction, table: GFTable): GFTable {
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
      newTable.x[i] = (i / (size - 1)) * (xMax - xMin) + xMin;
    }

    return newTable;
  }

  handleXMinChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = Number(event.target.value);
    const { table } = this.state;
    const xScale = defined(this.state.gf.xScale);
    const gf: GraphicalFunction = { ...this.state.gf, xScale: { ...xScale, min: value } };

    const newTable = LookupEditor.rescaleX(gf, table);

    this.setState({
      hasChange: true,
      gf,
      table: newTable,
    });
  };

  handleXMaxChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = Number(event.target.value);
    const { table } = this.state;
    const xScale = defined(this.state.gf.xScale);
    const gf: GraphicalFunction = { ...this.state.gf, xScale: { ...xScale, max: value } };

    const newTable = LookupEditor.rescaleX(gf, table);

    this.setState({
      hasChange: true,
      gf,
      table: newTable,
    });
  };

  handleDatapointCountChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const datapointCount = Number(event.target.value);
    this.setState({
      hasChange: true,
      datapointCount,
    });

    // don't rescale things when the current count is obviously bad
    if (datapointCount <= 0) {
      return;
    }

    const { gf } = this.state;
    const oldTable = this.state.table;
    const newTable = Object.assign({}, oldTable);
    newTable.x = new Float64Array(datapointCount);
    newTable.y = new Float64Array(datapointCount);

    const size = datapointCount;
    const xMin = defined(gf.xScale).min;
    const xMax = defined(gf.xScale).max;
    if (xMin >= xMax) {
      return;
    }

    for (let i = 0; i < size; i++) {
      const xVal = (i / (size - 1)) * (xMax - xMin) + xMin;
      newTable.x[i] = xVal;
      newTable.y[i] = lookup(oldTable, xVal);
    }
    newTable.size = datapointCount;

    this.setState({
      hasChange: true,
      table: newTable,
    });
  };

  handleLookupRemove = (): void => {
    this.props.onLookupChange(defined(this.props.variable.ident), null);
  };

  handleLookupCancel = (): void => {
    const gf = this.getVariableGF();
    const table = defined(tableFrom(gf));
    this.setState({
      hasChange: false,
      gf,
      table,
      yMin: defined(gf.yScale).min,
      yMax: defined(gf.yScale).max,
      datapointCount: table.size,
    });
  };

  handleLookupSave = (): void => {
    const { gf, table } = this.state;
    const yPoints = Array.from(table.y);
    this.props.onLookupChange(defined(this.props.variable.ident), { ...gf, yPoints });
    this.setState({ hasChange: false });
  };

  render() {
    const { datapointCount, gf, table, yMin, yMax } = this.state;

    const yMinChart = defined(gf.yScale).min;
    const yMaxChart = defined(gf.yScale).max;

    const xMin = gf.xScale ? gf.xScale.min : 0;
    const xMax = gf.xScale ? gf.xScale.max : 0;

    const lookupActionsEnabled = this.state.hasChange;

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
            onChange={this.handleYMaxChange}
            type="number"
            margin="normal"
          />
          <LineChart
            height={CHART_HEIGHT}
            series={[{ name: 'y', color: '#8884d8', points: series }]}
            yDomain={[yMinChart, yMaxChart]}
            tooltipFormatter={this.formatValue}
            onPointDrag={this.handlePointDrag}
            onDragStart={this.handleDragStart}
            onDragEnd={this.handleDragEnd}
          />
          <TextField
            className={styles.yAxisMin}
            error={yScaleError}
            label="Y axis min"
            value={yMin}
            onChange={this.handleYMinChange}
            type="number"
            margin="normal"
          />
          <br />
          <TextField
            className={styles.xScaleMin}
            error={xScaleError}
            label="X axis min"
            value={xMin}
            onChange={this.handleXMinChange}
            type="number"
            margin="normal"
          />
          <TextField
            className={styles.xScaleMax}
            error={xScaleError}
            label="X axis max"
            value={xMax}
            onChange={this.handleXMaxChange}
            type="number"
            margin="normal"
          />
          <TextField
            className={styles.datapoints}
            error={datapointCountError}
            label="Datapoint Count"
            value={datapointCount}
            onChange={this.handleDatapointCountChange}
            type="number"
            margin="normal"
          />
        </div>
        <div className={styles.cardActions}>
          <Button size="small" color="secondary" onClick={this.handleLookupRemove} className={styles.buttonLeft}>
            Remove
          </Button>
          <div className={styles.buttonRight}>
            <Button size="small" color="primary" disabled={!lookupActionsEnabled} onClick={this.handleLookupCancel}>
              Cancel
            </Button>
            <Button size="small" color="primary" disabled={isSaveDisabled} onClick={this.handleLookupSave}>
              Save
            </Button>
          </div>
        </div>
      </div>
    );
  }
}
