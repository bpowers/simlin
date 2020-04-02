// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { CartesianGrid, Line, LineChart, Tooltip, XAxis, YAxis } from 'recharts';
import { Button, CardActions, CardContent, TextField } from '@material-ui/core';
import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { defined } from '../common';
import { isEqual, Point } from './drawing/common';
import { Table } from '../../engine/vars';
import { GF, Scale } from '../../engine/xmile';

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

export interface Coordinates {
  x: List<number>;
  y: List<number>;
}

interface LookupEditorPropsFull extends WithStyles<typeof styles> {
  variable: Table;
  onLookupChange: (ident: string, newTable: GF) => void;
}

// export type LookupEditorProps = Pick<LookupEditorPropsFull, 'variable' | 'viewElement' | 'data'>;

interface LookupEditorState {
  inDrag: boolean;
  series: Point[];
  yMin: number;
  yMax: number;
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

      const { variable } = this.props;
      const gf = this.gf();

      let yMin = 0;
      let yMax = 0;
      const series: { x: number; y: number }[] = [];
      for (let i = 0; i < variable.x.size; i++) {
        const x = defined(variable.x.get(i));
        const y = defined(variable.y.get(i));
        series.push({ x, y });
        if (y < yMin) {
          yMin = y;
        }
        if (y > yMax) {
          yMax = y;
        }
      }
      yMin = Math.floor(yMin);
      yMax = Math.ceil(yMax);

      this.lookupRef = React.createRef();
      this.state = {
        inDrag: false,
        series,
        yMin: gf.yScale ? gf.yScale.min : yMin,
        yMax: gf.yScale ? gf.yScale.max : yMax,
      };
    }

    gf(): GF {
      const { variable } = this.props;
      const xVar = defined(variable.xmile);
      return defined(xVar.gf);
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

      const { series, yMin, yMax } = this.state;
      const xPoints: List<number> = List(series.map((p: Point): number => p.x));
      const yPoints: List<number> = List(series.map((p: Point): number => p.y));

      const { variable } = this.props;
      const xVar = defined(variable.xmile);
      const yScale = new Scale({ min: yMin, max: yMax });
      const gf = defined(xVar.gf).set('xPoints', xPoints).set('yPoints', yPoints).set('yScale', yScale);

      this.props.onLookupChange(defined(this.props.variable.ident), gf);
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

      const { yMin, yMax } = this.state;

      const x = details.activePayload[0].payload.x;
      let y = yScale.invert(details.chartY);
      if (y > yMax) {
        y = yMax;
      } else if (y < yMin) {
        y = yMin;
      }

      const series = this.state.series.map((p: Point) => {
        if (isEqual(p.x, x)) {
          return {
            x,
            y,
          };
        } else {
          return p;
        }
      });

      this.setState({ series });
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
      this.setState({ yMin: Number(event.target.value) });
    };

    handleYMaxChange = (event: React.ChangeEvent<HTMLInputElement>) => {
      this.setState({ yMax: Number(event.target.value) });
    };

    handleLookupRemove = (): void => {};

    handleEquationCancel = (): void => {};

    handleEquationSave = (): void => {};

    render() {
      const { classes } = this.props;
      const { yMin, yMax, series } = this.state;

      const charWidth = Math.max(yMin.toFixed(0).length, yMax.toFixed(0).length);
      const yAxisWidth = Math.max(40, 20 + charWidth * 6);

      const { left, right } = {
        left: 'dataMin',
        right: 'dataMax',
      };

      const gf = this.gf();
      const xMin = gf.xScale ? gf.xScale.min : 0;
      const xMax = gf.xScale ? gf.xScale.max : 0;

      const lookupActionsEnabled = false;

      return (
        <div>
          <CardContent>
            <TextField
              className={classes.yAxisMax}
              label="Y axis max"
              value={this.state.yMax}
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
              value={this.state.yMin}
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
              <Button size="small" color="primary" disabled={!lookupActionsEnabled} onClick={this.handleEquationCancel}>
                Cancel
              </Button>
              <Button size="small" color="primary" disabled={!lookupActionsEnabled} onClick={this.handleEquationSave}>
                Save
              </Button>
            </div>
          </CardActions>
        </div>
      );
    }
  },
);
