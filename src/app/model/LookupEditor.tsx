// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { CartesianGrid, Line, LineChart, Tooltip, XAxis, YAxis } from 'recharts';
import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { defined } from '../common';
import { isEqual, Point } from './drawing/common';
import { Table } from '../../engine/vars';

const styles = createStyles({});

export interface Coordinates {
  x: List<number>;
  y: List<number>;
}

interface LookupEditorPropsFull extends WithStyles<typeof styles> {
  variable: Table;
  onLookupChange: (ident: string, newTable: Coordinates) => void;
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

      const { variable } = props;

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
        yMin,
        yMax,
      };
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
      this.setState({ inDrag: false });
      console.log('mouse up!');
    };

    updatePoint(details: any) {
      if (!details.hasOwnProperty('chartX') || !details.hasOwnProperty('chartY')) {
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
        this.setState({ inDrag: false });
      }

      this.updatePoint(details);
    };

    render() {
      const { yMin, yMax, series } = this.state;

      const charWidth = Math.max(yMin.toFixed(0).length, yMax.toFixed(0).length);
      const yAxisWidth = Math.max(40, 20 + charWidth * 6);

      const { left, right } = {
        left: 'dataMin',
        right: 'dataMax',
      };

      return (
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
            <Line yAxisId="1" type="linear" dataKey="y" stroke="#8884d8" animationDuration={300} dot={false} />
          </LineChart>
        </div>
      );
    }
  },
);
