// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as React from 'react';

import { createStyles, Theme, withStyles, WithStyles } from '@material-ui/core/styles';

import { ViewElement, CloudViewElement } from '@system-dynamics/core/datamodel';

import { Point, Rect, square } from './common';
import { CloudRadius, CloudWidth } from './default';

import { defined } from '@system-dynamics/core/common';

const styles = ({ palette }: Theme) =>
  createStyles({
    cloud: {
      strokeWidth: 2,
      strokeLinejoin: 'round',
      strokeMiterlimit: 4,
      fill: palette.common.white,
      stroke: palette.mode === 'dark' ? '#2D498A' : '#6388dc',
    },
  });

const CloudPath =
  'M 25.731189,3.8741489 C 21.525742,3.8741489 18.07553,7.4486396 17.497605,' +
  '12.06118 C 16.385384,10.910965 14.996889,10.217536 13.45908,10.217535 C 9.8781481,' +
  '10.217535 6.9473481,13.959873 6.9473482,18.560807 C 6.9473482,19.228828 7.0507906,' +
  '19.875499 7.166493,20.498196 C 3.850265,21.890233 1.5000346,25.3185 1.5000346,29.310191' +
  ' C 1.5000346,34.243794 5.1009986,38.27659 9.6710049,38.715902 C 9.6186538,39.029349 ' +
  '9.6083922,39.33212 9.6083922,39.653348 C 9.6083922,45.134228 17.378069,49.59028 ' +
  '26.983444,49.590279 C 36.58882,49.590279 44.389805,45.134229 44.389803,39.653348 C ' +
  '44.389803,39.35324 44.341646,39.071755 44.295883,38.778399 C 44.369863,38.780301 ' +
  '44.440617,38.778399 44.515029,38.778399 C 49.470875,38.778399 53.499966,34.536825 ' +
  '53.499965,29.310191 C 53.499965,24.377592 49.928977,20.313927 45.360301,19.873232 C ' +
  '45.432415,19.39158 45.485527,18.91118 45.485527,18.404567 C 45.485527,13.821862 ' +
  '42.394553,10.092543 38.598118,10.092543 C 36.825927,10.092543 35.215888,10.918252 ' +
  '33.996078,12.248669 C 33.491655,7.5434856 29.994502,3.8741489 25.731189,3.8741489 z';

// eslint-disable-next-line @typescript-eslint/ban-types
type CloudState = {};

export interface CloudPropsFull extends WithStyles<typeof styles> {
  isSelected: boolean;
  onSelection: (element: ViewElement, e: React.PointerEvent<SVGElement>, isText?: boolean) => void;
  element: CloudViewElement;
}

export type CloudProps = Pick<CloudPropsFull, 'isSelected' | 'onSelection' | 'element'>;

export function cloudContains(element: CloudViewElement, point: Point): boolean {
  const cx = element.x;
  const cy = element.y;

  const distance = Math.sqrt(square(point.x - cx) + square(point.y - cy));
  return distance <= CloudRadius;
}

export function cloudBounds(element: CloudViewElement): Rect {
  const { x, y } = element;
  const radius = CloudRadius;
  return {
    top: (y || 0) - radius,
    left: (x || 0) - radius,
    right: (x || 0) + radius,
    bottom: (y || 0) + radius,
  };
}

export const Cloud = withStyles(styles)(
  class Cloud extends React.PureComponent<CloudPropsFull, CloudState> {
    state: CloudState;

    constructor(props: CloudPropsFull) {
      super(props);

      this.state = {};
    }

    handlePointerDown = (e: React.PointerEvent<SVGElement>): void => {
      e.preventDefault();
      e.stopPropagation();
      this.props.onSelection(this.props.element, e);
    };

    render() {
      const { element, classes } = this.props;
      const x = defined(element.x);
      const y = defined(element.y);

      const radius = CloudRadius;
      const diameter = radius * 2;

      const scale = diameter / CloudWidth;
      const t = `matrix(${scale}, 0, 0, ${scale}, ${x - radius}, ${y - radius})`;

      return <path d={CloudPath} className={classes.cloud} transform={t} onPointerDown={this.handlePointerDown} />;
    }
  },
);
