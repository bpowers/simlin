// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { CartesianGrid, Line, LineChart, Tooltip, XAxis, YAxis } from 'recharts';
import { Button, CardActions, CardContent, TextField } from '@material-ui/core';
import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { defined } from '../common';
import { isEqual } from './drawing/common';
import { Table } from '../../engine/vars';
import { GF, GFTable, Scale } from '../../engine/xmile';
import { List } from 'immutable';

const styles = createStyles({
  yAxisMax: {
    width: '30%',
    paddingRight: 4,
    marginTop: 0,
  },
  yAxisMin: {
    width: '30%',
    paddingRight: 4,
    marginTop: 4,
  },
  xScaleMin: {
    width: '30%',
    paddingRight: 4,
  },
  xScaleMax: {
    width: '30%',
    paddingLeft: 4,
    paddingRight: 4,
  },
  datapoints: {
    width: '40%',
    paddingLeft: 4,
  },
  buttonLeft: {
    float: 'left',
    marginRight: 'auto',
  },
  buttonRight: {
    float: 'right',
  },
});

interface LookupEditorPropsFull extends WithStyles<typeof styles> {
  variable: Table;
  onLookupChange: (ident: string, newTable: GF | null) => void;
}

// export type LookupEditorProps = Pick<LookupEditorPropsFull, 'variable' | 'viewElement' | 'data'>;

interface LookupEditorState {
  inDrag: boolean;
  hasChange: boolean;
  gf: GF;
  table: GFTable;
  yMin: number;
  yMax: number;
  datapointCount: number;
}

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

function getAnyElementOfObject(obj: any): any | undefined {
  if (!obj) {
    return undefined;
  }

  const keys = Object.keys(obj);
  if (keys && keys.length) {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return obj[keys[0]];
  }

  return undefined;
}

