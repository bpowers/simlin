// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { List } from 'immutable';
import { styled } from '@mui/material/styles';
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
  useOffset,
  useChartHeight,
} from 'recharts';
import type { CategoricalChartFunc } from 'recharts/types/chart/types';
import type { ChartOffset } from 'recharts/types';
import { Button, CardActions, CardContent, TextField } from '@mui/material';

import { defined } from '@system-dynamics/core/common';
import {
  Variable,
  GraphicalFunction,
  GraphicalFunctionScale,
  GraphicalFunctionKind,
} from '@system-dynamics/core/datamodel';

import { isEqual } from './drawing/common';

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
  const ypts: List<number> = gf.yPoints;

  const size = gf.yPoints.size;
  const xList = new Float64Array(size);
  const yList = new Float64Array(size);

  for (let i = 0; i < ypts.size; i++) {
    // either the x points have been explicitly specified, or
    // it is a linear mapping of points between xmin and xmax,
    // inclusive
    const xVal = xpts ? defined(xpts.get(i)) : (i / (size - 1)) * (xmax - xmin) + xmin;
    xList[i] = xVal;
    yList[i] = defined(ypts.get(i));
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

// Chart height from ResponsiveContainer
const CHART_HEIGHT = 300;

// Chart layout info extracted from recharts hooks
interface ChartLayoutInfo {
  chartHeight: number;
  offset: ChartOffset;
}

// Functional component that extracts chart layout info from recharts context
// and passes it to the parent via a callback. This must be rendered inside
// the LineChart to have access to the chart context.
function ChartLayoutExtractor({
  onLayoutChange,
}: {
  onLayoutChange: (layoutInfo: ChartLayoutInfo | undefined) => void;
}) {
  const offset = useOffset();
  const chartHeight = useChartHeight();

  React.useEffect(() => {
    if (offset && chartHeight) {
      onLayoutChange({ chartHeight, offset });
    } else {
      onLayoutChange(undefined);
    }
  }, [offset, chartHeight, onLayoutChange]);

  return null;
}

// Convert pixel Y coordinate to data Y value using chart layout info and Y domain
function pixelToDataY(pixelY: number, layoutInfo: ChartLayoutInfo, yDomain: { min: number; max: number }): number {
  const { chartHeight, offset } = layoutInfo;
  const plotTop = offset.top;
  const plotBottom = chartHeight - offset.bottom;
  const plotHeight = plotBottom - plotTop;

  if (plotHeight <= 0) {
    return yDomain.min;
  }

  // Calculate relative position within the plot area (0 = top, 1 = bottom)
  const relativeY = (pixelY - plotTop) / plotHeight;

  // Convert to data value (inverted because SVG Y increases downward)
  // At relativeY = 0 (top), we want yDomain.max
  // At relativeY = 1 (bottom), we want yDomain.min
  return yDomain.max - relativeY * (yDomain.max - yDomain.min);
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

export const LookupEditor = styled(
  class InnerLookupEditor extends React.PureComponent<LookupEditorProps & { className?: string }, LookupEditorState> {
    readonly containerRef: React.RefObject<HTMLDivElement | null>;

    // Stored as an instance variable rather than state because:
    // 1. We don't need re-renders when layoutInfo changes (it's only read during drag events)
    // 2. Drag events always occur after the initial render when layoutInfo is populated
    // 3. Making it state would cause unnecessary re-renders on every layout change
    layoutInfo: ChartLayoutInfo | undefined;

    constructor(props: LookupEditorProps) {
      super(props);

      const gf = this.getVariableGF();
      const table = defined(tableFrom(gf));

      this.containerRef = React.createRef();
      this.layoutInfo = undefined;
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
      let gf = defined(variable.gf);

      // ensure yScale always exists
      if (!gf.yScale) {
        let min = 0;
        let max = 0;
        for (let i = 0; i < gf.yPoints.size; i++) {
          const y = defined(gf.yPoints.get(i));
          if (y < min) {
            min = y;
          }
          if (y > max) {
            max = y;
          }
        }
        min = Math.floor(min);
        max = Math.ceil(max);

        gf = gf.set('yScale', new GraphicalFunctionScale({ min, max }));
      }

      return gf;
    }

    formatValue = (value: number | string | Array<number | string> | undefined): string => {
      if (value === undefined) {
        return '';
      }
      return typeof value === 'number' ? value.toFixed(3) : value.toString();
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
      // ensure we get 'mouse up' events (and friends) even if we leave the
      // confines of the chart
      const target = e.target as HTMLElement | null;
      if (!target || typeof target.setPointerCapture !== 'function') {
        return;
      }

      target.setPointerCapture(e.pointerId);
    };

    handleContainerTouchEnd = (_e: React.PointerEvent<HTMLDivElement>) => {};

    handleContainerTouchMove = (_e: React.PointerEvent<HTMLDivElement>) => {};

    handleMouseUp = () => {
      this.endEditing();
    };

    endEditing() {
      this.setState({ inDrag: false });
    }

    // Class arrow function properties have stable identity across renders (assigned once
    // at instance creation), so this won't cause unnecessary useEffect re-runs in
    // ChartLayoutExtractor despite being in its dependency array.
    handleLayoutChange = (layoutInfo: ChartLayoutInfo | undefined) => {
      this.layoutInfo = layoutInfo;
    };

    updatePoint(
      activeTooltipIndex: number | string | null | undefined,
      activeLabel: string | number | undefined,
      event: React.SyntheticEvent,
    ) {
      const container = this.containerRef.current;
      if (!container || !this.layoutInfo) {
        return;
      }

      const { gf, table } = this.state;

      const newTable = Object.assign({}, table);
      newTable.y = new Float64Array(table.y);

      const yMin = defined(gf.yScale).min;
      const yMax = defined(gf.yScale).max;

      // Get mouse/touch Y position relative to container.
      // Handle both MouseEvent (has clientY directly) and TouchEvent (has clientY on Touch objects).
      const nativeEvent = event.nativeEvent;
      let clientY: number | undefined;
      if ('clientY' in nativeEvent && typeof nativeEvent.clientY === 'number') {
        // MouseEvent or PointerEvent
        clientY = nativeEvent.clientY;
      } else if ('touches' in nativeEvent) {
        // TouchEvent - get from first touch point
        const touchEvent = nativeEvent as TouchEvent;
        const touch = touchEvent.touches[0] ?? touchEvent.changedTouches[0];
        if (touch) {
          clientY = touch.clientY;
        }
      }
      if (clientY === undefined) {
        return;
      }

      const rect = container.getBoundingClientRect();
      const relativeY = clientY - rect.top;

      // Use the layout info from recharts to convert pixel to data coordinates
      let y = pixelToDataY(relativeY, this.layoutInfo, { min: yMin, max: yMax });

      // Clamp to domain
      if (y > yMax) {
        y = yMax;
      } else if (y < yMin) {
        y = yMin;
      }

      let tableIndex: number | undefined;
      if (typeof activeTooltipIndex === 'number' && Number.isInteger(activeTooltipIndex)) {
        tableIndex = activeTooltipIndex;
      }

      if (tableIndex === undefined || tableIndex < 0 || tableIndex >= newTable.size) {
        tableIndex = undefined;
      }

      // Fallback: if activeTooltipIndex didn't give us a valid index, try to find
      // the point by matching the X value from activeLabel. This handles edge
      // cases where recharts doesn't provide a valid index (e.g., when points are
      // very close together).
      if (tableIndex === undefined && typeof activeLabel === 'number') {
        for (let i = 0; i < newTable.size; i++) {
          if (isEqual(newTable.x[i], activeLabel)) {
            tableIndex = i;
            break;
          }
        }
      }

      if (tableIndex === undefined) {
        return;
      }

      newTable.y[tableIndex] = y;
      this.setState({
        hasChange: true,
        table: newTable,
      });
    }

    handleMouseDown: CategoricalChartFunc = (nextState, event) => {
      this.setState({ inDrag: true });
      // nextState can be null when clicking outside the plot area (e.g., on axis labels)
      if (!nextState) {
        return;
      }
      this.updatePoint(nextState.activeTooltipIndex, nextState.activeLabel, event);
    };

    handleMouseMove: CategoricalChartFunc = (nextState, event) => {
      if (!this.state.inDrag) {
        return;
      }

      // If we were dragging in the chart, left the chart, stopped pressing the mouse/touch,
      // then moved back in we might mistakenly think we were still inDrag.
      // Check for buttons property which exists on MouseEvent and PointerEvent (but not TouchEvent).
      const nativeEvent = event.nativeEvent;
      if ('buttons' in nativeEvent && (nativeEvent as PointerEvent).buttons === 0) {
        this.endEditing();
        return;
      }

      // nextState can be null when the pointer is outside the plot area
      if (!nextState) {
        return;
      }
      this.updatePoint(nextState.activeTooltipIndex, nextState.activeLabel, event);
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
            new GraphicalFunctionScale({
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
            new GraphicalFunctionScale({
              min: yMin,
              max: value,
            }),
          ),
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
      const yPoints = table.y.reduce((pts: List<number>, curr: number) => pts.push(curr), List());
      this.props.onLookupChange(defined(this.props.variable.ident), gf.set('yPoints', yPoints));
      this.setState({ hasChange: false });
    };

    render() {
      const { className } = this.props;
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
        const xVal = table.x[i];
        const yVal = table.y[i];
        series.push({ x: xVal, y: yVal });
      }

      const xScaleError = xMin >= xMax;
      const yScaleError = yMin >= yMax;
      const datapointCountError = datapointCount <= 0;

      const isSaveDisabled = !lookupActionsEnabled || xScaleError || yScaleError || datapointCountError;

      return (
        <div className={className}>
          <CardContent>
            <TextField
              className="simlin-lookupeditor-yaxismax"
              error={yScaleError}
              label="Y axis max"
              value={yMax}
              onChange={this.handleYMaxChange}
              type="number"
              margin="normal"
            />
            <div
              ref={this.containerRef}
              onMouseDown={this.handleContainerMouseDown}
              onMouseUp={this.handleContainerMouseUp}
              onMouseMove={this.handleContainerMouseMove}
              onPointerDown={this.handleContainerTouchStart}
              onPointerUp={this.handleContainerTouchEnd}
              onPointerMove={this.handleContainerTouchMove}
            >
              <ResponsiveContainer width="100%" height={CHART_HEIGHT}>
                <LineChart
                  data={series}
                  onMouseDown={this.handleMouseDown}
                  onMouseMove={this.handleMouseMove}
                  onMouseUp={this.handleMouseUp}
                  layout={'horizontal'}
                >
                  <ChartLayoutExtractor onLayoutChange={this.handleLayoutChange} />
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
              </ResponsiveContainer>
            </div>
            <TextField
              className="simlin-lookupeditor-yaxismax"
              error={yScaleError}
              label="Y axis min"
              value={yMin}
              onChange={this.handleYMinChange}
              type="number"
              margin="normal"
            />
            <br />
            <TextField
              className="simlin-lookupeditor-xscalemin"
              error={xScaleError}
              label="X axis min"
              value={xMin}
              onChange={this.handleXMinChange}
              type="number"
              margin="normal"
            />
            <TextField
              className="simlin-lookupeditor-xscalemax"
              error={xScaleError}
              label="X axis max"
              value={xMax}
              onChange={this.handleXMaxChange}
              type="number"
              margin="normal"
            />
            <TextField
              className="simlin-lookupeditor-datapoints"
              error={datapointCountError}
              label="Datapoint Count"
              value={datapointCount}
              onChange={this.handleDatapointCountChange}
              type="number"
              margin="normal"
            />
          </CardContent>
          <CardActions>
            <Button
              size="small"
              color="secondary"
              onClick={this.handleLookupRemove}
              className="simlin-lookupeditor-buttonleft"
            >
              Remove
            </Button>
            <div className="simlin-lookupeditor-buttonright">
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
)(() => ({
  '.simlin-lookupeditor-yaxismax': {
    width: '30%',
    paddingRight: 4,
    marginTop: 0,
  },
  '.simlin-lookupeditor-yaxismin': {
    width: '30%',
    paddingRight: 4,
    marginTop: 4,
  },
  '.simlin-lookupeditor-xscalemin': {
    width: '30%',
    paddingRight: 4,
  },
  '.simlin-lookupeditor-xscalemax': {
    width: '30%',
    paddingLeft: 4,
    paddingRight: 4,
  },
  '.simlin-lookupeditor-datapoints': {
    width: '40%',
    paddingLeft: 4,
  },
  '.simlin-lookupeditor-buttonleft': {
    float: 'left',
    marginRight: 'auto',
  },
  '.simlin-lookupeditor-buttonright': {
    float: 'right',
  },
}));
