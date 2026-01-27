// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';
import { render, fireEvent } from '@testing-library/react';
import { LineChart, ChartSeries } from '../LineChart';

// Polyfill PointerEvent for jsdom (which doesn't provide it)
if (typeof PointerEvent === 'undefined') {
  (global as any).PointerEvent = class PointerEvent extends MouseEvent {
    readonly pointerId: number;
    readonly width: number;
    readonly height: number;
    readonly pressure: number;
    readonly tiltX: number;
    readonly tiltY: number;
    readonly pointerType: string;
    readonly isPrimary: boolean;

    constructor(type: string, params: PointerEventInit & MouseEventInit = {}) {
      super(type, params);
      this.pointerId = params.pointerId ?? 0;
      this.width = params.width ?? 1;
      this.height = params.height ?? 1;
      this.pressure = params.pressure ?? 0;
      this.tiltX = params.tiltX ?? 0;
      this.tiltY = params.tiltY ?? 0;
      this.pointerType = params.pointerType ?? '';
      this.isPrimary = params.isPrimary ?? false;
    }
  };
}

// Mock ResizeObserver
class MockResizeObserver {
  callback: ResizeObserverCallback;
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
  }
  observe(target: Element) {
    // Immediately fire with a mock width
    this.callback(
      [
        {
          target,
          contentRect: { width: 400, height: 300 } as DOMRectReadOnly,
          borderBoxSize: [],
          contentBoxSize: [],
          devicePixelContentBoxSize: [],
        },
      ],
      this as unknown as ResizeObserver,
    );
  }
  unobserve() {}
  disconnect() {}
}

beforeAll(() => {
  (global as any).ResizeObserver = MockResizeObserver;
});

afterAll(() => {
  delete (global as any).ResizeObserver;
});

// Mock setPointerCapture / releasePointerCapture on elements
beforeEach(() => {
  Element.prototype.setPointerCapture = jest.fn();
  Element.prototype.releasePointerCapture = jest.fn();
});

const simpleSeries: ChartSeries[] = [
  {
    name: 'test',
    color: '#ff0000',
    points: [
      { x: 0, y: 0 },
      { x: 1, y: 10 },
      { x: 2, y: 20 },
      { x: 3, y: 30 },
    ],
  },
];

const multiSeries: ChartSeries[] = [
  {
    name: 'alpha',
    color: '#ff0000',
    points: [
      { x: 0, y: 0 },
      { x: 1, y: 10 },
      { x: 2, y: 5 },
    ],
  },
  {
    name: 'beta',
    color: '#0000ff',
    points: [
      { x: 0, y: 20 },
      { x: 1, y: 15 },
      { x: 2, y: 25 },
    ],
  },
];

describe('LineChart rendering', () => {
  test('renders an SVG element', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const svg = container.querySelector('svg');
    expect(svg).not.toBeNull();
  });

  test('renders SVG with correct height', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const svg = container.querySelector('svg');
    expect(svg?.getAttribute('height')).toBe('300');
  });

  test('renders horizontal grid lines', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const gridLines = container.querySelectorAll('.grid line');
    expect(gridLines.length).toBeGreaterThan(0);
  });

  test('renders x-axis with tick labels', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const xAxisTexts = container.querySelectorAll('.x-axis text');
    expect(xAxisTexts.length).toBeGreaterThan(0);
  });

  test('renders y-axis with tick labels', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const yAxisTexts = container.querySelectorAll('.y-axis text');
    expect(yAxisTexts.length).toBeGreaterThan(0);
  });

  test('renders one path per series', () => {
    const { container } = render(<LineChart height={300} series={multiSeries} yDomain={[0, 25]} />);
    const paths = container.querySelectorAll('.series-lines path');
    expect(paths.length).toBe(2);
  });

  test('sets correct stroke color on paths', () => {
    const { container } = render(<LineChart height={300} series={multiSeries} yDomain={[0, 25]} />);
    const paths = container.querySelectorAll('.series-lines path');
    expect(paths[0]?.getAttribute('stroke')).toBe('#ff0000');
    expect(paths[1]?.getAttribute('stroke')).toBe('#0000ff');
  });

  test('renders with empty series', () => {
    const { container } = render(<LineChart height={300} series={[]} yDomain={[0, 10]} />);
    const svg = container.querySelector('svg');
    expect(svg).not.toBeNull();
    // axes should still render
    const yAxisTexts = container.querySelectorAll('.y-axis text');
    expect(yAxisTexts.length).toBeGreaterThan(0);
  });

  test('applies clipPath to series lines group', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const seriesGroup = container.querySelector('.series-lines');
    const clipPathAttr = seriesGroup?.getAttribute('clip-path');
    expect(clipPathAttr).toMatch(/^url\(#plot-clip-.+\)$/);
  });

  test('renders overlay rect for pointer events', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const overlay = container.querySelector('.overlay');
    expect(overlay).not.toBeNull();
    expect(overlay?.getAttribute('fill')).toBe('none');
    expect(overlay?.getAttribute('pointer-events')).toBe('all');
  });
});