export const LookupEditor = withStyles(styles)(
  class InnerLookupEditor extends React.PureComponent<LookupEditorPropsFull, LookupEditorState> {
    readonly lookupRef: React.RefObject<LineChart>;

    constructor(props: LookupEditorPropsFull) {
      super(props);

      const gf = this.getVariableGF();
      const table = defined(gf.table());

      this.lookupRef = React.createRef();
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

    getVariableGF(): GF {
      const { variable } = this.props;
      const xVar = defined(variable.xmile);
      let gf = defined(xVar.gf);

      // ensure yScale always exists
      if (!gf.yScale) {
        let min = 0;
        let max = 0;
        for (let i = 0; i < variable.x.size; i++) {
          const y = defined(variable.y.get(i));
          if (y < min) {
            min = y;
          }
          if (y > max) {
            max = y;
          }
        }
        min = Math.floor(min);
        max = Math.ceil(max);

        gf = gf.set('yScale', new Scale({ min, max }));
      }

      return gf;
    }

    formatValue = (value: string | number | (string | number)[]): string | (string | number)[] => {
      return typeof value === 'number' ? value.toFixed(3) : value;
    };

    handleContainerMouseDown = (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      e.stopPropagation();
    };

    handleContainerMouseUp = (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      e.stopPropagation();
    };

    handleContainerMouseMove = (e: React.MouseEvent<HTMLDivElement>) => {
      e.preventDefault();
      e.stopPropagation();
    };

    handleContainerTouchStart = (e: React.PointerEvent<HTMLDivElement>) => {
      const chart: any = this.lookupRef.current;
      if (!chart) {
        return;
      }
      // ensure we get 'mouse up' events (and friends) even if we leave the
      // confines of the chart
      // eslint-disable-next-line @typescript-eslint/no-unsafe-call
      (e.target as any).setPointerCapture(e.pointerId);
    };

    handleContainerTouchEnd = (_e: React.PointerEvent<HTMLDivElement>) => {};

    handleContainerTouchMove = (_e: React.PointerEvent<HTMLDivElement>) => {};

    handleMouseUp = () => {
      this.endEditing();
    };

    endEditing() {
      this.setState({ inDrag: false });

      // const { series, yMin, yMax } = this.state;
      // const xPoints: List<number> = List(series.map((p: Point): number => p.x));
      // const yPoints: List<number> = List(series.map((p: Point): number => p.y));
      //
      // const { variable } = this.props;
      // const xVar = defined(variable.xmile);
      // const yScale = new Scale({ min: yMin, max: yMax });
      // const gf = defined(xVar.gf).set('xPoints', xPoints).set('yPoints', yPoints).set('yScale', yScale);

      // this.props.onLookupChange(defined(this.props.variable.ident), gf);
    }

    updatePoint(details: any) {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-call
      if (!details || !details.hasOwnProperty('chartX') || !details.hasOwnProperty('chartY')) {
        return;
      }

      const chart: any = this.lookupRef.current;
      if (chart === null || !chart.state.yAxisMap) {
        return;
      }

      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const yAxisMap = getAnyElementOfObject(chart.state.yAxisMap);
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const yScale = yAxisMap.scale;
      if (!yScale || !yScale.invert) {
        return;
      }

      const { gf, table } = this.state;

      const newTable = Object.assign({}, table);
      newTable.y = new Float64Array(table.y);

      const yMin = defined(gf.yScale).min;
      const yMax = defined(gf.yScale).max;

      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const x = details.activePayload[0].payload.x;
      // eslint-disable-next-line @typescript-eslint/no-unsafe-call,@typescript-eslint/no-unsafe-assignment
      let y = yScale.invert(details.chartY);
      if (y > yMax) {
        y = yMax;
      } else if (y < yMin) {
        y = yMin;
      }

      let off = -1;
      for (let i = 0; i < newTable.size; i++) {
        if (isEqual(newTable.x[i], x)) {
          off = i;
          break;
        }
      }

      if (off < 0) {
        // this is very unexpected
        return;
      }

      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      newTable.y[off] = y;
      this.setState({
        hasChange: true,
        table: newTable,
      });
    }

    handleMouseDown = (details: any) => {
      this.setState({ inDrag: true });
      this.updatePoint(details);
    };

    handleMouseMove = (details: any, event: React.MouseEvent<LineChart>) => {
      if (!this.state.inDrag) {
        return;
      }

      // if we were dragging in the chart, left the chart, stopped pressing the mouse,
      // then moused back in we might mistakenly think we were still inDrag
      if (event.hasOwnProperty('buttons') && event.buttons === 0) {
        this.endEditing();
      }

      this.updatePoint(details);
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
          gf: this.state.gf.set(
            'yScale',
            new Scale({
              min: value,
              max: yMax,
            }),
          ),
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
          gf: this.state.gf.set(
            'yScale',
            new Scale({
              min: yMin,
              max: value,
            }),
          ),
        });
      }
    };

    static rescaleX(gf: GF, table: GFTable): GFTable {
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
      const gf = this.state.gf.set('xScale', xScale.set('min', value));

      const newTable = InnerLookupEditor.rescaleX(gf, table);

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
      const gf = this.state.gf.set('xScale', xScale.set('max', value));

      const newTable = InnerLookupEditor.rescaleX(gf, table);

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
        const x = (i / (size - 1)) * (xMax - xMin) + xMin;
        newTable.x[i] = x;
        newTable.y[i] = lookup(oldTable, x);
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
      const table = defined(gf.table());
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
      const yPoints = table.y.reduce((pts: List<number>, curr: number) => pts.push(curr), List());
      this.props.onLookupChange(defined(this.props.variable.ident), gf.set('yPoints', yPoints));
      this.setState({ hasChange: false });
    };

    render() {
      const { classes } = this.props;
      const { datapointCount, gf, table, yMin, yMax } = this.state;

      const yMinChart = defined(gf.yScale).min;
      const yMaxChart = defined(gf.yScale).max;

      const charWidth = Math.max(yMinChart.toFixed(0).length, yMaxChart.toFixed(0).length);
      const yAxisWidth = Math.max(40, 20 + charWidth * 6);

      const { left, right } = {
        left: 'dataMin',
        right: 'dataMax',
      };

      const xMin = gf.xScale ? gf.xScale.min : 0;
      const xMax = gf.xScale ? gf.xScale.max : 0;

      const lookupActionsEnabled = this.state.hasChange;

      const series: { x: number; y: number }[] = [];
      for (let i = 0; i < table.size; i++) {
        const x = table.x[i];
        const y = table.y[i];
        series.push({ x, y });
      }

      const xScaleError = xMin >= xMax;
      const yScaleError = yMin >= yMax;
      const datapointCountError = datapointCount <= 0;

      const isSaveDisabled = !lookupActionsEnabled || xScaleError || yScaleError || datapointCountError;

      return (
        <div>
          <CardContent>
            <TextField
              className={classes.yAxisMax}
              error={yScaleError}
              label="Y axis max"
              value={yMax}
              onChange={this.handleYMaxChange}
              type="number"
              margin="normal"
            />
            <div
              onMouseDown={this.handleContainerMouseDown}
              onMouseUp={this.handleContainerMouseUp}
              onMouseMove={this.handleContainerMouseMove}
              onPointerDown={this.handleContainerTouchStart}
              onPointerUp={this.handleContainerTouchEnd}
              onPointerMove={this.handleContainerTouchMove}
            >
              <LineChart
                width={327}
                height={300}
                data={series}
                onMouseDown={this.handleMouseDown}
                onMouseMove={this.handleMouseMove}
                onMouseUp={this.handleMouseUp}
                ref={this.lookupRef}
                layout={'horizontal'}
              >
                <CartesianGrid horizontal={true} vertical={false} />
                <XAxis allowDataOverflow={true} dataKey="x" domain={[left, right]} type="number" />
                <YAxis
                  width={yAxisWidth}
                  allowDataOverflow={true}
                  domain={[yMinChart, yMaxChart]}
                  type="number"
                  dataKey="y"
                  yAxisId="1"
                />
                <Tooltip formatter={this.formatValue} />
                <Line yAxisId="1" type="linear" dataKey="y" stroke="#8884d8" isAnimationActive={false} dot={false} />
              </LineChart>
            </div>
            <TextField
              className={classes.yAxisMin}
              error={yScaleError}
              label="Y axis min"
              value={yMin}
              onChange={this.handleYMinChange}
              type="number"
              margin="normal"
            />
            <br />
            <TextField
              className={classes.xScaleMin}
              error={xScaleError}
              label="X axis min"
              value={xMin}
              onChange={this.handleXMinChange}
              type="number"
              margin="normal"
            />
            <TextField
              className={classes.xScaleMax}
              error={xScaleError}
              label="X axis max"
              value={xMax}
              onChange={this.handleXMaxChange}
              type="number"
              margin="normal"
            />
            <TextField
              className={classes.datapoints}
              error={datapointCountError}
              label="Datapoint Count"
              value={datapointCount}
              onChange={this.handleDatapointCountChange}
              type="number"
              margin="normal"
            />
          </CardContent>
          <CardActions>
            <Button size="small" color="secondary" onClick={this.handleLookupRemove} className={classes.buttonLeft}>
              Remove
            </Button>
            <div className={classes.buttonRight}>
              <Button size="small" color="primary" disabled={!lookupActionsEnabled} onClick={this.handleLookupCancel}>
                Cancel
              </Button>
              <Button size="small" color="primary" disabled={isSaveDisabled} onClick={this.handleLookupSave}>
                Save
              </Button>
            </div>
          </CardActions>
        </div>
      );
    }
  },
);
