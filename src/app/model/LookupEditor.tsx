// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { CartesianGrid, Line, LineChart, Tooltip, XAxis, YAxis } from 'recharts';
import { createStyles, withStyles, WithStyles } from '@material-ui/core/styles';

import { Table } from '../../engine/vars';

import { defined } from '../common';

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

// eslint-disable-next-line @typescript-eslint/no-empty-interface
interface LookupEditorState {}

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

      this.lookupRef = React.createRef();

      this.state = {};
    }

    formatValue = (value: string | number | (string | number)[]): string | (string | number)[] => {
      return typeof value === 'number' ? value.toFixed(3) : value;
    };

    handleMouseUp = (_e: React.MouseEvent<LineChart>) => {
      console.log('mouse up!');
    };

    handleMouseDown = (_e: React.MouseEvent<LineChart>) => {
      console.log('mouse down!');
    };

    handleMouseMove = (e: React.MouseEvent<LineChart>) => {
      if (!e.hasOwnProperty('chartX') || !e.hasOwnProperty('chartY')) {
        return;
      }
      const chart: any = this.lookupRef.current;
      if (chart === null) {
        return;
      }
      if (!chart.state.xAxisMap || !chart.state.yAxisMap) {
        return;
      }
      const xAxisMap = getAnyElementOfObject(chart.state.xAxisMap);
      const yAxisMap = getAnyElementOfObject(chart.state.yAxisMap);
      const xScale = xAxisMap.scale;
      const yScale = yAxisMap.scale;

      if (!xScale || !xScale.invert || !yScale || !yScale.invert) {
        debugger;
        return;
      }

      const x = xScale.invert((e as any).chartX);
      const y = yScale.invert((e as any).chartY);

      console.log(`position: ${x.toFixed(3)},${y.toFixed(3)}`);
    };

    render() {
      const { variable } = this.props;

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

      const charWidth = Math.max(yMin.toFixed(0).length, yMax.toFixed(0).length);
      const yAxisWidth = Math.max(40, 20 + charWidth * 6);

      const { left, right } = {
        left: 'dataMin',
        right: 'dataMax',
      };

      return (
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
      );
    }
  },
);
