// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import {
  linearScale,
  invertLinearScale,
  niceAxisTicks,
  computeYAxisWidth,
  findNearestPointIndex,
  formatTickLabel,
} from './chart-utils';

import styles from './LineChart.module.css';

export interface ChartSeries {
  name: string;
  color: string;
  points: ReadonlyArray<{ x: number; y: number }>;
}

export interface LineChartProps {
  height: number;
  series: ChartSeries[];
  yDomain: [number, number];
  gridLines?: 'horizontal' | 'none';
  tooltipFormatter?: (value: number) => string;
  onPointDrag?: (seriesIndex: number, pointIndex: number, newY: number) => void;
  onDragStart?: () => void;
  onDragEnd?: () => void;
}

interface TooltipState {
  visible: boolean;
  x: number; // pixel x in SVG coords
  y: number; // pixel y in container coords (for HTML tooltip positioning)
  dataX: number;
  seriesValues: Array<{ name: string; color: string; value: number }>;
}

let nextId = 0;

export function LineChart(props: LineChartProps): React.ReactElement {
  const { height, series, gridLines: gridLinesProp, tooltipFormatter, onPointDrag, onDragStart, onDragEnd } = props;

  // A stable clip-path id generated once per mount (lazy initializer), mirroring
  // the old constructor's one-shot counter bump.
  const [clipId] = React.useState(() => `plot-clip-${nextId++}`);
  const containerRef = React.useRef<HTMLDivElement | null>(null);
  // The active-drag flag is a plain mutable instance field (not render state),
  // exactly as the class's `dragging` was: it gates pointer-move handling but
  // must not trigger a re-render when it flips.
  const dragging = React.useRef(false);

  const [containerWidth, setContainerWidth] = React.useState(0);
  const [tooltip, setTooltip] = React.useState<TooltipState>({
    visible: false,
    x: 0,
    y: 0,
    dataX: 0,
    seriesValues: [],
  });

  // Mount/unmount: observe the container's width. The ResizeObserver setState
  // re-derives layout on resize; the cleanup is symmetric so a StrictMode
  // mount/unmount/mount cycle leaves no dangling observer.
  React.useEffect(() => {
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === 'undefined') {
      return undefined;
    }
    const resizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const width = entry.contentRect.width;
        // setContainerWidth bails on identical values, so the `!==` guard the
        // class used to avoid redundant setState is preserved by React itself.
        setContainerWidth(width);
      }
    });
    resizeObserver.observe(el);
    return () => {
      resizeObserver.disconnect();
    };
  }, []);

  function getLayout() {
    const { yDomain } = props;

    let yMin = yDomain[0];
    let yMax = yDomain[1];
    if (yMin === yMax) {
      yMin -= 1;
      yMax += 1;
    }

    const yTicks = niceAxisTicks(yMin, yMax);
    const yAxisWidth = computeYAxisWidth(yTicks);

    // compute x domain from data extent across all series
    let xMin = Infinity;
    let xMax = -Infinity;
    for (const s of series) {
      for (const p of s.points) {
        if (p.x < xMin) xMin = p.x;
        if (p.x > xMax) xMax = p.x;
      }
    }
    if (!isFinite(xMin)) {
      xMin = 0;
      xMax = 1;
    }
    if (xMin === xMax) {
      xMin -= 1;
      xMax += 1;
    }

    const xTicks = niceAxisTicks(xMin, xMax);

    const margin = { top: 10, right: 20, bottom: 30, left: yAxisWidth };
    const plotWidth = Math.max(0, containerWidth - margin.left - margin.right);
    const plotHeight = Math.max(0, height - margin.top - margin.bottom);

    const xScale = linearScale([xMin, xMax], [0, plotWidth]);
    const yScale = linearScale([yMin, yMax], [plotHeight, 0]);
    const yInvert = invertLinearScale([yMin, yMax], [plotHeight, 0]);

    return { margin, plotWidth, plotHeight, xScale, yScale, yInvert, xTicks, yTicks, xMin, xMax };
  }

  function buildPath(
    points: ReadonlyArray<{ x: number; y: number }>,
    xScale: (v: number) => number,
    yScale: (v: number) => number,
  ): string {
    const parts: string[] = [];
    let started = false;
    for (const p of points) {
      if (!isFinite(p.x) || !isFinite(p.y)) {
        started = false;
        continue;
      }
      const px = xScale(p.x);
      const py = yScale(p.y);
      if (!started) {
        parts.push(`M${px},${py}`);
        started = true;
      } else {
        parts.push(`L${px},${py}`);
      }
    }
    return parts.join('');
  }

  const handlePointerMove = (e: React.PointerEvent<SVGRectElement>): void => {
    const layout = getLayout();
    const { margin, plotWidth, plotHeight, xScale, yInvert, xMin, xMax } = layout;

    if (plotWidth <= 0 || plotHeight <= 0) return;

    // Compute pointer position relative to the plot area.
    const overlayRect = e.currentTarget.getBoundingClientRect();
    const plotX = e.clientX - overlayRect.left;
    const plotY = e.clientY - overlayRect.top;

    const clampedX = Math.max(0, Math.min(plotWidth, plotX));
    const clampedY = Math.max(0, Math.min(plotHeight, plotY));

    // Convert pixel x to data x, then snap to the nearest data point.
    // All series share the same time axis, so we search once and reuse
    // the index for tooltip values and drag callbacks.
    const rawDataX = xMin + (clampedX / plotWidth) * (xMax - xMin);
    let snappedDataX = rawDataX;
    let snappedIdx = -1;
    for (const s of series) {
      if (s.points.length === 0) continue;
      snappedIdx = findNearestPointIndex(s.points, rawDataX);
      if (snappedIdx >= 0) {
        snappedDataX = s.points[snappedIdx].x;
      }
      break;
    }

    // Build tooltip values using the cached index.
    const seriesValues: TooltipState['seriesValues'] = [];
    if (snappedIdx >= 0) {
      for (const s of series) {
        if (snappedIdx < s.points.length) {
          seriesValues.push({ name: s.name, color: s.color, value: s.points[snappedIdx].y });
        }
      }
    }

    const crosshairX = margin.left + xScale(snappedDataX);

    setTooltip({
      visible: true,
      x: crosshairX,
      y: margin.top + clampedY,
      dataX: snappedDataX,
      seriesValues,
    });

    // handle drag
    if (dragging.current && onPointDrag && snappedIdx >= 0) {
      let newY = yInvert(clampedY);
      const [yLo, yHi] = props.yDomain;
      newY = Math.max(yLo, Math.min(yHi, newY));

      for (let si = 0; si < series.length; si++) {
        if (snappedIdx < series[si].points.length) {
          onPointDrag(si, snappedIdx, newY);
        }
      }
    }
  };

  const handlePointerLeave = (): void => {
    if (!dragging.current) {
      setTooltip((prev) => ({ ...prev, visible: false }));
    }
  };

  const handlePointerDown = (e: React.PointerEvent<SVGRectElement>): void => {
    if (!onPointDrag) return;

    dragging.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
    onDragStart?.();

    // also process this as a move to update the point
    handlePointerMove(e);
  };

  const handlePointerUp = (e: React.PointerEvent<SVGRectElement>): void => {
    if (!dragging.current) return;
    e.currentTarget.releasePointerCapture(e.pointerId);
    dragging.current = false;
    onDragEnd?.();
  };

  const handlePointerCancel = (e: React.PointerEvent<SVGRectElement>): void => {
    if (!dragging.current) return;
    e.currentTarget.releasePointerCapture(e.pointerId);
    dragging.current = false;
    onDragEnd?.();
  };

  const gridLines = gridLinesProp ?? 'horizontal';

  if (containerWidth === 0) {
    return <div ref={containerRef} className={styles.container} />;
  }

  const layout = getLayout();
  const { margin, plotWidth, plotHeight, xScale, yScale, xTicks, yTicks } = layout;

  const defaultFormatter = (v: number) => v.toString();
  const fmt = tooltipFormatter ?? defaultFormatter;

  // tooltip position clamped to container bounds
  let tooltipLeft = tooltip.x + 12;
  const tooltipEstWidth = 150;
  if (tooltipLeft + tooltipEstWidth > containerWidth) {
    tooltipLeft = tooltip.x - tooltipEstWidth - 12;
  }
  if (tooltipLeft < 0) tooltipLeft = 0;

  return (
    <div ref={containerRef} className={styles.container}>
      <svg width={containerWidth} height={height}>
        <defs>
          <clipPath id={clipId}>
            <rect x={0} y={0} width={plotWidth} height={plotHeight} />
          </clipPath>
        </defs>

        {/* Grid lines */}
        {gridLines === 'horizontal' && (
          <g className="grid">
            {yTicks.map((tick, i) => {
              const y = margin.top + yScale(tick);
              return (
                <line
                  key={i}
                  x1={margin.left}
                  y1={y}
                  x2={margin.left + plotWidth}
                  y2={y}
                  stroke="#e0e0e0"
                  strokeWidth={1}
                />
              );
            })}
          </g>
        )}

        {/* X axis */}
        <g className="x-axis">
          <line
            x1={margin.left}
            y1={margin.top + plotHeight}
            x2={margin.left + plotWidth}
            y2={margin.top + plotHeight}
            stroke="#666"
            strokeWidth={1}
          />
          {xTicks.map((tick, i) => {
            const x = margin.left + xScale(tick);
            return (
              <g key={i}>
                <line
                  x1={x}
                  y1={margin.top + plotHeight}
                  x2={x}
                  y2={margin.top + plotHeight + 5}
                  stroke="#666"
                  strokeWidth={1}
                />
                <text x={x} y={margin.top + plotHeight + 18} textAnchor="middle" fontSize={11} fill="#666">
                  {formatTickLabel(tick)}
                </text>
              </g>
            );
          })}
        </g>

        {/* Y axis */}
        <g className="y-axis">
          <line
            x1={margin.left}
            y1={margin.top}
            x2={margin.left}
            y2={margin.top + plotHeight}
            stroke="#666"
            strokeWidth={1}
          />
          {yTicks.map((tick, i) => {
            const y = margin.top + yScale(tick);
            return (
              <g key={i}>
                <line x1={margin.left - 5} y1={y} x2={margin.left} y2={y} stroke="#666" strokeWidth={1} />
                <text x={margin.left - 8} y={y} textAnchor="end" dominantBaseline="middle" fontSize={11} fill="#666">
                  {formatTickLabel(tick)}
                </text>
              </g>
            );
          })}
        </g>

        {/* Series lines */}
        <g className="series-lines" clipPath={`url(#${clipId})`} transform={`translate(${margin.left},${margin.top})`}>
          {series.map((s, i) => (
            <path key={i} d={buildPath(s.points, xScale, yScale)} fill="none" stroke={s.color} strokeWidth={1.5} />
          ))}
        </g>

        {/* Crosshair */}
        <line
          className="crosshair"
          x1={tooltip.x}
          y1={margin.top}
          x2={tooltip.x}
          y2={margin.top + plotHeight}
          stroke="#999"
          strokeWidth={1}
          strokeDasharray="3,3"
          visibility={tooltip.visible ? 'visible' : 'hidden'}
        />

        {/* Overlay for pointer events */}
        <rect
          className="overlay"
          x={margin.left}
          y={margin.top}
          width={plotWidth}
          height={plotHeight}
          fill="none"
          pointerEvents="all"
          style={{ touchAction: 'none' }}
          onPointerMove={handlePointerMove}
          onPointerLeave={handlePointerLeave}
          onPointerDown={handlePointerDown}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
        />
      </svg>

      {/* HTML Tooltip */}
      {tooltip.visible && tooltip.seriesValues.length > 0 && (
        <div
          data-testid="chart-tooltip"
          className={styles.tooltip}
          style={{
            left: tooltipLeft,
            top: Math.max(0, tooltip.y - 10),
          }}
        >
          <div className={styles.tooltipHeader}>{formatTickLabel(tooltip.dataX)}</div>
          {tooltip.seriesValues.map((sv, i) => (
            <div key={i} className={styles.tooltipRow}>
              <span className={styles.tooltipSwatch} style={{ backgroundColor: sv.color }} />
              <span>
                {sv.name}: {fmt(sv.value)}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
