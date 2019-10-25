// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

import * as common from './common';

import { defined } from './common';
import { isModel, Model } from './model';
import { Project } from './project';

export { Model } from './model';
export { Project, stdProject } from './project';
export { Sim } from './sim';
export { Stock } from './vars';
export {
  ArrayElement as XmileArrayElement,
  Behavior as XmileBehavior,
  Connection as XmileConnection,
  Data as XmileData,
  Dimension as XmileDimension,
  File as XmileFile,
  FileFromJSON,
  Format as XmileFormat,
  GF as XmileGF,
  Header as XmileHeader,
  Model as XmileModel,
  Options as XmileOptions,
  Point as XmilePoint,
  Product as XmileProduct,
  Range as XmileRange,
  Scale as XmileScale,
  Shape as XmileShape,
  SimSpec as XmileSimSpec,
  Style as XmileStyle,
  Unit as XmileUnit,
  Variable as XmileVariable,
  View,
  View as XmileView,
  ViewElement,
  ViewElement as XmileViewElement,
} from './xmile';

export const Error = common.Error;

/**
 * Attempts to parse the given xml string describing an xmile
 * model into a Model object, returning the Model on success, or
 * null on error.  On error, a string describing what went wrong
 * can be obtained by calling sd.error().
 *
 * @param xmlDoc An XMLDocument containing a xmile model.
 * @return A valid Model object on success, or null on error.
 */
export function newModel(xmlDoc: XMLDocument): Model {
  const [project, err] = new Project().addXmileFile(xmlDoc);
  if (err) {
    throw err;
  }

  const model = defined(project).model();
  if (!isModel(model)) {
    throw new Error('unreachable');
  }
  return model;
}

export async function load(url: string): Promise<[Model, undefined] | [undefined, common.Error]> {
  const response = await fetch(url);
  if (response.status >= 400) {
    return [undefined, new common.Error(`fetch(${url}): status ${response.status}`)];
  }

  const body = await response.text();
  const parser = new DOMParser();
  const xml: XMLDocument = parser.parseFromString(body, 'application/xml');

  const mdl = newModel(xml);
  if (!mdl) {
    return [undefined, new common.Error('newModel failed')];
  }

  return [mdl, undefined];
}