describe('LineChart tooltip', () => {
  test('shows tooltip on pointermove over overlay', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const overlay = container.querySelector('.overlay') as SVGRectElement;
    expect(overlay).not.toBeNull();

    // Simulate pointer move to trigger tooltip
    fireEvent.pointerMove(overlay, { clientX: 200, clientY: 150 });

    const tooltip = container.querySelector('[data-testid="chart-tooltip"]');
    expect(tooltip).not.toBeNull();
  });

  test('hides tooltip on pointerleave', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const overlay = container.querySelector('.overlay') as SVGRectElement;

    fireEvent.pointerMove(overlay, { clientX: 200, clientY: 150 });
    const tooltipBefore = container.querySelector('[data-testid="chart-tooltip"]');
    expect(tooltipBefore).not.toBeNull();

    fireEvent.pointerLeave(overlay);
    const tooltipAfter = container.querySelector('[data-testid="chart-tooltip"]');
    // tooltip should be hidden (null or display:none)
    expect(tooltipAfter === null || (tooltipAfter as HTMLElement).style.display === 'none').toBe(true);
  });

  test('shows crosshair line on pointermove', () => {
    const { container } = render(<LineChart height={300} series={simpleSeries} yDomain={[0, 30]} />);
    const overlay = container.querySelector('.overlay') as SVGRectElement;

    fireEvent.pointerMove(overlay, { clientX: 200, clientY: 150 });

    const crosshair = container.querySelector('.crosshair');
    expect(crosshair).not.toBeNull();
    // crosshair should be visible (not hidden)
    expect(crosshair?.getAttribute('visibility')).not.toBe('hidden');
  });

  test('formats tooltip values using tooltipFormatter', () => {
    const formatter = (v: number) => `$${v.toFixed(2)}`;
    const { container } = render(
      <LineChart height={300} series={simpleSeries} yDomain={[0, 30]} tooltipFormatter={formatter} />,
    );
    const overlay = container.querySelector('.overlay') as SVGRectElement;

    fireEvent.pointerMove(overlay, { clientX: 200, clientY: 150 });

    const tooltip = container.querySelector('[data-testid="chart-tooltip"]');
    expect(tooltip).not.toBeNull();
    // the tooltip should contain a formatted dollar value
    expect(tooltip?.textContent).toMatch(/\$/);
  });
});

