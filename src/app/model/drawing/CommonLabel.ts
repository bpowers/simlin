// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import { Map } from 'immutable';

import { ViewElement } from '../../../engine/xmile';

import { defined } from '../../common';

export const lineSpacing = 14;

export interface CommonLabelProps {
  uid: number;
  cx: number;
  cy: number;
  side: string;
  rw?: number;
  rh?: number;
}

export const LabelPadding = 4;

const SideMap = Map<number, string>([
  [0, 'right'],
  [1, 'bottom'],
  [2, 'left'],
  [3, 'top'],
]);

export const findSide = (element: ViewElement, defaultSide = 'bottom'): string => {
  if (element.labelSide) {
    const side = element.labelSide;
    // FIXME(bp) handle center 'side' case
    if (side === 'center') {
      return defaultSide;
    }
    return side;
  }
  if (element.labelAngle !== undefined) {
    const θ = (defined(element.labelAngle) + 45) % 360;
    const i = (θ / 90) | 0;
    return defined(SideMap.get(i));
  }
  return defaultSide;
};
