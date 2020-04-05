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
}

function getAnyElementOfObject(obj: any): any | undefined {
  if (!obj) {
    return undefined;
  }

  const keys = Object.keys(obj);
  if (keys && keys.length) {
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
      if (!details || !details.hasOwnProperty('chartX') || !details.hasOwnProperty('chartY')) {
        return;
      }

      const chart: any = this.lookupRef.current;
      if (chart === null || !chart.state.yAxisMap) {
        return;
      }

      const yAxisMap = getAnyElementOfObject(chart.state.yAxisMap);
      const yScale = yAxisMap.scale;
      if (!yScale || !yScale.invert) {
        return;
      }

      const { variable } = this.props;
      const { gf, table } = this.state;

      const newTable = Object.assign({}, table);
      newTable.y = new Float64Array(table.y);

      const yMin = defined(gf.yScale).min;
      const yMax = defined(gf.yScale).max;

      const x = details.activePayload[0].payload.x;
      let y = yScale.invert(details.chartY);
      if (y > yMax) {
        y = yMax;
      } else if (y < yMin) {
        y = yMin;
      }

      let off = -1;
      for (let i = 0; i < variable.x.size; i++) {
        if (isEqual(defined(variable.x.get(i)), x)) {
          off = i;
          break;
        }
      }

      if (off < 0) {
        // this is very unexpected
        return;
      }

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
      const yScale = defined(this.state.gf.yScale);
      const gf = this.state.gf.set('yScale', yScale.set('min', value));
      this.setState({
        hasChange: true,
        gf,
      });
    };

    handleYMaxChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      const value = Number(event.target.value);
      const yScale = defined(this.state.gf.yScale);
      const gf = this.state.gf.set('yScale', yScale.set('max', value));
      this.setState({
        hasChange: true,
        gf,
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
      });
    };

    handleLookupSave = (): void => {
      const { gf, table } = this.state;
      const yPoints = table.y.reduce((pts: List<number>, curr: number) => pts.push(curr), List());
      this.props.onLookupChange(defined(this.props.variable.ident), gf.set('yPoints', yPoints));
    };

    render() {
      const { classes } = this.props;
      const { gf, table } = this.state;

      const yMin = defined(gf.yScale).min;
      const yMax = defined(gf.yScale).max;
      const charWidth = Math.max(yMin.toFixed(0).length, yMax.toFixed(0).length);
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

      return (
        <div>
          <CardContent>
            <TextField
              className={classes.yAxisMax}
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
                  domain={[yMin, yMax]}
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
              label="Y axis min"
              value={yMin}
              onChange={this.handleYMinChange}
              type="number"
              margin="normal"
            />
            <br />
            <TextField
              className={classes.xScaleMin}
              label="X axis min"
              value={xMin}
              // onChange={this.handleYMinChange}
              type="number"
              margin="normal"
            />
            <TextField
              className={classes.xScaleMax}
              label="X axis max"
              value={xMax}
              // onChange={this.handleYMinChange}
              type="number"
              margin="normal"
            />
            <TextField
              className={classes.datapoints}
              label="Datapoint Count"
              value={xMax}
              // onChange={this.handleYMinChange}
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
              <Button size="small" color="primary" disabled={!lookupActionsEnabled} onClick={this.handleLookupSave}>
                Save
              </Button>
            </div>
          </CardActions>
        </div>
      );
    }
  },
);