describe('LineChart drag interaction', () => {
  test('calls onDragStart on pointerdown when drag enabled', () => {
    const onDragStart = jest.fn();
    const onDragEnd = jest.fn();
    const onPointDrag = jest.fn();

    const { container } = render(
      <LineChart
        height={300}
        series={simpleSeries}
        yDomain={[0, 30]}
        onPointDrag={onPointDrag}
        onDragStart={onDragStart}
        onDragEnd={onDragEnd}
      />,
    );

    const overlay = container.querySelector('.overlay') as SVGRectElement;
    fireEvent.pointerDown(overlay, { clientX: 200, clientY: 150, pointerId: 1 });

    expect(onDragStart).toHaveBeenCalledTimes(1);
  });

  test('calls setPointerCapture on pointerdown when drag enabled', () => {
    const onPointDrag = jest.fn();

    const { container } = render(
      <LineChart height={300} series={simpleSeries} yDomain={[0, 30]} onPointDrag={onPointDrag} />,
    );

    const overlay = container.querySelector('.overlay') as SVGRectElement;
    fireEvent.pointerDown(overlay, { clientX: 200, clientY: 150, pointerId: 1 });

    expect(overlay.setPointerCapture).toHaveBeenCalledWith(1);
  });

  test('calls onPointDrag on pointermove while dragging', () => {
    const onPointDrag = jest.fn();
    const onDragStart = jest.fn();

    const { container } = render(
      <LineChart
        height={300}
        series={simpleSeries}
        yDomain={[0, 30]}
        onPointDrag={onPointDrag}
        onDragStart={onDragStart}
      />,
    );

    const overlay = container.querySelector('.overlay') as SVGRectElement;
    fireEvent.pointerDown(overlay, { clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerMove(overlay, { clientX: 200, clientY: 100, pointerId: 1 });

    expect(onPointDrag).toHaveBeenCalled();
    // should be called with (seriesIndex, pointIndex, newY)
    const call = onPointDrag.mock.calls[0];
    expect(call[0]).toBe(0); // series index
    expect(typeof call[1]).toBe('number'); // point index
    expect(typeof call[2]).toBe('number'); // new Y value
  });

  test('clamps drag newY to yDomain', () => {
    const onPointDrag = jest.fn();

    const { container } = render(
      <LineChart height={300} series={simpleSeries} yDomain={[0, 30]} onPointDrag={onPointDrag} />,
    );

    const overlay = container.querySelector('.overlay') as SVGRectElement;
    fireEvent.pointerDown(overlay, { clientX: 200, clientY: 150, pointerId: 1 });
    // move way above the chart (clientY very small = high Y value)
    fireEvent.pointerMove(overlay, { clientX: 200, clientY: -1000, pointerId: 1 });

    if (onPointDrag.mock.calls.length > 0) {
      const lastCall = onPointDrag.mock.calls[onPointDrag.mock.calls.length - 1];
      const newY = lastCall[2];
      expect(newY).toBeLessThanOrEqual(30);
      expect(newY).toBeGreaterThanOrEqual(0);
    }
  });

  test('calls onDragEnd on pointerup', () => {
    const onDragStart = jest.fn();
    const onDragEnd = jest.fn();
    const onPointDrag = jest.fn();

    const { container } = render(
      <LineChart
        height={300}
        series={simpleSeries}
        yDomain={[0, 30]}
        onPointDrag={onPointDrag}
        onDragStart={onDragStart}
        onDragEnd={onDragEnd}
      />,
    );

    const overlay = container.querySelector('.overlay') as SVGRectElement;
    fireEvent.pointerDown(overlay, { clientX: 200, clientY: 150, pointerId: 1 });
    fireEvent.pointerUp(overlay, { clientX: 200, clientY: 150, pointerId: 1 });

    expect(onDragEnd).toHaveBeenCalledTimes(1);
  });

  test('does not call onPointDrag when not dragging', () => {
    const onPointDrag = jest.fn();

    const { container } = render(
      <LineChart height={300} series={simpleSeries} yDomain={[0, 30]} onPointDrag={onPointDrag} />,
    );

    const overlay = container.querySelector('.overlay') as SVGRectElement;
    // move without clicking first
    fireEvent.pointerMove(overlay, { clientX: 200, clientY: 100, pointerId: 1 });

    expect(onPointDrag).not.toHaveBeenCalled();
  });
});
